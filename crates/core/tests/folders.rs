//! Folders and several files in one Transfer: the Receiver gets the same tree, with empty
//! folders, modification times and the executable bit; symlinks are left behind and counted;
//! and a manifest that is malformed, or that the Collection does not match, is turned away.
//! The hostile peers here are written by hand and speak the control protocol raw.
//! The executable bit and symlinks are Unix things; on Windows the bit is never kept and the
//! symlink test makes its links only if the account may.

mod support;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use bhayanakshare_core::{
    Error, INCOMING_DIR, TransferState,
    manifest::{Entry, MAX_ENTRIES, Manifest, ManifestError},
    protocol::{self, FrameError, Hello, MAX_FRAME_LEN, Message, Offer, read_frame, spawn_reader, write_frame},
};
use iroh::{
    Endpoint,
    endpoint::{Connection, RecvStream, SendStream},
    protocol::Router,
};
use iroh_blobs::{BlobsProtocol, Hash, format::collection::Collection, store::mem::MemStore};
use support::{TestDevice, dial_addr, list_dir, pseudo_random_bytes, raw_peer};

/// The nanoseconds of the times these tests set. NTFS keeps times to 100 ns, so on Windows the
/// last two digits cannot be kept.
const NANOS: u32 = if cfg!(windows) { 123_456_700 } else { 123_456_789 };

/// The reason a Sender is shown when the Receiver turns its Offer away.
const INVALID_NAMES: &str = "Couldn't be sent: invalid file names";

#[derive(Debug, PartialEq)]
enum Node {
    Dir,
    File { bytes: Vec<u8>, mtime: SystemTime, executable: bool },
}

#[cfg(unix)]
fn is_executable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

/// Windows has no executable bit, and a Receiver there never keeps one.
#[cfg(not(unix))]
fn is_executable(_: &std::fs::Metadata) -> bool {
    false
}

/// Everything under `root` by relative path, not following symlinks (which are not sent, so
/// they are not part of what a copy has to match).
fn snapshot(root: &Path) -> BTreeMap<String, Node> {
    fn walk(dir: &Path, rel: &str, out: &mut BTreeMap<String, Node>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            let name = format!("{rel}{}", entry.file_name().to_string_lossy());
            if kind.is_symlink() {
                continue;
            } else if kind.is_dir() {
                out.insert(name.clone(), Node::Dir);
                walk(&entry.path(), &format!("{name}/"), out);
            } else {
                let meta = entry.metadata().unwrap();
                out.insert(
                    name,
                    Node::File {
                        bytes: std::fs::read(entry.path()).unwrap(),
                        mtime: meta.modified().unwrap(),
                        executable: is_executable(&meta),
                    },
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, "", &mut out);
    out
}

/// Makes a symlink at `link` to `target`. On Windows that takes a privilege (administrator, or
/// developer mode), so it can fail where the account does not have it.
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    return if target.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    };
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn set_mtime(path: &Path, secs: u64) {
    let at = UNIX_EPOCH + Duration::new(secs, NANOS);
    std::fs::OpenOptions::new().write(true).open(path).unwrap().set_modified(at).unwrap();
}

/// `album/`: nested and empty folders, a few kinds of file (empty, small, big enough to live
/// on disk in the store, the same big one twice, an executable), each with its own time.
fn make_album(parent: &Path) -> PathBuf {
    let album = parent.join("album");
    let big = pseudo_random_bytes(300 * 1024, 5);
    write(&album.join("a.txt"), b"alpha");
    write(&album.join("big.bin"), &big);
    write(&album.join("copy of big.bin"), &big);
    write(&album.join("sub/deep/c.txt"), b"ccc");
    write(&album.join("bin/run.sh"), b"#!/bin/sh\necho hi\n");
    write(&album.join("zero"), b"");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(album.join("bin/run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::create_dir_all(album.join("empty")).unwrap();
    std::fs::create_dir_all(album.join("sub/also empty")).unwrap();
    for (i, file) in ["a.txt", "big.bin", "copy of big.bin", "sub/deep/c.txt", "bin/run.sh", "zero"]
        .into_iter()
        .enumerate()
    {
        set_mtime(&album.join(file), 1_600_000_000 + 1_000 * i as u64);
    }
    album
}

/// Sends `paths` from alice to bob, bob accepts, both complete. Returns the Transfer.
async fn send_and_accept(
    alice: &mut TestDevice,
    bob: &mut TestDevice,
    paths: &[PathBuf],
) -> bhayanakshare_core::TransferId {
    let id = alice.device.send(bob.addr(), paths).await.unwrap();
    bob.wait_offer().await;
    bob.device.accept(id).await.unwrap();
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;
    id
}

// ---- The round trip -------------------------------------------------------------------

#[tokio::test]
async fn a_folder_round_trips_with_nested_and_empty_folders_times_and_the_executable_bit() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let src = tempfile::tempdir().unwrap();
    let album = make_album(src.path());

    let id = alice.device.send_file(bob.addr(), &album).await.unwrap();
    // The Offer sheet's facts: the top-level items, the file count and the total size.
    let offer = bob.wait_offer().await;
    assert_eq!(offer.items, ["album"]);
    assert_eq!(offer.name, "album");
    assert_eq!(offer.file_count, 6);
    assert_eq!(offer.size, 5 + 2 * 300 * 1024 + 3 + 18);
    assert_eq!(offer.skipped_links, 0);
    bob.device.accept(id).await.unwrap();
    bob.wait_state(id, "completed").await;
    let sent = alice.wait_state(id, "completed").await;
    assert_eq!((sent.items, sent.file_count), (vec!["album".to_owned()], 6));

    let received = bob.save_dir.join("album");
    let tree = snapshot(&received);
    assert_eq!(tree, snapshot(&album));
    // The facts that matter are really there, not just equal to each other.
    assert_eq!(tree["empty"], Node::Dir);
    assert_eq!(tree["sub/also empty"], Node::Dir);
    let Node::File { mtime, executable, .. } = &tree["bin/run.sh"] else { panic!() };
    assert_eq!(*executable, cfg!(unix));
    assert_eq!(*mtime, UNIX_EPOCH + Duration::new(1_600_000_000 + 4_000, NANOS));
    let Node::File { executable, .. } = &tree["a.txt"] else { panic!() };
    assert!(!executable);

    // The Receiver's Completed event says where it went.
    let record = &bob.device.transfers().await.unwrap()[0];
    assert_eq!(record.items, ["album"]);
    assert_eq!(record.state, TransferState::Completed { saved_to: Some(received.to_string_lossy().into_owned()) });
    assert_eq!(bob.history(id), ["offered", "accepted", "transferring", "saving", "completed"]);
    alice.shutdown().await;
    bob.shutdown().await;
    assert_eq!(list_dir(&bob.save_dir), [INCOMING_DIR, "album"]);
    assert!(list_dir(&bob.save_dir.join(INCOMING_DIR)).is_empty(), "incoming store left behind");
}

#[tokio::test]
async fn a_folder_of_hundreds_of_small_files_arrives_whole() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let src = tempfile::tempdir().unwrap();
    let many = src.path().join("many");
    for i in 0..600u32 {
        // Some contents repeat, as they do in real folders; the sizes vary around the point
        // where iroh-blobs keeps data inline.
        let bytes = pseudo_random_bytes(((i % 97) * 400) as usize, u64::from(i % 400));
        write(&many.join(format!("d{}/e{}/f{i}.dat", i % 30, i % 7)), &bytes);
    }

    let began = std::time::Instant::now();
    let id = alice.device.send(bob.addr(), &[many.clone()]).await.unwrap();
    bob.wait_offer().await;
    bob.device.accept(id).await.unwrap();
    bob.wait_state_big(id, "completed").await;
    alice.wait_state_big(id, "completed").await;
    eprintln!("600 files sent and received in {:?}", began.elapsed());

    assert_eq!(snapshot(&bob.save_dir.join("many")), snapshot(&many));
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn several_files_and_folders_arrive_each_under_its_own_name() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let src = tempfile::tempdir().unwrap();
    write(&src.path().join("one.txt"), b"1");
    write(&src.path().join("docs/two.txt"), b"22");
    std::fs::create_dir_all(src.path().join("nothing")).unwrap();
    let paths = [src.path().join("one.txt"), src.path().join("docs"), src.path().join("nothing")];

    let id = alice.device.send(bob.addr(), &paths).await.unwrap();
    let offer = bob.wait_offer().await;
    assert_eq!(offer.items, ["docs", "nothing", "one.txt"]);
    assert_eq!((offer.name.as_str(), offer.file_count, offer.size), ("docs", 2, 3));
    bob.device.accept(id).await.unwrap();
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;

    assert_eq!(std::fs::read(bob.save_dir.join("one.txt")).unwrap(), b"1");
    assert_eq!(std::fs::read(bob.save_dir.join("docs/two.txt")).unwrap(), b"22");
    assert!(bob.save_dir.join("nothing").is_dir());
    // With several items there is no one place to show: it is the save folder.
    let record = &bob.device.transfers().await.unwrap()[0];
    let saved = bob.save_dir.to_string_lossy().into_owned();
    assert_eq!(record.state, TransferState::Completed { saved_to: Some(saved) });
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn a_folder_that_is_only_an_empty_folder_arrives() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let src = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(src.path().join("hollow")).unwrap();

    send_and_accept(&mut alice, &mut bob, &[src.path().join("hollow")]).await;

    assert!(bob.save_dir.join("hollow").is_dir());
    assert!(list_dir(&bob.save_dir.join("hollow")).is_empty());
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn a_folder_is_never_merged_into_or_over_one_that_is_already_there() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let src = tempfile::tempdir().unwrap();
    let album = make_album(src.path());
    write(&bob.save_dir.join("album/mine.txt"), b"mine");
    write(&bob.save_dir.join("a.txt"), b"mine too");

    send_and_accept(&mut alice, &mut bob, &[album.clone(), album.join("a.txt")]).await;

    // Each top-level item is renamed as a unit; what was there is untouched.
    assert_eq!(list_dir(&bob.save_dir.join("album")), ["mine.txt"]);
    assert_eq!(std::fs::read(bob.save_dir.join("a.txt")).unwrap(), b"mine too");
    assert_eq!(snapshot(&bob.save_dir.join("album (1)")), snapshot(&album));
    assert_eq!(std::fs::read(bob.save_dir.join("a (1).txt")).unwrap(), b"alpha");
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn symlinks_are_skipped_and_both_sides_see_how_many() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let src = tempfile::tempdir().unwrap();
    write(&src.path().join("outside/secret.txt"), b"not for sending");
    let pack = src.path().join("pack");
    write(&pack.join("real.txt"), b"real");
    let links = [
        symlink(&src.path().join("outside/secret.txt"), &pack.join("link")),
        symlink(&src.path().join("outside"), &pack.join("dirlink")),
        symlink(&pack, &pack.join("loop")),
    ];
    if let Some(Err(e)) = links.iter().find(|l| l.is_err()) {
        eprintln!("SKIPPED: this account cannot make symlinks ({e}). Symlinks were NOT tested.");
        return;
    }

    let id = alice.device.send_file(bob.addr(), &pack).await.unwrap();

    let sending = alice.wait_state(id, "offered").await;
    assert_eq!(sending.skipped_links, 3);
    let offer = bob.wait_offer().await;
    assert_eq!((offer.skipped_links, offer.file_count), (3, 1));
    bob.device.accept(id).await.unwrap();
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;
    assert_eq!(list_dir(&bob.save_dir.join("pack")), ["real.txt"]);
    let record = &alice.device.transfers().await.unwrap()[0];
    assert_eq!(record.skipped_links, 3);
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn auto_accept_takes_a_folder_without_a_prompt() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    bob.device.add_contact(alice.device.device_id(), None).await.unwrap();
    bob.device.set_auto_accept(alice.device.device_id(), true).await.unwrap();
    let src = tempfile::tempdir().unwrap();
    let album = make_album(src.path());

    let id = alice.device.send_file(bob.addr(), &album).await.unwrap();
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;

    assert_eq!(bob.history(id), ["accepted", "transferring", "saving", "completed"]);
    assert_eq!(snapshot(&bob.save_dir.join("album")), snapshot(&album));
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn the_free_space_check_counts_the_whole_folder() {
    let mut alice = TestDevice::start("alice").await;
    // Room for any one file in the album, but not for all of it.
    let mut bob = TestDevice::start_with_free_space("bob", |_: &Path| Ok(400 * 1024)).await;
    let src = tempfile::tempdir().unwrap();
    let album = make_album(src.path());

    let id = alice.device.send_file(bob.addr(), &album).await.unwrap();
    bob.wait_offer().await;

    let check = bob.device.check_offer(id, None).await.unwrap();
    assert!(!check.fits());
    assert!(matches!(bob.device.accept(id).await, Err(Error::NotEnoughSpace { .. })));
    bob.device.decline(id).await.unwrap();
    alice.wait_state(id, "declined").await;
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn a_cancelled_folder_leaves_nothing_in_the_save_folder() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let src = tempfile::tempdir().unwrap();
    let album = make_album(src.path());

    let id = alice.device.send_file(bob.addr(), &album).await.unwrap();
    bob.wait_offer().await;
    bob.device.accept(id).await.unwrap();
    // Cancelled as soon as it is accepted, wherever the fetch has got to by then.
    bob.wait_state(id, "accepted").await;
    let _ = bob.device.cancel(id).await;

    let ended = bob.wait_for("the end", |t| t.transfer_id == id && t.state.is_terminal()).await;
    alice.shutdown().await;
    bob.shutdown().await;
    // Either it was stopped in time, or it had already been saved whole: never half of it.
    match ended.state {
        TransferState::Cancelled { .. } => {
            let left = list_dir(&bob.save_dir);
            assert!(left.is_empty() || left == [INCOMING_DIR], "{left:?}");
        }
        TransferState::Completed { .. } => assert_eq!(snapshot(&bob.save_dir.join("album")), snapshot(&album)),
        other => panic!("{other:?}"),
    }
    let incoming = bob.save_dir.join(INCOMING_DIR);
    assert!(!incoming.exists() || list_dir(&incoming).is_empty());
}

// ---- What the Sender sends and refuses -----------------------------------------------

/// A Receiver written by hand that reads the Offer a real Sender makes and says nothing.
async fn offer_seen_by_a_raw_receiver(alice: &TestDevice, paths: &[PathBuf]) -> (Offer, TransferHalf) {
    let endpoint = raw_peer().await;
    endpoint.set_alpns(vec![protocol::ALPN.to_vec()]);
    let device_id = data_encoding::BASE32_NOPAD.encode(endpoint.id().as_bytes());
    let to = bhayanakshare_core::DeviceAddr {
        id: device_id.parse().unwrap(),
        direct: endpoint.bound_sockets(),
        relay_url: None,
    };
    let id = alice.device.send(to, paths).await.unwrap();
    let conn = endpoint.accept().await.expect("Alice dials").await.unwrap();
    let (mut send, recv) = conn.accept_bi().await.unwrap();
    let mut incoming = spawn_reader(recv);
    assert!(matches!(incoming.recv().await.unwrap().unwrap(), Message::Hello(_)));
    write_frame(&mut send, &Message::Hello(Hello::current())).await.unwrap();
    let Message::Offer(offer) = incoming.recv().await.unwrap().unwrap() else { panic!("expected an Offer") };
    assert_eq!(offer.transfer_id, *id.as_bytes());
    (offer, TransferHalf { id, send, _endpoint: endpoint, _conn: conn })
}

struct TransferHalf {
    id: bhayanakshare_core::TransferId,
    send: SendStream,
    _endpoint: Endpoint,
    _conn: Connection,
}

#[tokio::test]
async fn the_offer_carries_a_manifest_of_paths_sizes_times_exec_bits_and_empty_folders() {
    let mut alice = TestDevice::start("alice").await;
    let src = tempfile::tempdir().unwrap();
    let album = make_album(src.path());
    // Where symlinks cannot be made (Windows without the privilege) there is nothing to skip.
    let skipped = u32::from(symlink(&album.join("a.txt"), &album.join("link")).is_ok());

    let (offer, _half) = offer_seen_by_a_raw_receiver(&alice, &[album]).await;

    let file = |path: &str, size: u64, secs: u64, executable: bool| Entry::File {
        path: path.into(),
        size,
        mtime_ns: ((1_600_000_000 + secs) * 1_000_000_000 + u64::from(NANOS)) as i64,
        executable,
    };
    let big = 300 * 1024;
    // Sorted by path: the same every time, whatever order the disk lists in.
    assert_eq!(
        support::manifest_of(&offer).entries.as_slice(),
        [
            file("album/a.txt", 5, 0, false),
            file("album/big.bin", big, 1_000, false),
            file("album/bin/run.sh", 18, 4_000, cfg!(unix)),
            file("album/copy of big.bin", big, 2_000, false),
            Entry::empty_dir("album/empty"),
            Entry::empty_dir("album/sub/also empty"),
            file("album/sub/deep/c.txt", 3, 3_000, false),
            file("album/zero", 0, 5_000, false),
        ]
    );
    assert_eq!((offer.size, offer.file_count, offer.skipped_links), (5 + 2 * big + 3 + 18, 6, skipped));
    assert_eq!(offer.validate(), Ok(()));
    alice.shutdown().await;
}

#[tokio::test]
async fn a_receiver_that_turns_the_offer_away_as_malformed_shows_the_sender_why() {
    let mut alice = TestDevice::start("alice").await;
    let src = tempfile::tempdir().unwrap();
    write(&src.path().join("a.txt"), b"a");

    let (_offer, mut half) = offer_seen_by_a_raw_receiver(&alice, &[src.path().join("a.txt")]).await;
    write_frame(&mut half.send, &Message::InvalidOffer).await.unwrap();

    let failed = alice.wait_state(half.id, "failed").await;
    assert_eq!(failed.state, TransferState::Failed { reason: INVALID_NAMES.into() });
    alice.shutdown().await;
}

#[tokio::test]
async fn selections_that_cannot_be_sent_are_refused_before_anything_is_sent() {
    let mut alice = TestDevice::start("alice").await;
    let bob = TestDevice::start("bob").await;
    let src = tempfile::tempdir().unwrap();
    write(&src.path().join("ok.txt"), b"ok");

    // Names a Receiver would refuse (a backslash and a newline are legal on Linux).
    let refused = ["back\\slash", "new\nline"];
    if cfg!(windows) {
        eprintln!("SKIPPED: a file named {refused:?} cannot be made on Windows, so refusing it was NOT tested.");
    } else {
        for name in refused {
            write(&src.path().join("bad").join(name), b"x");
            let result = alice.device.send(bob.addr(), &[src.path().join("bad")]).await;
            assert!(matches!(result, Err(Error::Manifest(ManifestError::InvalidName(_)))), "{name:?}: {result:?}");
            std::fs::remove_file(src.path().join("bad").join(name)).unwrap();
        }
    }
    // Nothing chosen, something missing, and two items with one name.
    assert!(matches!(alice.device.send(bob.addr(), &[]).await, Err(Error::Manifest(ManifestError::Empty))));
    let missing = alice.device.send(bob.addr(), &[src.path().join("ok.txt"), src.path().join("missing")]).await;
    assert!(matches!(missing, Err(Error::NotAFile(_))));
    write(&src.path().join("other/ok.txt"), b"other");
    let twins = alice.device.send(bob.addr(), &[src.path().join("ok.txt"), src.path().join("other/ok.txt")]).await;
    assert!(matches!(twins, Err(Error::Manifest(ManifestError::Duplicate(_)))));

    // None of it became a Transfer, here or there.
    assert!(alice.device.transfers().await.unwrap().is_empty());
    alice.shutdown().await;
    assert!(alice.log.is_empty(), "{:?}", alice.log);
}

// ---- What the Receiver refuses --------------------------------------------------------

/// A raw peer that has said Hello to `bob`, ready to send whatever bytes it likes next.
async fn raw_hello(bob: &TestDevice, peer: &Endpoint) -> (Connection, SendStream, RecvStream) {
    let conn = peer.connect(dial_addr(bob), protocol::ALPN).await.unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, &Message::Hello(Hello::current())).await.unwrap();
    assert!(matches!(read_frame(&mut recv).await.unwrap(), Message::Hello(_)));
    (conn, send, recv)
}

/// Sends `bytes` to `bob` as the first thing after Hello and returns what Bob answers, then
/// hangs up.
async fn raw_offer(bob: &TestDevice, peer: &Endpoint, bytes: Vec<u8>) -> Result<Message, FrameError> {
    let (conn, mut send, mut recv) = raw_hello(bob, peer).await;
    send.write_all(&bytes).await.unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(20), read_frame(&mut recv)).await;
    conn.close(0u32.into(), b"done");
    reply.expect("Bob answers")
}

/// A frame holding `msg`, as `write_frame` would write it.
fn frame(msg: &Message) -> Vec<u8> {
    let body = postcard::to_stdvec(msg).unwrap();
    [(body.len() as u32).to_be_bytes().to_vec(), body].concat()
}

fn files(entries: &[Entry]) -> Manifest {
    Manifest { entries: entries.to_vec() }
}

#[tokio::test]
async fn malformed_manifests_are_turned_away_unseen() {
    let mut bob = TestDevice::start("bob").await;
    let peer = raw_peer().await;
    let f = |path: &str| Entry::file(path, 1);
    let long_path = vec!["p".repeat(255); 17].join("/");
    let many: Vec<Entry> = (0..=MAX_ENTRIES).map(|i| Entry::file(i.to_string(), 0)).collect();
    let good = Offer::new([1; 16], files(&[f("a"), f("b")]), 0);

    let hostile: Vec<(&str, Offer)> = vec![
        ("absolute path", Offer::new([2; 16], files(&[f("/etc/passwd")]), 0)),
        ("windows absolute path", Offer::new([2; 16], files(&[f("\\windows\\system32")]), 0)),
        ("drive and backslashes", Offer::new([2; 16], files(&[f("C:\\Users\\x")]), 0)),
        ("parent first", Offer::new([2; 16], files(&[f("../evil.txt")]), 0)),
        ("parent in the middle", Offer::new([2; 16], files(&[f("a/../../evil.txt")]), 0)),
        ("current dir", Offer::new([2; 16], files(&[f("a/./b")]), 0)),
        ("a dot", Offer::new([2; 16], files(&[f(".")]), 0)),
        ("empty segment", Offer::new([2; 16], files(&[f("a//b")]), 0)),
        ("trailing slash", Offer::new([2; 16], files(&[f("a/")]), 0)),
        ("empty path", Offer::new([2; 16], files(&[f("")]), 0)),
        ("NUL", Offer::new([2; 16], files(&[f("a\0b")]), 0)),
        ("control character", Offer::new([2; 16], files(&[f("a\u{1b}[2Jb")]), 0)),
        ("newline", Offer::new([2; 16], files(&[f("a\nb")]), 0)),
        ("backslash separator", Offer::new([2; 16], files(&[f("a\\b")]), 0)),
        ("name of 256 bytes", Offer::new([2; 16], files(&[f(&"n".repeat(256))]), 0)),
        ("path of over 4096 bytes", Offer::new([2; 16], files(&[f(&long_path)]), 0)),
        ("duplicate", Offer::new([2; 16], files(&[f("a"), f("b"), f("a")]), 0)),
        ("file in a file", Offer::new([2; 16], files(&[f("a"), f("a/b")]), 0)),
        ("file in an empty folder", Offer::new([2; 16], files(&[Entry::empty_dir("d"), f("d/x")]), 0)),
        ("no entries", Offer::new([2; 16], files(&[]), 0)),
        ("too many entries", Offer::new([2; 16], Manifest { entries: many }, 0)),
        ("total size that is not the sum", Offer { size: 3, ..good.clone() }),
        ("file count that is not the count", Offer { file_count: 3, ..good.clone() }),
    ];
    for (what, offer) in hostile {
        let reply = raw_offer(&bob, &peer, frame(&Message::Offer(offer))).await;
        assert!(matches!(reply, Ok(Message::InvalidOffer)), "{what}: {reply:?}");
    }

    // A body that is not a manifest at all, and a frame bigger than any Offer may be.
    let garbage = [vec![0, 0, 0, 4, 1], vec![0xff; 3]].concat();
    let reply = raw_offer(&bob, &peer, garbage).await;
    assert!(matches!(reply, Ok(Message::InvalidOffer)), "{reply:?}");
    let too_big = (MAX_FRAME_LEN + 1).to_be_bytes().to_vec();
    let reply = raw_offer(&bob, &peer, too_big).await;
    assert!(matches!(reply, Ok(Message::InvalidOffer)), "{reply:?}");

    // None of it was shown, recorded or written; and Bob is none the worse for it.
    bob.quiet_for(Duration::from_millis(300)).await;
    assert!(bob.log.is_empty(), "an invalid Offer must not surface: {:?}", bob.log);
    assert!(bob.device.transfers().await.unwrap().is_empty());
    assert!(list_dir(&bob.save_dir).is_empty());
    let (_conn, mut send, _recv) = raw_hello(&bob, &peer).await;
    send.write_all(&frame(&Message::Offer(good))).await.unwrap();
    let shown = bob.wait_offer().await;
    assert_eq!(shown.items, ["a", "b"]);
    bob.shutdown().await;
}

// ---- A Sender that does not serve what it offered -------------------------------------

/// A Sender written by hand: it offers `manifest` and then serves `served`, files that may
/// have nothing to do with it. Bob accepts, and fails the Transfer; returns why.
async fn what_bob_makes_of(manifest: Manifest, served: &[(&str, Vec<u8>)]) -> (String, TestDevice) {
    let mut bob = TestDevice::start("bob").await;
    let store = MemStore::new();
    let mut tags = Vec::new();
    let mut entries = Vec::new();
    for (name, bytes) in served {
        let tag = store.blobs().add_bytes(bytes.clone()).temp_tag().await.unwrap();
        entries.push((name.to_string(), tag.hash()));
        tags.push(tag);
    }
    let root: Hash = Collection::from_iter(entries).store(&store).await.unwrap().hash();
    let endpoint = raw_peer().await;
    let _provider = Router::builder(endpoint.clone())
        .accept(iroh_blobs::ALPN, BlobsProtocol::new(&store, None))
        .spawn();

    let conn = endpoint.connect(dial_addr(&bob), protocol::ALPN).await.unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, &Message::Hello(Hello::current())).await.unwrap();
    assert!(matches!(read_frame(&mut recv).await.unwrap(), Message::Hello(_)));
    let id = bhayanakshare_core::TransferId::from_bytes([9; 16]);
    write_frame(&mut send, &Message::Offer(Offer::new(*id.as_bytes(), manifest, 0))).await.unwrap();
    bob.wait_offer().await;
    bob.device.accept(id).await.unwrap();
    assert!(matches!(read_frame(&mut recv).await.unwrap(), Message::Accept));
    write_frame(&mut send, &Message::HashReady { collection_hash: *root.as_bytes() }).await.unwrap();

    let failed = bob.wait_state(id, "failed").await;
    let TransferState::Failed { reason } = failed.state else { unreachable!() };
    bob.shutdown().await;
    drop((tags, conn));
    (reason, bob)
}

#[tokio::test]
async fn a_collection_that_does_not_match_the_manifest_fails_the_transfer_and_saves_nothing() {
    let five = |c: u8| vec![c; 5];
    let one = |name: &str, size| files(&[Entry::file(name, size)]);
    let two = files(&[Entry::file("a.txt", 5), Entry::file("b/c.txt", 5)]);
    let cases: Vec<(&str, Manifest, Vec<(&str, Vec<u8>)>)> = vec![
        ("another name", two.clone(), vec![("a.txt", five(1)), ("b/other.txt", five(2))]),
        ("a hostile name", two.clone(), vec![("a.txt", five(1)), ("../evil", five(2))]),
        ("a file missing", two.clone(), vec![("a.txt", five(1))]),
        ("a file too many", one("a.txt", 5), vec![("a.txt", five(1)), ("extra.txt", five(2))]),
        ("the right files in another order", two.clone(), vec![("b/c.txt", five(2)), ("a.txt", five(1))]),
        ("a file of another size", one("a.txt", 5), vec![("a.txt", vec![7; 20])]),
        ("no files at all", one("a.txt", 5), vec![]),
    ];
    for (what, manifest, served) in cases {
        let (reason, bob) = what_bob_makes_of(manifest, &served).await;
        assert!(reason.contains("different files"), "{what}: {reason}");
        // Nothing reached the save folder, and the incoming store is gone.
        assert_eq!(list_dir(&bob.save_dir), [INCOMING_DIR], "{what}");
        assert!(list_dir(&bob.save_dir.join(INCOMING_DIR)).is_empty(), "{what}");
    }
}
