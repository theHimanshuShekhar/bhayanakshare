//! The Receiver side of a Transfer.
//!
//! Hello, then the Offer is shown to the user and nothing moves until they accept. After
//! `Accept` and the Sender's `HashReady`, the content is fetched over iroh-blobs into a
//! per-Transfer store under `<save folder>/.bhayanakshare-incoming/<transfer id>/` (on the
//! save folder's filesystem, so saving is a rename), verified against the Offer's manifest,
//! built into a staging tree next to the store, and only when all of it is right is each
//! top-level item moved into the save folder, fsynced, and only then is `Completed` sent and the
//! incoming store deleted. The names are the Sender's, made safe to write on any system (spec
//! section 6) before the tree is built; an item whose name is taken in the save folder arrives
//! under a numbered one instead. The save folder is the one the Offer was accepted into, which the
//! Receiver may choose per Offer.
//!
//! Text that came in the Offer is the exception: there is nothing to fetch and no incoming
//! store, and nothing goes to the save folder. Accepting it keeps the text in the database
//! (which is what Transfer History shows) and completes the Transfer, and only then is the
//! Sender told, `Accept` and `Completed` together. A text Offer is never resumed: there is no
//! content hash to resume from, so a restart before it is answered lets it expire.
//!
//! Until the move into the save folder starts, either side can cancel (the incoming store is
//! deleted), and an Offer nobody answers expires. A Sender that already has 5 Offers waiting
//! gets `Busy` instead of a new one.
//!
//! The Receiver drives resume. Once it has accepted, a lost connection (or a restart of
//! either Device) does not end the Transfer: it shows Reconnecting, dials the Sender again
//! with backoff and says `Resume`, and fetches again when the Sender answers `ResumeOk` and
//! `HashReady`. There is one fetch per Transfer, with no chunking: iroh-blobs keeps the
//! verified part of the incoming store, re-hashing it after a crash, and asks only for what
//! is missing. With no progress for 24 hours the Receiver gives up and deletes it.

use std::{
    collections::{BTreeSet, hash_map::Entry},
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use iroh::{
    EndpointAddr,
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};
use iroh_blobs::{
    Hash, HashAndFormat,
    api::{
        blobs::{ExportMode, ExportOptions},
        remote::GetProgressItem,
    },
    format::collection::Collection,
    get::GetError,
};
use n0_future::StreamExt;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::{
    clock::{UnixMillis, sleep_until},
    device::{Decision, PendingOffer, Shared, TransferInfo},
    fsmove::rename_no_replace,
    identity::DeviceId,
    manifest::{self, Manifest},
    names::{adjust_names, numbered},
    protocol::{self, FrameError, Message, OfferKind, spawn_reader, write_frame},
    session::{
        CLOSE_GRACE, FORGOTTEN, Failure, LOST, STALLED, Session, Stall, Stop, UNEXPECTED,
        expect_hello, fail, made_progress, retry_delay, save_progress, stop,
    },
    store,
    transfer::{OFFER_TTL_MS, Role, TransferId, TransferKind, TransferState},
};

/// Directory inside the save folder that holds in-progress downloads.
pub const INCOMING_DIR: &str = ".bhayanakshare-incoming";

/// Where, inside a Transfer's incoming directory, its tree is built before it is moved.
const OUT_DIR: &str = "out";

/// How many Offers one Sender may have waiting for an answer before the next gets `Busy`.
const MAX_PENDING_OFFERS: usize = 5;

/// The least time, by the Device's clock, between two progress reports while fetching.
const PROGRESS_INTERVAL_MS: UnixMillis = 100;

/// Bytes a Transfer may download beyond the files themselves, before the Collection's own
/// size: slack for its header and the framing.
const COLLECTION_SLACK: u64 = 4096;

/// What the Collection itself, which is fetched along with the files, may add to what the
/// Offer says: a hash and a name for each file, however the names are encoded.
fn collection_allowance(manifest: &Manifest) -> u64 {
    // 32 bytes of hash and a few of length per file, for one more than the files (the names).
    const PER_ENTRY: u64 = 37;
    let names: u64 = manifest.files().map(|(path, _)| path.len() as u64).sum();
    COLLECTION_SLACK + PER_ENTRY * (manifest.file_count() + 1) + names
}

/// The longest path the platform's filesystems take, in bytes, counting everything from the
/// root. Linux's `PATH_MAX` and macOS's include the terminating NUL. Windows is not limited to
/// this (the app uses `\\?\` paths there, untested) but is held to it too, for want of a
/// better number, since an Offer's own paths are far shorter.
const MAX_PATH: usize = if cfg!(target_os = "linux") { 4095 } else { 1023 };

/// How long one try to reach the Sender may take before the next is made.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a Receiver that cancelled while disconnected tries to tell the Sender.
const TELL_TIMEOUT: Duration = Duration::from_secs(5);

const BAD_OFFER: &str = "The other Device sent an invalid Offer.";
const CANT_FETCH: &str = "Could not download the files from the sending Device.";
const WRONG_FILE: &str = "The sending Device sent different files than it offered.";
const TOO_MUCH: &str = "The sending Device sent more than it offered.";
const CANT_SAVE: &str = "Could not save the files to the save folder.";
const CANT_KEEP_TEXT: &str = "Could not keep the text.";

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

/// A Transfer's incoming store, once opened.
struct Opened {
    store: store::Store,
    dir: PathBuf,
}

async fn run(sh: Arc<Shared>, conn: Connection) {
    // Set once the Offer is accepted for processing, so a failure can be reported against it.
    let mut info = None;
    let mut opened = None;
    let outcome = tokio::select! {
        () = sh.cancel.cancelled() => None,
        outcome = flow(&sh, &conn, &mut info, &mut opened) => Some(outcome),
    };
    finish(&sh, info, opened, outcome).await;
}

/// Picks up a Transfer this Device had accepted before it restarted: the Receiver redials
/// the Sender, which still has the Transfer, and fetches what is missing from the incoming
/// store the Transfer left behind.
pub(crate) fn recover(
    sh: &Arc<Shared>,
    info: TransferInfo,
    root: [u8; 32],
    save_dir: PathBuf,
    manifest: Manifest,
    progress_at: UnixMillis,
) {
    let cancel = sh.track(info.id);
    let task_sh = sh.clone();
    sh.tasks.spawn(async move {
        let sh = task_sh;
        let mut opened = None;
        let root = Some(Hash::from_bytes(root));
        let manifest = Arc::new(manifest);
        let outcome = tokio::select! {
            () = sh.cancel.cancelled() => None,
            outcome = settle(
                &sh, &info, &save_dir, &manifest, None, root, None, &cancel, &mut opened,
                Stall::new(progress_at),
            ) => Some(outcome),
        };
        finish(&sh, Some(info), opened, outcome).await;
    });
}

/// Ends the Transfer's task. `outcome` is `None` when the Device is shutting down: the
/// Transfer is not over, so the incoming store is closed cleanly but kept for the next start.
async fn finish(
    sh: &Shared,
    info: Option<TransferInfo>,
    opened: Option<Opened>,
    outcome: Option<Result<(), Failure>>,
) {
    if let Some(opened) = opened {
        close_store(sh, opened, outcome.is_some());
    }
    let Some(info) = info else { return };
    sh.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&info.id);
    sh.untrack(info.id);
    if let Some(Err(Failure(reason))) = outcome {
        sh.transition(&info, TransferState::Failed { reason }).await;
    }
}

async fn flow(
    sh: &Arc<Shared>,
    conn: &Connection,
    announced: &mut Option<TransferInfo>,
    opened: &mut Option<Opened>,
) -> Result<(), Failure> {
    let peer_endpoint = conn.remote_id();
    let (mut send, recv) = conn.accept_bi().await.map_err(fail(LOST))?;
    let mut incoming = spawn_reader(recv);
    write_frame(&mut send, &Message::Hello(protocol::Hello::named(sh.device_name().await)))
        .await
        .map_err(fail(LOST))?;
    let peer_name = expect_hello(&mut incoming).await?;
    sh.remember_peer(DeviceId::from_endpoint_id(peer_endpoint), conn, peer_name.clone()).await;
    let mut session = Session { conn: conn.clone(), send, incoming };

    let offer = match session.incoming.recv().await {
        Some(Ok(Message::Offer(offer))) => offer,
        // Not an Offer: the Receiver of one of this Device's Transfers dialling back.
        Some(Ok(Message::Resume { transfer_id })) => {
            crate::sender::resume(sh, session, TransferId::from_bytes(transfer_id)).await;
            return Ok(());
        }
        Some(Ok(_)) => return Err(Failure::with(UNEXPECTED, "expected Offer")),
        // An Offer that cannot be read, or is bigger than any Offer may be, is malformed.
        Some(Err(e @ (FrameError::Malformed(_) | FrameError::TooLarge(_)))) => {
            tracing::warn!(peer = %DeviceId::from_endpoint_id(peer_endpoint).fingerprint(), "Offer refused: {e}");
            refuse(&mut session, conn).await;
            return Ok(());
        }
        Some(Err(e)) => return Err(Failure::with(LOST, e)),
        None => return Err(Failure::with(LOST, "closed before the Offer")),
    };
    // Nothing in the Offer is trusted until it has passed: it is not shown, recorded or
    // looked at again otherwise, and a Sender whose Offer fails is told so.
    if let Err(e) = offer.validate() {
        tracing::warn!(peer = %DeviceId::from_endpoint_id(peer_endpoint).fingerprint(), "Offer refused: {e}");
        refuse(&mut session, conn).await;
        return Ok(());
    }
    // Files are fetched and saved; text is all here already.
    let (kind, items, adjusted_names, longest_path, manifest, text) = match offer.kind {
        OfferKind::Files(manifest) => {
            let adjusted = adjust_names(&manifest, INCOMING_DIR);
            let longest = adjusted.manifest.entries.iter().map(|entry| entry.path().len()).max().unwrap_or(0);
            let items = manifest.top_level_items();
            (TransferKind::Files, items, adjusted.count, longest, Some(Arc::new(manifest)), None)
        }
        OfferKind::Text(text) => (TransferKind::Text, Vec::new(), 0, 0, None, Some(text)),
    };
    let info = TransferInfo {
        id: TransferId::from_bytes(offer.transfer_id),
        role: Role::Receiver,
        peer: DeviceId::from_endpoint_id(peer_endpoint),
        peer_name,
        kind,
        name: items.first().cloned().unwrap_or_default(),
        size: offer.size,
        text,
        items,
        file_count: offer.file_count,
        skipped_links: offer.skipped_links,
        adjusted_names,
        // The Sender's Batch is its own business; the Offer does not mention it.
        batch: None,
        expires_at: sh.now() + OFFER_TTL_MS,
    };

    let (decide, mut decision) = oneshot::channel();
    let auto = auto_accept_folder(sh, &info, longest_path).await;
    // Register for a decision before announcing, so a command sent the moment the event
    // is seen finds it. The same lock settles whether this Sender already has too many
    // Offers waiting.
    let busy = match &auto {
        // A trusted Contact whose Offer passed the checks: nobody is asked, so it neither waits
        // for an answer nor counts against the Sender's pending Offers.
        Some(folder) => {
            let _ = decide.send(Decision::Accept(folder.clone()));
            false
        }
        None => {
            let mut pending = sh.pending.lock().unwrap_or_else(|e| e.into_inner());
            let waiting = pending.values().filter(|p| p.peer == info.peer).count();
            if waiting >= MAX_PENDING_OFFERS {
                true
            } else {
                match pending.entry(info.id) {
                    Entry::Vacant(slot) => {
                        slot.insert(PendingOffer {
                            peer: info.peer,
                            size: info.size,
                            longest_path,
                            text: info.text.is_some(),
                            decide,
                        })
                    }
                    Entry::Occupied(_) => return Err(Failure::with(BAD_OFFER, "duplicate Transfer ID")),
                };
                false
            }
        }
    };
    if busy {
        // Not shown to the user and not recorded: it is as if the Offer had never come.
        tracing::debug!(peer = %info.peer.fingerprint(), "Offer refused: too many waiting");
        write_frame(&mut session.send, &Message::Busy).await.map_err(fail(LOST))?;
        let _ = session.send.finish();
        let _ = tokio::time::timeout(CLOSE_GRACE, conn.closed()).await;
        return Ok(());
    }
    // An auto-accepted Transfer starts out Accepted, so the Offer sheet never appears.
    let first = if auto.is_some() { TransferState::Accepted } else { TransferState::Offered };
    if let Err(e) = sh.begin(&info, first).await {
        sh.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&info.id);
        return Err(Failure::with("Could not record the Offer.", e));
    }
    *announced = Some(info.clone());
    let cancel = sh.track(info.id);
    let mut expiry = Box::pin(sleep_until(&*sh.clock, info.expires_at));

    let mut root: Option<Hash> = None;
    let decision = loop {
        tokio::select! {
            // The user's answer, then what the Sender has said, count before our own clock
            // and cancel, so both sides end the Transfer the same way.
            biased;
            decided = &mut decision => break decided.map_err(|_| Failure::with(LOST, "Offer withdrawn"))?,
            msg = session.incoming.recv() => match msg {
                // A text Offer has no content to be ready.
                Some(Ok(Message::HashReady { collection_hash })) if root.is_none() && manifest.is_some() => {
                    root = Some(collection_hash.into());
                }
                Some(Ok(Message::Cancel)) => {
                    stop(sh, &info, Some(&mut session), Stop::PeerCancelled).await;
                    return Ok(());
                }
                Some(Ok(Message::Expired)) => {
                    stop(sh, &info, Some(&mut session), Stop::PeerExpired).await;
                    return Ok(());
                }
                Some(Ok(_)) => return Err(Failure::with(UNEXPECTED, "message while deciding")),
                Some(Err(e)) => return Err(Failure::with(LOST, e)),
                None => return Err(Failure::with(LOST, "stream ended while deciding")),
            },
            () = cancel.cancelled() => {
                stop(sh, &info, Some(&mut session), Stop::Cancelled).await;
                return Ok(());
            }
            () = &mut expiry => {
                stop(sh, &info, Some(&mut session), Stop::Expired).await;
                return Ok(());
            }
        }
    };

    let Decision::Accept(save_dir) = decision else {
        write_frame(&mut session.send, &Message::Decline).await.map_err(fail(LOST))?;
        let _ = session.send.finish();
        sh.transition(&info, TransferState::Declined).await;
        let _ = tokio::time::timeout(CLOSE_GRACE, conn.closed()).await;
        return Ok(());
    };

    let Some(manifest) = manifest else {
        return keep_text(sh, &info, session, auto.is_none()).await;
    };
    // Saved before the Sender can learn of the yes, so a restart finds where to resume.
    let now = sh.now();
    let known = root.map(|hash| *hash.as_bytes());
    if let Err(e) = sh.db.start_transfer(info.id, known, Some(&save_dir), now).await {
        tracing::warn!(transfer = %info.id, "could not record the save folder, so a restart cannot resume: {e}");
    }
    if let Err(e) = sh.db.insert_manifest(info.id, &manifest).await {
        tracing::warn!(transfer = %info.id, "could not record the manifest, so a restart cannot resume: {e}");
    }
    write_frame(&mut session.send, &Message::Accept).await.map_err(fail(LOST))?;
    // An auto-accepted Transfer was announced as Accepted when it began.
    if auto.is_none() {
        sh.transition(&info, TransferState::Accepted).await;
    }
    let shown = Some(TransferState::Accepted);
    settle(sh, &info, &save_dir, &manifest, Some(session), root, shown, &cancel, opened, Stall::new(now))
        .await
}

/// Takes a text Offer the Receiver accepted: keeps the text and completes the Transfer, then
/// tells the Sender. Nothing is fetched, and the text goes in the database, not the save folder.
/// From the yes there is nothing left to cancel, as from Saving. `announce` is whether the
/// Transfer has still to be shown Accepted (an auto-accepted one began that way).
async fn keep_text(
    sh: &Arc<Shared>,
    info: &TransferInfo,
    mut session: Session,
    announce: bool,
) -> Result<(), Failure> {
    sh.untrack(info.id);
    if announce {
        sh.transition(info, TransferState::Accepted).await;
    }
    // Before the Sender hears of it: a Sender that is told `Completed` has been told the truth.
    sh.complete_text(info).await.map_err(fail(CANT_KEEP_TEXT))?;
    let told = async {
        write_frame(&mut session.send, &Message::Accept).await?;
        write_frame(&mut session.send, &Message::Completed).await
    }
    .await;
    let _ = session.send.finish();
    match told {
        Ok(()) => {
            let _ = tokio::time::timeout(CLOSE_GRACE, session.conn.closed()).await;
        }
        Err(e) => tracing::warn!("the Sender was out of reach, so it was not told the text arrived: {e}"),
    }
    Ok(())
}

/// Turns a malformed Offer away: the Sender is told, and nothing is recorded or shown, as if
/// it had never come.
async fn refuse(session: &mut Session, conn: &Connection) {
    let _ = write_frame(&mut session.send, &Message::InvalidOffer).await;
    let _ = session.send.finish();
    let _ = tokio::time::timeout(CLOSE_GRACE, conn.closed()).await;
}

/// The save folder to accept `info` into without asking, if its Sender is a Contact with
/// Auto-accept on and the Offer passes the Receiver's checks: it fits, and its paths (the longest
/// is `longest_path` bytes) are not too long. Any failed check (or a failure to run one) returns
/// `None`, and the Offer is shown as a normal prompt with its warning. Adjusted names are no
/// reason to ask.
async fn auto_accept_folder(sh: &Shared, info: &TransferInfo, longest_path: usize) -> Option<PathBuf> {
    match sh.db.contact(info.peer).await {
        Ok(Some(contact)) if contact.auto_accept => {}
        Ok(_) => return None,
        Err(e) => {
            tracing::warn!("could not look up the Sender as a Contact: {e}");
            return None;
        }
    }
    if info.text.is_some() {
        // Kept in the database, not the save folder: there is nothing to check.
        return Some(sh.save_dir.clone());
    }
    match sh.space_check(info.id, info.size, longest_path, &sh.save_dir).await {
        Ok(check) if check.passes() => Some(sh.save_dir.clone()),
        Ok(_) => None,
        Err(e) => {
            tracing::warn!("Auto-accept held back, the save folder cannot be checked: {e}");
            None
        }
    }
}

fn incoming_dir(save_dir: &Path, id: TransferId) -> PathBuf {
    save_dir.join(INCOMING_DIR).join(id.to_string())
}

/// Whether the longest path of Transfer `id` (`longest_path` bytes, relative to the save
/// folder) is short enough to write under `save_dir`. The tree is built under the incoming
/// store, which is deeper than where it ends up, so the path there is the one that has to
/// fit; it also leaves room for the number a clashing item is given.
pub(crate) fn paths_fit(save_dir: &Path, id: TransferId, longest_path: usize) -> bool {
    let staged = incoming_dir(save_dir, id).join(OUT_DIR);
    staged.as_os_str().as_encoded_bytes().len() + 1 + longest_path <= MAX_PATH
}

/// How the Transfer ended after the Receiver accepted it, and the connection to the Sender
/// if one is up.
enum Ended {
    /// The file is in the save folder, at this path.
    Saved(String, Option<Session>),
    /// Cancelled before anything was saved.
    Stopped(Stop, Option<Session>),
}

/// Everything after the Receiver said yes: fetch, reconnecting as often as it takes, then
/// tell the Sender and record how it ended. `session` is the connection the Offer arrived
/// on (none after a restart), `root` the content hash if the Sender already sent it, and
/// `shown` the state the Transfer was last announced in on this Device's event stream.
async fn settle(
    sh: &Arc<Shared>,
    info: &TransferInfo,
    save_dir: &Path,
    manifest: &Arc<Manifest>,
    session: Option<Session>,
    root: Option<Hash>,
    shown: Option<TransferState>,
    cancel: &CancellationToken,
    opened: &mut Option<Opened>,
    stall: Stall,
) -> Result<(), Failure> {
    let ended = drive(sh, info, save_dir, manifest, session, root, shown, cancel, opened, stall).await;
    // However it ended, whatever is left in the incoming store is deleted with it: partial
    // data after a failure or cancel, nothing after a save.
    match opened.take() {
        Some(opened) => close_store(sh, opened, true),
        None => remove_dir(&incoming_dir(save_dir, info.id)).await,
    }
    match ended? {
        Ended::Saved(saved, session) => {
            let sent = match session {
                Some(mut session) => {
                    let sent = write_frame(&mut session.send, &Message::Completed).await;
                    let _ = session.send.finish();
                    Some(sent.map(|()| session.conn))
                }
                None => None,
            };
            finish_saved(sh, info, saved, sent).await
        }
        Ended::Stopped(how, mut session) => {
            stop(sh, info, session.as_mut(), how).await;
            if session.is_none() && matches!(how, Stop::Cancelled) {
                // Cancelled while the Sender was out of reach, so it was not told: one quick
                // try, after the user has seen it cancelled, to tell it now.
                if let Ok(Resumed::Session(mut live)) =
                    tokio::time::timeout(TELL_TIMEOUT, try_resume(sh, info)).await
                {
                    let _ = write_frame(&mut live.send, &Message::Cancel).await;
                    let _ = live.send.finish();
                    let _ = tokio::time::timeout(CLOSE_GRACE, live.conn.closed()).await;
                }
            }
            Ok(())
        }
    }
}

/// Records the file as received. `sent` is the outcome of telling the Sender, if it could
/// be told; the file is safely saved either way.
async fn finish_saved(
    sh: &Shared,
    info: &TransferInfo,
    saved: String,
    sent: Option<Result<Connection, protocol::FrameError>>,
) -> Result<(), Failure> {
    sh.transition(info, TransferState::Completed { saved_to: Some(saved) }).await;
    match sent {
        Some(Ok(conn)) => {
            let _ = tokio::time::timeout(CLOSE_GRACE, conn.closed()).await;
        }
        Some(Err(e)) => tracing::warn!("could not tell the Sender the file arrived: {e}"),
        None => tracing::warn!("the Sender was out of reach, so it was not told the file arrived"),
    }
    Ok(())
}

/// Fetches the content, over a new connection whenever the last one is lost.
async fn drive(
    sh: &Arc<Shared>,
    info: &TransferInfo,
    save_dir: &Path,
    manifest: &Arc<Manifest>,
    mut session: Option<Session>,
    mut root: Option<Hash>,
    mut shown: Option<TransferState>,
    cancel: &CancellationToken,
    opened: &mut Option<Opened>,
    mut stall: Stall,
) -> Result<Ended, Failure> {
    // Tries made to reach the Sender since the fetch last made progress; paces the next.
    let mut attempt = 0;
    loop {
        let mut live = match session.take() {
            Some(live) => live,
            None => {
                show(sh, info, &mut shown, TransferState::Reconnecting).await;
                match reconnect(sh, info, cancel, &stall, &mut attempt).await? {
                    Redialled::Session(live) => live,
                    Redialled::Stopped(how) => return Ok(Ended::Stopped(how, None)),
                }
            }
        };
        // The go-ahead to fetch is the Sender's `HashReady`, which may already have arrived.
        // After a reconnect the hash is the one known already and `ResumeOk` is the go-ahead.
        let hash = match root {
            Some(hash) => hash,
            None => match await_hash(&mut live, cancel).await? {
                Awaited::Ready(hash) => {
                    // Saved so a restart can resume.
                    let saved = sh.db.start_transfer(info.id, Some(*hash.as_bytes()), None, sh.now());
                    if let Err(e) = saved.await {
                        tracing::warn!(transfer = %info.id, "could not record the content hash, so a restart cannot resume: {e}");
                    }
                    *root.insert(hash)
                }
                Awaited::Stopped(how) => return Ok(Ended::Stopped(how, Some(live))),
            },
        };
        show(sh, info, &mut shown, TransferState::Transferring).await;
        if opened.is_none() {
            // On the chosen folder's filesystem, so saving is a rename.
            let dir = incoming_dir(save_dir, info.id);
            match store::open(&dir).await {
                Ok(store) => *opened = Some(Opened { store, dir }),
                Err(e) => {
                    remove_dir(&dir).await;
                    return Err(Failure::with("Could not prepare space to receive the file.", e));
                }
            }
        }
        let store = &opened.as_ref().expect("opened above").store;
        let last_progress = stall.last();
        match fetch_and_save(sh, &mut live, cancel, store, info, hash, save_dir, manifest, &mut stall)
            .await?
        {
            Fetched::Saved(saved) => return Ok(Ended::Saved(saved, Some(live))),
            Fetched::Stopped(how) => return Ok(Ended::Stopped(how, Some(live))),
            Fetched::Lost => {
                tracing::debug!(transfer = %info.id, "lost the Sender mid-fetch; reconnecting");
                // The Sender is gone, but it may not know it yet.
                live.conn.close(0u32.into(), b"reconnecting");
                // A connection that got somewhere starts the quick retries over.
                if stall.last() != last_progress {
                    attempt = 0;
                }
                save_progress(sh, info, &mut stall).await;
            }
        }
    }
}

/// Announces a state change unless the Transfer is already shown in that state (`None`:
/// nothing has been announced on this Device's event stream yet).
async fn show(
    sh: &Shared,
    info: &TransferInfo,
    shown: &mut Option<TransferState>,
    state: TransferState,
) {
    if shown.as_ref() != Some(&state) {
        sh.transition(info, state.clone()).await;
        *shown = Some(state);
    }
}

enum Redialled {
    Session(Session),
    Stopped(Stop),
}

/// Dials the Sender and says `Resume` until it answers `ResumeOk`, pacing the tries with
/// [`retry_delay`]. Ends the Transfer if the Sender no longer knows it or has failed or
/// cancelled it, or if 24 hours pass with no progress.
async fn reconnect(
    sh: &Shared,
    info: &TransferInfo,
    cancel: &CancellationToken,
    stall: &Stall,
    attempt: &mut u32,
) -> Result<Redialled, Failure> {
    loop {
        let pause = retry_delay(*attempt);
        *attempt = attempt.saturating_add(1);
        let try_again = async {
            tokio::time::sleep(pause).await;
            try_resume(sh, info).await
        };
        let resumed = tokio::select! {
            biased;
            () = cancel.cancelled() => return Ok(Redialled::Stopped(Stop::Cancelled)),
            () = sleep_until(&*sh.clock, stall.deadline()) => return Err(Failure(STALLED.into())),
            resumed = try_again => resumed,
        };
        match resumed {
            Resumed::Session(live) => return Ok(Redialled::Session(live)),
            Resumed::Cancelled => return Ok(Redialled::Stopped(Stop::PeerCancelled)),
            Resumed::Refused(reason) => return Err(Failure(reason)),
            Resumed::Unreachable => {}
        }
    }
}

enum Resumed {
    /// The Sender answered `ResumeOk`.
    Session(Session),
    /// The Sender cancelled the Transfer.
    Cancelled,
    /// The Sender does not have the Transfer any more, or it failed there: the reason.
    Refused(String),
    /// Could not reach the Sender, or lost it again before it answered. Try again.
    Unreachable,
}

async fn try_resume(sh: &Shared, info: &TransferInfo) -> Resumed {
    let to = EndpointAddr::new(info.peer.endpoint_id());
    let dialled = async {
        let conn = sh.endpoint.connect(to, protocol::ALPN).await.map_err(|e| e.to_string())?;
        let (mut send, recv) = conn.open_bi().await.map_err(|e| e.to_string())?;
        let mut incoming = spawn_reader(recv);
        let hello = Message::Hello(protocol::Hello::named(sh.device_name().await));
        write_frame(&mut send, &hello).await.map_err(|e| e.to_string())?;
        let peer_name = expect_hello(&mut incoming).await.map_err(|Failure(reason)| reason)?;
        sh.remember_peer(info.peer, &conn, peer_name).await;
        let resume = Message::Resume { transfer_id: *info.id.as_bytes() };
        write_frame(&mut send, &resume).await.map_err(|e| e.to_string())?;
        Ok::<_, String>(Session { conn, send, incoming })
    };
    let mut live = match tokio::time::timeout(CONNECT_TIMEOUT, dialled).await {
        Ok(Ok(live)) => live,
        Ok(Err(e)) => {
            tracing::debug!(transfer = %info.id, "could not reach the Sender: {e}");
            return Resumed::Unreachable;
        }
        Err(_) => {
            tracing::debug!(transfer = %info.id, "timed out reaching the Sender");
            return Resumed::Unreachable;
        }
    };
    // The Sender may take a while to answer: it has to look at its files first.
    match live.incoming.recv().await {
        Some(Ok(Message::ResumeOk)) => Resumed::Session(live),
        Some(Ok(Message::Cancel)) => Resumed::Cancelled,
        Some(Ok(Message::Unknown)) => Resumed::Refused(FORGOTTEN.into()),
        Some(Ok(Message::Failed { reason })) => Resumed::Refused(reason),
        other => {
            tracing::debug!(transfer = %info.id, "no usable answer to Resume: {other:?}");
            Resumed::Unreachable
        }
    }
}

enum Awaited {
    Ready(Hash),
    Stopped(Stop),
}

/// Waits for the Sender's `HashReady`: the content is hashed and this Receiver may fetch.
/// A connection lost before then ends the Transfer: only an accepted Transfer with its
/// content ready can be resumed.
async fn await_hash(live: &mut Session, cancel: &CancellationToken) -> Result<Awaited, Failure> {
    tokio::select! {
        biased;
        msg = live.incoming.recv() => match msg {
            Some(Ok(Message::HashReady { collection_hash })) => Ok(Awaited::Ready(collection_hash.into())),
            Some(Ok(Message::Cancel)) => Ok(Awaited::Stopped(Stop::PeerCancelled)),
            // The Sender's clock ran out just before it read our answer.
            Some(Ok(Message::Expired)) => Ok(Awaited::Stopped(Stop::PeerExpired)),
            Some(Ok(_)) => Err(Failure::with(UNEXPECTED, "message before HashReady")),
            Some(Err(e)) => Err(Failure::with(LOST, e)),
            None => Err(Failure::with(LOST, "stream ended before HashReady")),
        },
        () = cancel.cancelled() => Ok(Awaited::Stopped(Stop::Cancelled)),
    }
}

enum Fetched {
    /// The file is in the save folder, at this path.
    Saved(String),
    /// Cancelled before anything was saved.
    Stopped(Stop),
    /// The Sender went away mid-fetch. What arrived stays in the store.
    Lost,
}

/// Announces how much has arrived on our own event stream and tells the Sender, whose
/// display it feeds. The Sender being unreachable is no reason to stop.
async fn report_progress(sh: &Shared, live: &mut Session, info: &TransferInfo, bytes: u64) {
    sh.progress(info, bytes);
    let _ = write_frame(&mut live.send, &Message::Progress { bytes: bytes.min(info.size) }).await;
}

/// Fetches the Transfer's content into `store`, checks it is what was offered, builds the tree
/// the manifest describes and moves it into `save_dir`. Returns where it ended up. Whatever
/// `store` already holds from an earlier try is kept, and only the rest is requested.
async fn fetch_and_save(
    sh: &Arc<Shared>,
    live: &mut Session,
    cancel: &CancellationToken,
    store: &store::Store,
    info: &TransferInfo,
    root: Hash,
    save_dir: &Path,
    manifest: &Arc<Manifest>,
    stall: &mut Stall,
) -> Result<Fetched, Failure> {
    let blobs: &iroh_blobs::api::Store = store;
    let content = HashAndFormat::hash_seq(root);
    let sender = info.peer.endpoint_id();
    // iroh-blobs counts only what a request downloads, so the progress shown adds what an
    // earlier try left in the store.
    let already = blobs.remote().local(content).await.map_err(fail(CANT_FETCH))?.local_bytes();
    let allowance = collection_allowance(manifest);

    let conn = tokio::select! {
        biased;
        () = cancel.cancelled() => return Ok(Fetched::Stopped(Stop::Cancelled)),
        conn = sh.endpoint.connect(EndpointAddr::new(sender), iroh_blobs::ALPN) => match conn {
            Ok(conn) => conn,
            Err(e) => {
                tracing::debug!(transfer = %info.id, "could not reach the Sender's provider: {e}");
                return Ok(Fetched::Lost);
            }
        }
    };
    // iroh-blobs checks every chunk against its BLAKE3 hash as it arrives.
    let mut fetch = Box::pin(blobs.remote().fetch(conn.clone(), content).stream());
    let mut last_report: Option<UnixMillis> = None;
    loop {
        let item = tokio::select! {
            biased;
            msg = live.incoming.recv() => match msg {
                Some(Ok(Message::Cancel)) => return Ok(Fetched::Stopped(Stop::PeerCancelled)),
                Some(Ok(_)) => return Err(Failure::with(UNEXPECTED, "message while fetching")),
                // The Sender only ends this stream by going away.
                Some(Err(_)) | None => return Ok(Fetched::Lost),
            },
            () = cancel.cancelled() => return Ok(Fetched::Stopped(Stop::Cancelled)),
            () = sleep_until(&*sh.clock, stall.deadline()) => return Err(Failure(STALLED.into())),
            item = fetch.next() => item,
        };
        let Some(item) = item else { break };
        match item {
            GetProgressItem::Progress(downloaded) => {
                let bytes = already.saturating_add(downloaded);
                // The free-space check was made for the offered size; a Sender that sends
                // more must not fill the disk beyond it.
                if bytes > info.size.saturating_add(allowance) {
                    conn.close(0u32.into(), b"too much");
                    return Err(Failure::with(TOO_MUCH, format!("{bytes} bytes, offered {}", info.size)));
                }
                made_progress(sh, info, stall).await;
                let now = sh.now();
                if last_report.is_none_or(|at| now - at >= PROGRESS_INTERVAL_MS) {
                    last_report = Some(now);
                    report_progress(sh, live, info, bytes).await;
                }
            }
            GetProgressItem::Done(_) => break,
            // A broken store is ours to fail on; anything else is the connection.
            GetProgressItem::Error(e @ GetError::LocalFailure { .. }) => {
                return Err(Failure::with(CANT_FETCH, e));
            }
            GetProgressItem::Error(e) => {
                tracing::debug!(transfer = %info.id, "fetch interrupted: {e}");
                return Ok(Fetched::Lost);
            }
        }
    }
    conn.close(0u32.into(), b"done");
    if !blobs.remote().local(content).await.map_err(fail(CANT_FETCH))?.is_complete() {
        return Err(Failure::with(CANT_FETCH, "fetch ended incomplete"));
    }
    // The count above includes the Collection's own few bytes, so only now is it exact.
    report_progress(sh, live, info, info.size).await;

    // The Collection must list exactly the files that were offered, under the names the
    // manifest gave them.
    let collection = Collection::load(root, blobs).await.map_err(fail(WRONG_FILE))?;
    let named = collection.iter().map(|(name, _)| name.as_str());
    if collection.len() as u64 != manifest.file_count() || !named.eq(manifest.files().map(|(path, _)| path)) {
        return Err(Failure::with(WRONG_FILE, "the Collection's names differ from the manifest"));
    }

    // From here the files are put in place, which cannot be taken back.
    sh.untrack(info.id);
    sh.transition(info, TransferState::Saving).await;
    // Everything is built under `out` first, so that nothing reaches the save folder until
    // all of it is received and right. A try before a restart may have left some behind.
    let out = incoming_dir(save_dir, info.id).join(OUT_DIR);
    remove_dir(&out).await;
    // Under safe names (the same ones the Offer was shown with), file for file in the order
    // the Collection was just checked to have.
    let adjusted = adjust_names(manifest, INCOMING_DIR).manifest;
    for ((_, hash), (staged, _)) in collection.iter().zip(adjusted.files()) {
        blobs
            .blobs()
            .export_with_opts(ExportOptions {
                hash: *hash,
                // The store keeps the data in its own file, so this is a rename, not a copy.
                mode: ExportMode::TryReference,
                target: out.join(staged),
            })
            .finish()
            .await
            .map_err(fail(CANT_SAVE))?;
    }

    let save_dir = save_dir.to_owned();
    let saved = tokio::task::spawn_blocking(move || {
        build_tree(&out, &adjusted)?;
        move_into_save_folder(&out, &save_dir, &adjusted.top_level_items())
    })
    .await
    .map_err(fail(CANT_SAVE))?
    .map_err(|e| {
        if e.kind() == io::ErrorKind::InvalidData { Failure::with(WRONG_FILE, e) } else { Failure::with(CANT_SAVE, e) }
    })?;
    Ok(Fetched::Saved(saved.to_string_lossy().into_owned()))
}

/// How many threads finish the staged files at once. Each file is fsynced, which waits on the
/// disk, so a folder of thousands of small files takes minutes one at a time.
const BUILD_THREADS: usize = 8;

/// Makes the files exported under `out` what the manifest says they are: checks each is the
/// size offered, gives it its modification time and executable bit, makes the empty folders,
/// and makes all of it durable. A file of another size is `InvalidData`.
fn build_tree(out: &Path, manifest: &Manifest) -> io::Result<()> {
    let per_thread = manifest.entries.len().div_ceil(BUILD_THREADS).max(1);
    std::thread::scope(|scope| {
        manifest
            .entries
            .chunks(per_thread)
            .map(|part| scope.spawn(move || part.iter().try_for_each(|entry| finish_entry(out, entry))))
            .collect::<Vec<_>>()
            .into_iter()
            .try_for_each(|worker| {
                worker.join().unwrap_or_else(|_| Err(io::Error::other("a worker thread panicked")))
            })
    })?;
    // Every folder that holds something, to be fsynced so the names in it last.
    let mut folders = BTreeSet::from([out.to_owned()]);
    for entry in &manifest.entries {
        let path = out.join(entry.path());
        let holder = match entry {
            manifest::Entry::File { .. } => path.parent().unwrap_or(out),
            manifest::Entry::EmptyDir { .. } => &path,
        };
        for folder in holder.ancestors().take_while(|folder| *folder != out) {
            folders.insert(folder.to_owned());
        }
    }
    folders.iter().try_for_each(|folder| sync_dir(folder))
}

fn finish_entry(out: &Path, entry: &manifest::Entry) -> io::Result<()> {
    match entry {
        manifest::Entry::File { path, size, mtime_ns, executable } => {
            let file = std::fs::OpenOptions::new().write(true).open(out.join(path))?;
            if file.metadata()?.len() != *size {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "a file is not the size offered"));
            }
            // A time before 1970 or out of range is left as it came out of the store.
            let at = u64::try_from(*mtime_ns)
                .ok()
                .and_then(|ns| std::time::UNIX_EPOCH.checked_add(Duration::from_nanos(ns)));
            if let Some(at) = at {
                file.set_modified(at)?;
            }
            if *executable {
                make_executable(&file)?;
            }
            file.sync_all()
        }
        manifest::Entry::EmptyDir { path } => std::fs::create_dir_all(out.join(path)),
    }
}

/// Lets everyone who can read the file run it too, as the Sender's file could be run by them.
#[cfg(unix)]
fn make_executable(file: &std::fs::File) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = file.metadata()?.permissions().mode();
    file.set_permissions(std::fs::Permissions::from_mode(mode | ((mode & 0o444) >> 2)))
}

#[cfg(not(unix))]
fn make_executable(_: &std::fs::File) -> io::Result<()> {
    Ok(())
}

/// Moves each top-level item built under `out` into `save_dir` under a name that is not
/// taken (`photos`, `photos (1)`, ...), and fsyncs the folder so the renames survive a power
/// cut. An item is moved as a unit, never merged into one already there, and the move fails
/// rather than replaces, so an item that appears at the chosen name at any moment, even from
/// another program, is never overwritten: the next name is tried. If one item cannot be moved
/// the ones already moved are put back, so that a failure leaves nothing in `save_dir`.
/// Returns where the item went, or `save_dir` itself when there were several.
fn move_into_save_folder(out: &Path, save_dir: &Path, items: &[String]) -> io::Result<PathBuf> {
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    let result = items.iter().try_for_each(|item| {
        let staged = out.join(item);
        let dest = move_item(&staged, save_dir, item)?;
        moved.push((dest, staged));
        Ok(())
    });
    if let Err(e) = result.and_then(|()| sync_dir(save_dir)) {
        for (dest, staged) in moved.iter().rev() {
            if let Err(undo) = rename_no_replace(dest, staged) {
                tracing::warn!("could not take {} back out of the save folder: {undo}", dest.display());
            }
        }
        return Err(e);
    }
    Ok(match moved.as_slice() {
        [(dest, _)] => dest.clone(),
        _ => save_dir.to_owned(),
    })
}

fn move_item(staged: &Path, save_dir: &Path, name: &str) -> io::Result<PathBuf> {
    for n in 0u32.. {
        let candidate = if n == 0 { name.to_owned() } else { numbered(name, n) };
        let dest = save_dir.join(candidate);
        match rename_no_replace(staged, &dest) {
            Ok(()) => return Ok(dest),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
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

/// Shuts the Transfer's store down, then deletes its directory unless `delete` is false (the
/// Device is shutting down with the Transfer unfinished). Runs as its own tracked task, so a
/// Device shutdown waits for it instead of cancelling it.
fn close_store(sh: &Shared, opened: Opened, delete: bool) {
    sh.tasks.spawn(async move {
        let Opened { store, dir } = opened;
        if let Err(e) = store.shutdown().await {
            tracing::warn!("closing incoming store: {e}");
        }
        if delete {
            remove_dir(&dir).await;
        }
    });
}

async fn remove_dir(dir: &Path) {
    if let Err(e) = tokio::fs::remove_dir_all(dir).await {
        if e.kind() != io::ErrorKind::NotFound {
            tracing::warn!("removing {}: {e}", dir.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::UNIX_EPOCH;

    use iroh_blobs::{hashseq::HashSeq, store::mem::MemStore};

    use super::*;
    use crate::manifest::Entry;

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn items(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).to_owned()).collect()
    }

    #[test]
    fn each_top_level_item_moves_as_a_unit_to_a_name_that_is_free() {
        let tmp = tempfile::tempdir().unwrap();
        let (out, save) = (tmp.path().join("out"), tmp.path().join("save"));
        write(&out.join("album/a.txt"), b"theirs");
        write(&out.join("note.txt"), b"theirs");
        write(&save.join("album/mine.txt"), b"mine");
        write(&save.join("note.txt"), b"mine");
        write(&save.join("note (1).txt"), b"mine too");

        let at = move_into_save_folder(&out, &save, &items(&["album", "note.txt"])).unwrap();

        // Several items: there is no one place to point at.
        assert_eq!(at, save);
        assert_eq!(names(&save), ["album", "album (1)", "note (1).txt", "note (2).txt", "note.txt"]);
        assert_eq!(names(&save.join("album")), ["mine.txt"]);
        assert_eq!(std::fs::read(save.join("album (1)/a.txt")).unwrap(), b"theirs");
        assert_eq!(std::fs::read(save.join("note.txt")).unwrap(), b"mine");
        assert_eq!(std::fs::read(save.join("note (2).txt")).unwrap(), b"theirs");
        assert!(names(&out).is_empty());
    }

    #[test]
    fn a_path_fits_when_the_staged_tree_under_the_incoming_store_stays_within_the_limit() {
        let save = Path::new("/home/me/Downloads/BhayanakShare");
        let id = TransferId::from_bytes([0xab; 16]);
        // What is in front of a Transfer's paths while it is being built.
        let before = incoming_dir(save, id).join(OUT_DIR).as_os_str().len() + 1;
        assert_eq!(before, save.as_os_str().len() + 1 + INCOMING_DIR.len() + 1 + 32 + 1 + 3 + 1);

        assert!(paths_fit(save, id, 0));
        assert!(paths_fit(save, id, MAX_PATH - before));
        assert!(!paths_fit(save, id, MAX_PATH - before + 1));
        // A longer save folder leaves less room.
        assert!(!paths_fit(&save.join("x".repeat(100)), id, MAX_PATH - before));
        // The limit counts bytes, not characters: 60 characters of 2 bytes are 120.
        assert!(paths_fit(&save.join("é".repeat(60)), id, MAX_PATH - before - 121));
        assert!(!paths_fit(&save.join("é".repeat(60)), id, MAX_PATH - before - 120));
    }

    #[test]
    fn a_single_item_reports_where_it_went() {
        let tmp = tempfile::tempdir().unwrap();
        let (out, save) = (tmp.path().join("out"), tmp.path().join("save"));
        write(&out.join("album/a.txt"), b"a");
        std::fs::create_dir_all(save.join("album")).unwrap();

        let at = move_into_save_folder(&out, &save, &items(&["album"])).unwrap();

        assert_eq!(at, save.join("album (1)"));
    }

    #[test]
    fn when_one_item_cannot_be_moved_the_ones_already_moved_are_put_back() {
        let tmp = tempfile::tempdir().unwrap();
        let (out, save) = (tmp.path().join("out"), tmp.path().join("save"));
        write(&out.join("first/a.txt"), b"a");
        write(&out.join("second.txt"), b"b");
        std::fs::create_dir_all(&save).unwrap();

        // The third is not there to move: nothing is left half-delivered.
        let result = move_into_save_folder(&out, &save, &items(&["first", "second.txt", "third"]));

        assert!(result.is_err());
        assert!(names(&save).is_empty(), "{:?}", names(&save));
        assert_eq!(names(&out), ["first", "second.txt"]);
        assert_eq!(std::fs::read(out.join("first/a.txt")).unwrap(), b"a");
    }

    #[test]
    fn the_staged_tree_gets_its_times_bits_and_empty_folders_and_is_checked_for_size() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("out");
        write(&out.join("d/run.sh"), b"#!/bin/sh\n");
        write(&out.join("d/plain"), b"plain");
        let nanos = 1_600_000_000_123_456_789i64;
        let manifest = Manifest {
            entries: vec![
                Entry::File { path: "d/run.sh".into(), size: 10, mtime_ns: nanos, executable: true },
                Entry::File { path: "d/plain".into(), size: 5, mtime_ns: -1, executable: false },
                Entry::empty_dir("d/sub/empty"),
            ],
        };

        build_tree(&out, &manifest).unwrap();

        let run = std::fs::metadata(out.join("d/run.sh")).unwrap();
        assert_eq!(run.modified().unwrap(), UNIX_EPOCH + Duration::from_nanos(nanos as u64));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_ne!(run.permissions().mode() & 0o111, 0);
            let plain = std::fs::metadata(out.join("d/plain")).unwrap();
            assert_eq!(plain.permissions().mode() & 0o111, 0);
        }
        assert!(out.join("d/sub/empty").is_dir());

        // A file that is not the size offered is refused, not delivered.
        let wrong = Manifest { entries: vec![Entry::file("d/plain", 6)] };
        let err = build_tree(&out, &wrong).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    /// The allowance for the Collection is what lets a folder through the "sent more than it
    /// offered" cut-off; it has to cover the real thing, names and hashes, for a big folder.
    #[tokio::test]
    async fn the_collection_allowance_covers_a_real_collections_size() {
        let store = MemStore::new();
        let mut entries = Vec::new();
        let mut manifest = Manifest::default();
        for i in 0..2_000 {
            let path = format!("{}/{}", "d".repeat(100), format!("{i}-{}", "n".repeat(150)));
            let tag = store.blobs().add_bytes(vec![i as u8; 3]).temp_tag().await.unwrap();
            entries.push((path.clone(), tag.hash()));
            manifest.entries.push(Entry::file(path, 3));
        }
        let root = Collection::from_iter(entries).store(&store).await.unwrap();

        let sequence = store.blobs().get_bytes(root.hash()).await.unwrap();
        let meta = HashSeq::try_from(sequence.clone()).unwrap().iter().next().unwrap();
        let meta_len = store.blobs().get_bytes(meta).await.unwrap().len();
        let on_the_wire = (sequence.len() + meta_len) as u64;

        assert!(on_the_wire <= collection_allowance(&manifest), "{on_the_wire} bytes of Collection");
        // And it is not wildly more than that: a Sender cannot hide much in it.
        assert!(collection_allowance(&manifest) < on_the_wire * 2);
    }
}
