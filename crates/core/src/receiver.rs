//! The Receiver side of a Transfer.
//!
//! Hello, then the Offer is shown to the user and nothing moves until they accept. After
//! `Accept` and the Sender's `HashReady`, the content is fetched over iroh-blobs into a
//! per-Transfer store under `<save folder>/.bhayanakshare-incoming/<transfer id>/` (on the
//! save folder's filesystem, so saving is a rename), verified, moved into the save folder,
//! fsynced, and only then is `Completed` sent and the incoming store deleted.

use std::{
    collections::hash_map::Entry,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use iroh::{
    EndpointAddr,
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};
use iroh_blobs::{
    Hash, HashAndFormat,
    api::blobs::{ExportMode, ExportOptions},
    format::collection::Collection,
};
use tokio::sync::oneshot;

use crate::{
    device::{Decision, Shared, TransferInfo},
    identity::DeviceId,
    names::{numbered, validate_file_name},
    protocol::{self, Message, spawn_reader, write_frame},
    session::{Failure, LOST, UNEXPECTED, expect_hello, fail},
    store,
    transfer::{Role, TransferId, TransferState},
};

/// Directory inside the save folder that holds in-progress downloads.
pub const INCOMING_DIR: &str = ".bhayanakshare-incoming";

/// How long to wait for the Sender to close the connection after our last frame, so the
/// frame is not cut off by us hanging up first.
const CLOSE_GRACE: Duration = Duration::from_secs(5);

const BAD_OFFER: &str = "The other Device sent an invalid Offer.";
const CANT_FETCH: &str = "Could not download the file from the sending Device.";
const WRONG_FILE: &str = "The sending Device sent a different file than it offered.";
const CANT_SAVE: &str = "Could not save the file to the save folder.";

pub(crate) struct Handler {
    sh: Arc<Shared>,
}

impl Handler {
    pub fn new(sh: Arc<Shared>) -> Self {
        Self { sh }
    }
}

impl std::fmt::Debug for Handler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("receiver::Handler").finish_non_exhaustive()
    }
}

impl ProtocolHandler for Handler {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        // The task owns the connection, so returning here does not close it.
        self.sh.tasks.spawn(run(self.sh.clone(), conn));
        Ok(())
    }
}

async fn run(sh: Arc<Shared>, conn: Connection) {
    // Set once the Offer is accepted for processing, so a failure can be reported against it.
    let mut info = None;
    let outcome = tokio::select! {
        () = sh.cancel.cancelled() => None,
        outcome = flow(&sh, &conn, &mut info) => Some(outcome),
    };
    let Some(info) = info else { return };
    sh.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&info.id);
    if let Some(Err(Failure(reason))) = outcome {
        sh.transition(&info, TransferState::Failed { reason }).await;
    }
}

async fn flow(
    sh: &Arc<Shared>,
    conn: &Connection,
    announced: &mut Option<TransferInfo>,
) -> Result<(), Failure> {
    let peer_endpoint = conn.remote_id();
    let (mut send, recv) = conn.accept_bi().await.map_err(fail(LOST))?;
    let mut incoming = spawn_reader(recv);
    write_frame(&mut send, &Message::Hello(protocol::Hello::current()))
        .await
        .map_err(fail(LOST))?;
    expect_hello(&mut incoming).await?;

    let offer = match incoming.recv().await {
        Some(Ok(Message::Offer(offer))) => offer,
        Some(Ok(_)) => return Err(Failure::with(UNEXPECTED, "expected Offer")),
        Some(Err(e)) => return Err(Failure::with(LOST, e)),
        None => return Err(Failure::with(LOST, "closed before the Offer")),
    };
    validate_file_name(&offer.name).map_err(fail(BAD_OFFER))?;
    let info = TransferInfo {
        id: TransferId::from_bytes(offer.transfer_id),
        role: Role::Receiver,
        peer: DeviceId::from_endpoint_id(peer_endpoint),
        name: offer.name,
        size: offer.size,
    };

    // Register for a decision before announcing, so a command sent the moment the event
    // is seen finds it.
    let (decide, mut decision) = oneshot::channel();
    match sh.pending.lock().unwrap_or_else(|e| e.into_inner()).entry(info.id) {
        Entry::Vacant(slot) => slot.insert(decide),
        Entry::Occupied(_) => return Err(Failure::with(BAD_OFFER, "duplicate Transfer ID")),
    };
    if let Err(e) = sh.begin(&info, TransferState::Offered).await {
        sh.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&info.id);
        return Err(Failure::with("Could not record the Offer.", e));
    }
    *announced = Some(info.clone());

    let mut root: Option<Hash> = None;
    let decision = loop {
        tokio::select! {
            decided = &mut decision => break decided.map_err(|_| Failure::with(LOST, "Offer withdrawn"))?,
            msg = incoming.recv() => match msg {
                Some(Ok(Message::HashReady { collection_hash })) if root.is_none() => {
                    root = Some(collection_hash.into());
                }
                Some(Ok(_)) => return Err(Failure::with(UNEXPECTED, "message while deciding")),
                Some(Err(e)) => return Err(Failure::with(LOST, e)),
                None => return Err(Failure::with(LOST, "stream ended while deciding")),
            },
        }
    };

    if decision == Decision::Decline {
        write_frame(&mut send, &Message::Decline).await.map_err(fail(LOST))?;
        let _ = send.finish();
        sh.transition(&info, TransferState::Declined).await;
        let _ = tokio::time::timeout(CLOSE_GRACE, conn.closed()).await;
        return Ok(());
    }

    write_frame(&mut send, &Message::Accept).await.map_err(fail(LOST))?;
    sh.transition(&info, TransferState::Accepted).await;
    while root.is_none() {
        match incoming.recv().await {
            Some(Ok(Message::HashReady { collection_hash })) => root = Some(collection_hash.into()),
            Some(Ok(_)) => return Err(Failure::with(UNEXPECTED, "message before HashReady")),
            Some(Err(e)) => return Err(Failure::with(LOST, e)),
            None => return Err(Failure::with(LOST, "stream ended before HashReady")),
        }
    }
    let root = root.expect("loop above runs until set");

    sh.transition(&info, TransferState::Transferring).await;
    let dir = sh.save_dir.join(INCOMING_DIR).join(info.id.to_string());
    let store = match store::open(&dir).await {
        Ok(store) => store,
        Err(e) => {
            remove_dir(&dir).await;
            return Err(Failure::with("Could not prepare space to receive the file.", e));
        }
    };

    match fetch_and_save(sh, peer_endpoint, &store, &info, root, &dir).await {
        Ok(saved) => {
            let sent = write_frame(&mut send, &Message::Completed).await;
            let _ = send.finish();
            spawn_cleanup(sh, store, dir);
            sh.transition(&info, TransferState::Completed { saved_to: Some(saved) }).await;
            match sent {
                Ok(()) => {
                    let _ = tokio::time::timeout(CLOSE_GRACE, conn.closed()).await;
                }
                // The file is safely saved either way.
                Err(e) => tracing::warn!("could not tell the Sender the file arrived: {e}"),
            }
            Ok(())
        }
        Err(failure) => {
            spawn_cleanup(sh, store, dir);
            Err(failure)
        }
    }
}

/// Fetches the Transfer's content into `store`, checks it is what was offered, and moves it
/// into the save folder. Returns where it ended up.
async fn fetch_and_save(
    sh: &Arc<Shared>,
    sender: iroh::EndpointId,
    store: &store::Store,
    info: &TransferInfo,
    root: Hash,
    dir: &Path,
) -> Result<String, Failure> {
    let blobs: &iroh_blobs::api::Store = store;
    let content = HashAndFormat::hash_seq(root);

    let conn = sh
        .endpoint
        .connect(EndpointAddr::new(sender), iroh_blobs::ALPN)
        .await
        .map_err(fail(CANT_FETCH))?;
    // iroh-blobs checks every chunk against its BLAKE3 hash as it arrives.
    blobs.remote().fetch(conn.clone(), content).await.map_err(fail(CANT_FETCH))?;
    conn.close(0u32.into(), b"done");
    if !blobs.remote().local(content).await.map_err(fail(CANT_FETCH))?.is_complete() {
        return Err(Failure::with(CANT_FETCH, "fetch ended incomplete"));
    }

    // The Collection must be exactly the one file that was offered.
    let collection = Collection::load(root, blobs).await.map_err(fail(WRONG_FILE))?;
    let [(name, file)] = collection.iter().cloned().collect::<Vec<_>>().try_into().map_err(
        |entries: Vec<_>| Failure::with(WRONG_FILE, format!("{} entries", entries.len())),
    )?;
    if name != info.name {
        return Err(Failure::with(WRONG_FILE, "name differs from the Offer"));
    }
    let size = blobs.blobs().observe(file).await.map_err(fail(WRONG_FILE))?.size();
    if size != info.size {
        return Err(Failure::with(WRONG_FILE, format!("{size} bytes, offered {}", info.size)));
    }

    sh.transition(info, TransferState::Saving).await;
    let staged = dir.join("out").join(&info.name);
    blobs
        .blobs()
        .export_with_opts(ExportOptions {
            hash: file,
            // The store keeps the data in its own file, so this is a rename, not a copy.
            mode: ExportMode::TryReference,
            target: staged.clone(),
        })
        .finish()
        .await
        .map_err(fail(CANT_SAVE))?;

    let save_dir = sh.save_dir.clone();
    let name = info.name.clone();
    let saved = tokio::task::spawn_blocking(move || move_into_save_folder(&staged, &save_dir, &name))
        .await
        .map_err(fail(CANT_SAVE))?
        .map_err(fail(CANT_SAVE))?;
    Ok(saved.to_string_lossy().into_owned())
}

/// Serialises the check-then-rename below across the process, so two Transfers of the same
/// name cannot both pick the same free spot.
static SAVE_LOCK: Mutex<()> = Mutex::new(());

/// Makes the staged file durable, renames it into `save_dir` under a name that is not taken
/// (`a.txt`, `a (1).txt`, ...; an existing file is never replaced), and fsyncs the folder so
/// the rename survives a power cut.
fn move_into_save_folder(staged: &Path, save_dir: &Path, name: &str) -> io::Result<PathBuf> {
    std::fs::OpenOptions::new().write(true).open(staged)?.sync_all()?;
    let _lock = SAVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for n in 0u32.. {
        let candidate = if n == 0 { name.to_owned() } else { numbered(name, n) };
        let dest = save_dir.join(candidate);
        if dest.symlink_metadata().is_ok() {
            continue;
        }
        std::fs::rename(staged, &dest)?;
        sync_dir(save_dir)?;
        return Ok(dest);
    }
    unreachable!("the loop above only ends by returning")
}

#[cfg(unix)]
fn sync_dir(dir: &Path) -> io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}

/// Directories cannot be opened for syncing on Windows; NTFS journals the rename.
#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

/// Shuts the Transfer's store down and deletes its directory. Runs as its own tracked task,
/// so a Device shutdown waits for it instead of cancelling it.
fn spawn_cleanup(sh: &Shared, store: store::Store, dir: PathBuf) {
    sh.tasks.spawn(async move {
        if let Err(e) = store.shutdown().await {
            tracing::warn!("closing incoming store: {e}");
        }
        remove_dir(&dir).await;
    });
}

async fn remove_dir(dir: &Path) {
    if let Err(e) = tokio::fs::remove_dir_all(dir).await {
        if e.kind() != io::ErrorKind::NotFound {
            tracing::warn!("removing {}: {e}", dir.display());
        }
    }
}
