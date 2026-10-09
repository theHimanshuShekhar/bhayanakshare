//! A tree whose paths are longer than the 260 characters Windows once allowed arrives whole. The
//! Sender is written by hand and serves the files from memory, so that what is tested is how the
//! Receiver creates, writes, exports into and moves the tree, and what it says about the paths
//! before and after. On Windows that only works with extended-length (`\\?\`) paths; the tree is
//! read back here with plain `std::fs`, which converts the paths it is given.

mod support;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};

use bhayanakshare_core::{
    INCOMING_DIR, TransferId, TransferState,
    manifest::{Entry, Manifest},
    protocol::{self, Hello, Message, Offer, read_frame, write_frame},
};
use iroh::protocol::Router;
use iroh_blobs::{BlobsProtocol, format::collection::Collection, store::mem::MemStore};
use support::{TestDevice, dial_addr, list_dir, pseudo_random_bytes, raw_peer};

/// The modification time of every file sent. NTFS keeps times to 100 ns, so a whole second is
/// kept as it is.
const MTIME: Duration = Duration::from_secs(1_600_000_000);

/// `depth` folders of 50 characters each, one inside the next, as a relative `/` path.
fn nested(depth: usize) -> String {
    (0..depth).map(|level| format!("{level:02}{}", "n".repeat(48))).collect::<Vec<_>>().join("/")
}

/// Every folder (`None`) and file (its contents) under `dir`, by `/` path relative to `dir`.
/// The time of each file is checked on the way.
fn read_back(dir: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    fn walk(dir: &Path, rel: &str, out: &mut BTreeMap<String, Option<Vec<u8>>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let rel = format!("{rel}{}", entry.file_name().to_string_lossy());
            if entry.file_type().unwrap().is_dir() {
                out.insert(rel.clone(), None);
                walk(&entry.path(), &format!("{rel}/"), out);
            } else {
                let modified = entry.metadata().unwrap().modified().unwrap();
                assert_eq!(modified, UNIX_EPOCH + MTIME, "{rel}");
                out.insert(rel, Some(std::fs::read(entry.path()).unwrap()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, "", &mut out);
    out
}

/// Bob, and a hand-written Sender that has offered `files` (and the empty folder `empty`) to him
/// and will serve them once told he accepted.
struct Offered {
    bob: TestDevice,
    id: TransferId,
    root: iroh_blobs::Hash,
    send: iroh::endpoint::SendStream,
    recv: iroh::endpoint::RecvStream,
    // Kept so that the Sender stays up: the tags keep the files in its store.
    _keep: (iroh::endpoint::Connection, Router, Vec<iroh_blobs::api::TempTag>),
}

async fn offer(files: &[(String, Vec<u8>)], empty: &str) -> Offered {
    let mut bob = TestDevice::start("bob").await;
    let mtime_ns = i64::try_from(MTIME.as_nanos()).unwrap();
    let mut entries: Vec<Entry> = files
        .iter()
        .map(|(path, bytes)| Entry::File { path: path.clone(), size: bytes.len() as u64, mtime_ns, executable: false })
        .collect();
    entries.push(Entry::empty_dir(empty));

    let store = MemStore::new();
    let mut tags = Vec::new();
    let mut named = Vec::new();
    for (path, bytes) in files {
        let tag = store.blobs().add_bytes(bytes.clone()).temp_tag().await.unwrap();
        named.push((path.clone(), tag.hash()));
        tags.push(tag);
    }
    let root = Collection::from_iter(named).store(&store).await.unwrap().hash();
    let peer = raw_peer().await;
    let provider = Router::builder(peer.clone()).accept(iroh_blobs::ALPN, BlobsProtocol::new(&store, None)).spawn();

    let conn = peer.connect(dial_addr(&bob), protocol::ALPN).await.unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, &Message::Hello(Hello::current())).await.unwrap();
    assert!(matches!(read_frame(&mut recv).await.unwrap(), Message::Hello(_)));
    let id = TransferId::from_bytes([5; 16]);
    write_frame(&mut send, &Message::Offer(Offer::new(*id.as_bytes(), Manifest { entries }, 0))).await.unwrap();
    bob.wait_offer().await;
    Offered { bob, id, root, send, recv, _keep: (conn, provider, tags) }
}

impl Offered {
    /// Bob accepts into `folder` (his save folder if `None`) and the Sender serves; waits for
    /// the Transfer to complete and returns where Bob says it went.
    async fn accept_and_complete(&mut self, folder: Option<&Path>) -> String {
        self.bob.device.accept_into(self.id, folder).await.unwrap();
        assert!(matches!(read_frame(&mut self.recv).await.unwrap(), Message::Accept));
        write_frame(&mut self.send, &Message::HashReady { collection_hash: *self.root.as_bytes() }).await.unwrap();
        let completed = self.bob.wait_state_big(self.id, "completed").await;
        let TransferState::Completed { saved_to: Some(saved_to) } = completed.state else { panic!("{completed:?}") };
        // What Bob's user sees and History keeps is the plain path, never the `\\?\` form that
        // Windows needs underneath.
        assert!(!saved_to.starts_with(r"\\?\"), "{saved_to}");
        let record = &self.bob.device.transfers().await.unwrap()[0];
        assert_eq!(record.state, TransferState::Completed { saved_to: Some(saved_to.clone()) });
        assert_eq!(self.bob.history(self.id), ["offered", "accepted", "transferring", "saving", "completed"]);
        saved_to
    }
}

/// The files of a tree with folders `depth` deep: two at the bottom, one near the top.
fn tree(depth: usize) -> (String, String, Vec<(String, Vec<u8>)>) {
    let (top, deep) = (nested(1), nested(depth));
    let files = vec![
        (format!("{deep}/small.txt"), b"small".to_vec()),
        // Big enough to live in a file of its own in the store, not inline.
        (format!("{deep}/big.bin"), pseudo_random_bytes(300 * 1024, 7)),
        (format!("{top}/shallow.txt"), b"shallow".to_vec()),
    ];
    (top, deep, files)
}

/// What `read_back` finds of the whole tree under `top`, relative to the save folder.
fn expected(files: &[(String, Vec<u8>)], empty: &str) -> BTreeMap<String, Option<Vec<u8>>> {
    let mut map = BTreeMap::new();
    let mut folders = vec![empty.to_owned()];
    for (path, bytes) in files {
        map.insert(path.clone(), Some(bytes.clone()));
        folders.push(path.rsplit_once('/').unwrap().0.to_owned());
    }
    for folder in folders {
        let mut at = String::new();
        for name in folder.split('/') {
            at = if at.is_empty() { name.to_owned() } else { format!("{at}/{name}") };
            map.entry(at.clone()).or_insert(None);
        }
    }
    map
}

/// Six folders of 50 characters: 300 for the folders alone, over 400 with the save folder.
#[tokio::test]
async fn a_tree_whose_paths_pass_260_characters_arrives_whole() {
    let (top, deep, files) = tree(6);
    let empty = format!("{deep}/empty");
    let mut sent = offer(&files, &empty).await;
    let bob_save = sent.bob.save_dir.clone();
    assert!(bob_save.join(&deep).join("small.txt").as_os_str().len() > 260);
    let check = sent.bob.device.check_offer(sent.id, None).await.unwrap();
    assert!(!check.paths_too_long && check.passes());

    let saved_to = sent.accept_and_complete(None).await;

    assert_eq!(saved_to, bob_save.join(&top).to_string_lossy());
    assert_eq!(read_back(&bob_save.join(&top)), expected(&files, &empty).into_iter().filter_map(|(path, node)| {
        path.strip_prefix(&format!("{top}/")).map(|rest| (rest.to_owned(), node))
    }).collect());
    // Nothing is left of the incoming store, which is deleted as the Device winds down.
    sent.bob.shutdown().await;
    assert_eq!(list_dir(&bob_save), [INCOMING_DIR.to_owned(), top]);
    assert!(list_dir(&bob_save.join(INCOMING_DIR)).is_empty(), "incoming store left behind");
}

/// 24 folders: past the 1,023 characters this app once held a Windows Receiver to, for want of a
/// better number, and well within what Windows allows with extended-length paths.
#[tokio::test]
async fn a_tree_whose_paths_pass_a_thousand_characters_is_not_held_back() {
    let (top, deep, files) = tree(24);
    let empty = format!("{deep}/empty");
    let mut sent = offer(&files, &empty).await;
    let check = sent.bob.device.check_offer(sent.id, None).await.unwrap();
    assert!(!check.paths_too_long && check.passes());

    sent.accept_and_complete(None).await;

    let saved = sent.bob.save_dir.join(&top);
    let found = read_back(&saved);
    assert_eq!(found[&deep.strip_prefix(&format!("{top}/")).unwrap().to_owned()], None);
    assert_eq!(found.values().filter(|node| node.is_some()).count(), 3);
    sent.bob.shutdown().await;
}

/// The folder Bob chooses for an Offer is itself past 260 characters: the check before Accept
/// reads its free space, and the tree is saved in it.
#[tokio::test]
async fn a_save_folder_whose_own_path_passes_260_characters_takes_a_tree() {
    let (top, _, files) = tree(1);
    let empty = format!("{top}/empty");
    let mut sent = offer(&files[2..], &empty).await;
    let elsewhere = tempfile::tempdir().unwrap();
    // One name at a time, so that the separators are the system's and the Device's answer compares.
    let folder: PathBuf = nested(6).split('/').fold(elsewhere.path().to_owned(), |dir, name| dir.join(name));
    std::fs::create_dir_all(&folder).unwrap();
    assert!(folder.as_os_str().len() > 260);
    let check = sent.bob.device.check_offer(sent.id, Some(&folder)).await.unwrap();
    assert!(check.free.is_some(), "free space is not read for a long folder");
    assert!(!check.paths_too_long && check.passes());

    let saved_to = sent.accept_and_complete(Some(&folder)).await;

    assert_eq!(saved_to, folder.join(&top).to_string_lossy());
    assert_eq!(read_back(&folder.join(&top)).len(), 2);
    sent.bob.shutdown().await;
    assert_eq!(list_dir(&folder), [INCOMING_DIR.to_owned(), top]);
    assert!(list_dir(&folder.join(INCOMING_DIR)).is_empty(), "incoming store left behind");
}
