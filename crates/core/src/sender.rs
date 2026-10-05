//! The Sender side of a Transfer.
//!
//! Dial the Receiver, say Hello, send the Offer straight away, and hash the files while the
//! Receiver decides (they are imported by reference, so nothing is copied). Once hashing
//! is done and the Receiver has accepted, check that the files are as they were hashed (a changed
//! or missing one fails the Transfer, and the Receiver is told why), allow that Receiver to fetch
//! (see `gate`) and send `HashReady`; both sides show the Transfer as Preparing meanwhile (see
//! `PreparingEvent`). The Receiver then pulls the content over iroh-blobs from this Device's global
//! store. The Transfer ends with the Receiver's `Decline` or `Completed`, or earlier: either
//! side can cancel, an Offer nobody answers expires, and a Receiver with too many Offers from
//! this Device says `Busy`.
//!
//! From `HashReady` on, a lost connection does not end the Transfer. The Sender drops its
//! grant and waits: the Receiver drives resume, dialling again and saying `Resume` (see
//! [`resume`]). The Sender then checks its files, takes a fresh grant and answers `ResumeOk`.
//! A Sender that restarted does the same: the Transfer record, content hash and source files
//! are in the database, and [`recover`] starts waiting for the Receiver. With no progress for
//! 24 hours the Sender gives up.
//!
//! What is offered is a manifest (see `manifest`) and an iroh-blobs Collection of the same
//! files, in the same order, named by their manifest paths.
//!
//! Text of up to 64 KiB is the exception: it is the Offer itself, so there is nothing to hash,
//! no Collection and no grant, and no `HashReady`. The Receiver answers `Accept` and, once it
//! has kept the text, `Completed`; a connection lost before that fails the Transfer, since
//! there is no content hash to resume from. Longer text is written to a file called
//! `text.txt` and sent as one file.
//!
//! A Batch is one send to several Receivers: one Transfer, and one task here, per Receiver, all
//! sharing one [`Outgoing`]. The files are hashed once for all of them, and at most
//! [`MAX_DOWNLOADS`] of the Receivers that have accepted download at a time. A Receiver that
//! has accepted and has to wait for a slot is simply not sent `HashReady` yet, and is not
//! granted anything: it already waits for `HashReady`, so it needs no message of its own, and
//! the Sender still reads its `Cancel` meanwhile. The Sender shows such a Transfer as Waiting.
//! A Receiver gives its slot up when it loses its connection (it may be gone for hours) and
//! takes one again before it is answered `ResumeOk`.

use std::{collections::HashMap, io::Write, path::PathBuf, sync::Arc, time::Duration};

use iroh::EndpointId;
use iroh_blobs::{
    BlobFormat, Hash, HashAndFormat,
    api::{
        TempTag,
        blobs::{AddPathOptions, ImportMode},
    },
    format::collection::Collection,
};
use n0_future::{BufferedStreamExt, StreamExt, stream};
use tokio::sync::{OnceCell, OwnedSemaphorePermit, Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

use crate::{
    clock::{UnixMillis, sleep_until},
    db::{Db, Source},
    device::{DeviceAddr, Shared, TransferInfo},
    error::Error,
    gate::Grant,
    identity::DeviceId,
    manifest::Manifest,
    protocol::{self, LONG_TEXT_NAME, MAX_INLINE_TEXT, Message, Offer, write_frame},
    scan::{self, Scan},
    session::{
        BUSY, CLOSE_GRACE, Failure, HelloError, INVALID_NAMES, LOST, STALLED, Session, Stall, Stop, UNEXPECTED,
        expect_hello, fail, made_progress, save_progress, stop,
    },
    transfer::{Role, STALL_TTL_MS, TransferId, TransferState},
};

/// How many of a Batch's Receivers may download at once (spec section 4).
pub(crate) const MAX_DOWNLOADS: usize = 3;

/// What one send is made of.
#[derive(Debug, Clone)]
pub(crate) enum Payload {
    /// Files and folders, by absolute path.
    Paths(Vec<PathBuf>),
    Text(String),
}

/// What the Transfers made by one send have in common: what was scanned, the content hash
/// (worked out once, whichever Transfer needs it first), and the Batch's download slots.
pub(crate) struct Outgoing {
    manifest: Manifest,
    sources: Vec<Source>,
    skipped_links: u32,
    /// What the user picked, kept for `Device::resend`.
    payload: Payload,
    slots: Arc<Semaphore>,
    hashed: OnceCell<Hashed>,
    /// The file a text too long to go inline is sent from; deleted with the send.
    _file: Option<TextFile>,
}

struct Hashed {
    root: Hash,
    /// Keep the imported blobs alive for as long as any Transfer of the send runs.
    _tags: Vec<TempTag>,
}

impl Outgoing {
    pub fn new(scan: Scan, payload: Payload, slots: Arc<Semaphore>) -> Self {
        let Scan { manifest, sources, skipped_links } = scan;
        Self { manifest, sources, skipped_links, payload, slots, hashed: OnceCell::new(), _file: None }
    }

    /// Reads what `payload` names off the disk, or, for text too long to go inline, writes it
    /// to a file first. Text that fits in an Offer needs neither. Fails if the selection
    /// cannot be sent (a limit, a name, empty text).
    pub async fn prepare(sh: &Shared, payload: Payload, slots: Arc<Semaphore>) -> Result<Self, Error> {
        let (payload, roots, file) = match payload {
            Payload::Paths(paths) => {
                let roots = paths
                    .iter()
                    .map(|path| std::path::absolute(path).map_err(|e| Error::io("resolving the path", e)))
                    .collect::<Result<Vec<_>, _>>()?;
                (Payload::Paths(roots.clone()), roots, None)
            }
            Payload::Text(text) if text.is_empty() => return Err(Error::EmptyText),
            Payload::Text(text) if text.len() <= MAX_INLINE_TEXT => {
                let nothing = Scan { manifest: Manifest::default(), sources: Vec::new(), skipped_links: 0 };
                return Ok(Self::new(nothing, Payload::Text(text), slots));
            }
            Payload::Text(text) => {
                let file = TextFile::write(sh, &text).await?;
                let roots = vec![file.path()];
                (Payload::Text(text), roots, Some(file))
            }
        };
        let scan = tokio::task::spawn_blocking(move || scan::scan(&roots))
            .await
            .map_err(|e| Error::io("reading the files", std::io::Error::other(e)))??;
        Ok(Self { _file: file, ..Self::new(scan, payload, slots) })
    }

    /// The text, if it goes in the Offer itself.
    pub fn inline_text(&self) -> Option<&str> {
        match &self.payload {
            Payload::Text(text) if text.len() <= MAX_INLINE_TEXT => Some(text),
            _ => None,
        }
    }

    pub fn payload(&self) -> &Payload {
        &self.payload
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn sources(&self) -> &[Source] {
        &self.sources
    }

    pub fn skipped_links(&self) -> u32 {
        self.skipped_links
    }

    /// The Collection's hash, importing the files the first time it is asked for. Transfers
    /// that ask meanwhile wait for that one import; if the one doing it is dropped (its
    /// Receiver declined), another takes over.
    async fn hash(&self, store: iroh_blobs::api::Store, db: &Db) -> Result<Hash, Failure> {
        let hashed = self
            .hashed
            .get_or_try_init(|| async {
                let (root, tags) = import(store, db, &self.sources).await?;
                Ok(Hashed { root, _tags: tags })
            })
            .await?;
        Ok(hashed.root)
    }
}

/// Where, in the data folder, the files of texts too long to go inline are written.
fn texts_dir(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join("outgoing-text")
}

/// The file a long text is sent from, `<data folder>/outgoing-text/<random>/text.txt`. It lives
/// in the data folder, not a temp folder, because a Transfer resumed after a restart is served
/// from its files again. It is deleted when the send is over (its Transfers have all ended),
/// not when the Device is shutting down with some of them unfinished: [`sweep_texts`] takes
/// care of the ones a crash or a restart leaves.
///
/// The same text sent again (a retry, or the same words twice) comes from a new file, while the
/// store may still refer to this one for the same content; [`import`] sees to that.
struct TextFile {
    dir: PathBuf,
    shutdown: CancellationToken,
}

impl TextFile {
    async fn write(sh: &Shared, text: &str) -> Result<Self, Error> {
        let dir = texts_dir(&sh.data_dir).join(TransferId::random().to_string());
        // Owned first, so that a write that fails part-way is cleaned up.
        let file = Self { dir: dir.clone(), shutdown: sh.cancel.clone() };
        let (path, text) = (file.path(), text.to_owned());
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&dir)?;
            let mut out = std::fs::File::create(path)?;
            out.write_all(text.as_bytes())?;
            out.sync_all()
        })
        .await
        .map_err(|e| Error::io("writing the text", std::io::Error::other(e)))?
        .map_err(|e| Error::io("writing the text", e))?;
        Ok(file)
    }

    fn path(&self) -> PathBuf {
        self.dir.join(LONG_TEXT_NAME)
    }
}

impl Drop for TextFile {
    fn drop(&mut self) {
        if !self.shutdown.is_cancelled() {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// Deletes what earlier runs left in `outgoing-text` that no Transfer this Device is yet to
/// carry on is served from. Run once at start, after the Transfers that cannot be carried on
/// have been ended.
pub(crate) async fn sweep_texts(sh: &Shared) -> Result<(), Error> {
    let mut wanted = Vec::new();
    for unfinished in sh.db.unfinished().await? {
        if unfinished.record.role == Role::Sender {
            wanted.extend(sh.db.sources(unfinished.record.id).await?.into_iter().map(|s| s.path));
        }
    }
    let root = texts_dir(&sh.data_dir);
    tokio::task::spawn_blocking(move || {
        let Ok(dirs) = std::fs::read_dir(&root) else { return };
        for dir in dirs.flatten().map(|dir| dir.path()) {
            if wanted.iter().any(|path| path.starts_with(&dir)) {
                continue;
            }
            if let Err(e) = std::fs::remove_dir_all(&dir) {
                tracing::warn!("removing {}: {e}", dir.display());
            }
        }
    })
    .await
    .map_err(|e| Error::io("clearing old texts", std::io::Error::other(e)))
}

pub(crate) async fn run(
    sh: Arc<Shared>,
    info: TransferInfo,
    to: DeviceAddr,
    out: Arc<Outgoing>,
    cancel: CancellationToken,
) {
    let outcome = tokio::select! {
        () = sh.cancel.cancelled() => None,
        outcome = flow(&sh, &info, &to, &out, &cancel) => Some(outcome),
    };
    sh.untrack(info.id);
    if let Some(Err(Failure(reason))) = outcome {
        sh.transition(&info, TransferState::Failed { reason }).await;
    }
}

/// Picks up a Transfer this Device was serving before it restarted, waiting for the Receiver
/// to dial back. Must run before the Device accepts connections, so a `Resume` that arrives
/// early finds the Transfer.
pub(crate) fn recover(
    sh: &Arc<Shared>,
    info: TransferInfo,
    root: [u8; 32],
    progress_at: UnixMillis,
) {
    let cancel = sh.track(info.id);
    let resumes = Resumes::register(sh, info.id, info.peer.endpoint_id());
    let slots = sh.slots(info.batch);
    let task_sh = sh.clone();
    sh.tasks.spawn(async move {
        let sh = task_sh;
        let root = Hash::from_bytes(root);
        let stall = Stall::new(progress_at);
        let outcome = tokio::select! {
            () = sh.cancel.cancelled() => None,
            outcome = transferring(
                &sh, &info, root, None, resumes, &cancel, stall, &slots, None,
            ) => Some(outcome),
        };
        sh.untrack(info.id);
        if let Some(Err(Failure(reason))) = outcome {
            sh.transition(&info, TransferState::Failed { reason }).await;
        }
    });
}

/// Why a Transfer fails when no address of the Receiver works: every lookup came back empty or
/// the addresses found got no answer.
const UNREACHABLE: &str = "Could not reach the receiving Device. It may be offline, or one of you may have no internet connection.";

/// How long to try the addresses found for the Receiver before giving up on it: a Device none of
/// the lookups can place fails at once, but a stale address (a Contact that has since gone
/// offline) gets no answer at all.
const DIAL_TIMEOUT: Duration = Duration::from_secs(20);

/// Dials the Receiver and exchanges `Hello`. The Receiver is found by the addresses in `to`,
/// the last known address of a Contact, n0 DNS and the DHT, all at once.
async fn connect(sh: &Shared, to: &DeviceAddr) -> Result<(Session, Option<String>), Failure> {
    let dial = sh.endpoint.connect(to.to_endpoint_addr(), protocol::ALPN);
    let conn = tokio::time::timeout(DIAL_TIMEOUT, dial)
        .await
        .map_err(|_| Failure::with(UNREACHABLE, "no answer from any address"))?
        .map_err(fail(UNREACHABLE))?;
    let (mut send, recv) = conn
        .open_bi()
        .await
        .map_err(fail("Could not reach the receiving Device."))?;
    let mut incoming = protocol::spawn_reader(recv);

    write_frame(&mut send, &Message::Hello(protocol::Hello::named(sh.device_name().await)))
        .await
        .map_err(fail(LOST))?;
    let peer_name = expect_hello(sh, to.id, &mut send, &mut incoming).await.map_err(HelloError::failure)?;
    Ok((Session { conn, send, incoming }, peer_name))
}

async fn flow(
    sh: &Arc<Shared>,
    info: &TransferInfo,
    to: &DeviceAddr,
    out: &Outgoing,
    cancel: &CancellationToken,
) -> Result<(), Failure> {
    // Until the Offer is out nobody else knows of the Transfer, so there is no one to tell.
    let greeted = tokio::select! {
        () = cancel.cancelled() => None,
        greeted = connect(sh, to) => Some(greeted?),
    };
    let Some((mut session, peer_name)) = greeted else {
        sh.untrack(info.id);
        sh.transition(info, TransferState::Cancelled { by: info.role }).await;
        return Ok(());
    };
    sh.remember_peer(to.id, &session.conn, peer_name.clone()).await;
    if let Some(name) = &peer_name {
        // For History, which says whom it was sent to as they called themselves then.
        if let Err(e) = sh.db.set_peer_name(info.id, name).await {
            tracing::warn!(transfer = %info.id, "could not record the Receiver's name: {e}");
        }
    }
    // From here on the Transfer's events carry what the Receiver calls itself.
    let info = &TransferInfo { peer_name, ..info.clone() };
    // Nothing in it says who else gets the files: every Receiver's Offer is its own.
    let inline = out.inline_text().is_some();
    let offer = Message::Offer(match out.inline_text() {
        Some(text) => Offer::text(*info.id.as_bytes(), text.to_owned()),
        None => Offer::new(*info.id.as_bytes(), out.manifest.clone(), out.skipped_links),
    });
    write_frame(&mut session.send, &offer).await.map_err(fail(LOST))?;
    drop(offer);

    // Hash while the Receiver decides, for the whole Batch at once (see `Outgoing::hash`). Not
    // spawned: leaving this function drops it, which abandons the hashing if no other
    // Transfer is waiting on it.
    // Text that went in the Offer has nothing to hash.
    let mut hashing = (!inline).then(|| Box::pin(out.hash(sh.blobs.clone(), &sh.db)));
    if hashing.is_some() {
        sh.preparing(info, true);
    }
    let mut accepted = false;
    let mut root = None;
    let peer = to.id.endpoint_id();
    // An Offer nobody answers lapses; once the Receiver says yes the clock no longer matters.
    let mut expiry = Box::pin(sleep_until(&*sh.clock, info.expires_at));

    // The Receiver has accepted and the content is hashed.
    let root = loop {
        if accepted {
            if let Some(root) = root {
                break root;
            }
        }
        tokio::select! {
            // In this order: what the Receiver has already said counts before our own cancel
            // or expiry, so both sides end the Transfer the same way.
            biased;
            msg = session.incoming.recv() => match msg {
                Some(Ok(Message::Accept)) if !accepted => {
                    accepted = true;
                    if inline {
                        // The Receiver keeps the text and says so straight after its yes: from
                        // here there is nothing left to cancel.
                        sh.untrack(info.id);
                    }
                    sh.transition(info, TransferState::Accepted).await;
                }
                Some(Ok(Message::Completed)) if accepted && inline => {
                    sh.transition(info, TransferState::Completed { saved_to: None }).await;
                    let _ = session.send.finish();
                    session.conn.close(0u32.into(), b"done");
                    return Ok(());
                }
                Some(Ok(Message::Decline)) if !accepted => {
                    sh.transition(info, TransferState::Declined).await;
                    // Nothing more is coming: close; the Receiver waits for this before
                    // dropping its side, so its last frame is never cut off.
                    let _ = session.send.finish();
                    session.conn.close(0u32.into(), b"done");
                    return Ok(());
                }
                Some(Ok(Message::Cancel)) => {
                    stop(sh, info, Some(&mut session), Stop::PeerCancelled).await;
                    return Ok(());
                }
                Some(Ok(Message::Expired)) if !accepted => {
                    lapse(sh, info, to, out.payload());
                    stop(sh, info, Some(&mut session), Stop::PeerExpired).await;
                    return Ok(());
                }
                Some(Ok(Message::Busy)) if !accepted => {
                    return Err(Failure::with(BUSY, "the Receiver answered Busy"));
                }
                Some(Ok(Message::InvalidOffer)) if !accepted => {
                    return Err(Failure::with(INVALID_NAMES, "the Receiver refused the manifest"));
                }
                Some(Ok(_)) => return Err(Failure::with(UNEXPECTED, "out-of-order message")),
                Some(Err(e)) => return Err(Failure::with(LOST, e)),
                None => return Err(Failure::with(LOST, "stream ended")),
            },
            () = cancel.cancelled(), if !(inline && accepted) => {
                stop(sh, info, Some(&mut session), Stop::Cancelled).await;
                return Ok(());
            }
            () = &mut expiry, if !accepted => {
                lapse(sh, info, to, out.payload());
                stop(sh, info, Some(&mut session), Stop::Expired).await;
                return Ok(());
            }
            done = async { hashing.as_mut().expect("guarded by the if below").await },
                if hashing.is_some() =>
            {
                hashing = None;
                match done {
                    Ok(hash) => {
                        root = Some(hash);
                        sh.preparing(info, false);
                    }
                    Err(failure) => {
                        // A file that went missing or changed while it was read is named as such.
                        let Failure(reason) = check_sources(sh, info).await.err().unwrap_or(failure);
                        tell_failed(&mut session, &reason).await;
                        return Err(Failure(reason));
                    }
                }
            }
        }
    };

    // Only so many of the Batch's Receivers download at once; this one has to wait its turn
    // for a slot, and until then is told nothing and may fetch nothing.
    let give_up = sh.now() + STALL_TTL_MS;
    let slot = match take_slot(sh, info, &out.slots, &mut session, cancel, give_up).await? {
        // `Transferring` is announced below either way.
        Slotted::Got { slot, .. } => slot,
        Slotted::Stopped(how) => {
            stop(sh, info, Some(&mut session), how).await;
            return Ok(());
        }
        Slotted::Gone => return Err(Failure::with(LOST, "the Receiver left while waiting for a slot")),
    };

    // The files may have changed since they were hashed, however long ago: the Receiver deciding,
    // or this one waiting for a slot, can take a while. They are checked now, before the
    // Receiver is allowed to fetch them, and a Receiver fetching a file that changed would only
    // see the fetch fail.
    if let Err(Failure(reason)) = check_sources(sh, info).await {
        tell_failed(&mut session, &reason).await;
        return Err(Failure(reason));
    }

    // `HashReady` is sent only once the Receiver has accepted, and only after the grant is
    // taken. Sent earlier, a Receiver holding the hash could dial the provider the moment
    // it says yes, before this has read that yes, and be turned away. The hash is saved
    // before the Receiver can learn it, so a restart can pick the Transfer up again.
    let grant = sh.gate.allow(peer, root);
    let resumes = Resumes::register(sh, info.id, peer);
    let now = sh.now();
    if let Err(e) = sh.db.start_transfer(info.id, Some(*root.as_bytes()), None, now).await {
        tracing::warn!(transfer = %info.id, "could not record the content hash, so a restart cannot resume: {e}");
    }
    write_frame(&mut session.send, &Message::HashReady { collection_hash: *root.as_bytes() })
        .await
        .map_err(fail(LOST))?;
    sh.transition(info, TransferState::Transferring).await;
    let live = Some((session, grant));
    transferring(sh, info, root, live, resumes, cancel, Stall::new(now), &out.slots, Some(slot)).await
}

enum Slotted {
    /// `waited`: the Transfer was shown as Waiting, so it is due to be shown again.
    Got { slot: OwnedSemaphorePermit, waited: bool },
    Stopped(Stop),
    /// The Receiver's connection ended while it waited.
    Gone,
}

/// Takes one of the Batch's download slots, showing the Transfer as Waiting if none is free.
/// While waiting, the Receiver's `Cancel` and this Device's own cancel still count; with no
/// progress by `give_up` the Transfer fails as stalled, like any other.
async fn take_slot(
    sh: &Shared,
    info: &TransferInfo,
    slots: &Arc<Semaphore>,
    session: &mut Session,
    cancel: &CancellationToken,
    give_up: UnixMillis,
) -> Result<Slotted, Failure> {
    if let Ok(slot) = slots.clone().try_acquire_owned() {
        return Ok(Slotted::Got { slot, waited: false });
    }
    sh.transition(info, TransferState::Waiting).await;
    tokio::select! {
        biased;
        msg = session.incoming.recv() => match msg {
            Some(Ok(Message::Cancel)) => Ok(Slotted::Stopped(Stop::PeerCancelled)),
            Some(Ok(_)) => Err(Failure::with(UNEXPECTED, "message while waiting for a slot")),
            Some(Err(_)) | None => Ok(Slotted::Gone),
        },
        () = cancel.cancelled() => Ok(Slotted::Stopped(Stop::Cancelled)),
        () = sleep_until(&*sh.clock, give_up) => Err(Failure(STALLED.into())),
        // Fair: slots go to the Transfers that have waited longest.
        slot = slots.clone().acquire_owned() => {
            slot.map(|slot| Slotted::Got { slot, waited: true }).map_err(fail(UNEXPECTED))
        }
    }
}

/// Serves the Transfer until the Receiver reports it finished: with the Receiver connected it
/// follows its progress; when the connection is lost it waits, grant dropped, for the
/// Receiver to dial back. `live` is the control connection of a Transfer that just started;
/// a recovered one has none. `slot` is the Batch's download slot the Transfer holds while it
/// has a connected Receiver (a recovered one holds none until its Receiver is back).
async fn transferring(
    sh: &Arc<Shared>,
    info: &TransferInfo,
    root: Hash,
    live: Option<(Session, Grant)>,
    mut resumes: Resumes,
    cancel: &CancellationToken,
    mut stall: Stall,
    slots: &Arc<Semaphore>,
    mut slot: Option<OwnedSemaphorePermit>,
) -> Result<(), Failure> {
    let (mut live, mut grant) = match live {
        Some((session, grant)) => (Some(session), Some(grant)),
        None => (None, None),
    };
    let mut keep_alive = Vec::new();
    let mut returned: Option<Session> = None;
    // A Transfer that just started has announced Transferring; a recovered one, whose last
    // run did, has not yet on this Device's event stream.
    let mut announced = live.is_some();
    loop {
        if let Some(mut back) = returned.take() {
            // The Receiver is answered, and so allowed to fetch, only once it has a slot.
            let taken = match slot.take() {
                Some(slot) => Slotted::Got { slot, waited: false },
                None => take_slot(sh, info, slots, &mut back, cancel, stall.deadline()).await?,
            };
            match taken {
                Slotted::Got { slot: taken, waited } => {
                    announced &= !waited;
                    // Otherwise the Receiver went away again before it was answered, and
                    // `taken` goes back.
                    if let Some((session, fresh)) = welcome(sh, info, root, back, &mut keep_alive).await? {
                        live = Some(session);
                        grant = Some(fresh);
                        slot = Some(taken);
                        if !announced {
                            announced = true;
                            sh.transition(info, TransferState::Transferring).await;
                        }
                    }
                }
                Slotted::Stopped(how) => {
                    stop(sh, info, Some(&mut back), how).await;
                    return Ok(());
                }
                // It went away again while it waited.
                Slotted::Gone => {}
            }
        }
        let Some(mut session) = live.take() else {
            // Nobody is connected: the Receiver may come back until the Transfer stalls out.
            tokio::select! {
                biased;
                Some(back) = resumes.rx.recv() => returned = Some(back),
                () = cancel.cancelled() => {
                    stop(sh, info, None, Stop::Cancelled).await;
                    return Ok(());
                }
                () = sleep_until(&*sh.clock, stall.deadline()) => {
                    return Err(Failure(STALLED.into()));
                }
            }
            continue;
        };
        match follow(sh, info, &mut session, &mut grant, &mut resumes, cancel, &mut stall).await? {
            Followed::Finished => return Ok(()),
            Followed::Lost => {
                drop(grant.take());
                // Nobody is downloading, and it may be a long while: let another Receiver in.
                drop(slot.take());
                save_progress(sh, info, &mut stall).await;
            }
            // The Receiver is back before we noticed it had gone: the old connection is dead,
            // and the slot stays this Transfer's.
            Followed::Replaced(back) => {
                drop(grant.take());
                session.conn.close(0u32.into(), b"replaced");
                returned = Some(back);
            }
        }
    }
}

enum Followed {
    /// Completed or cancelled by either side: the Transfer is over.
    Finished,
    /// The control connection ended.
    Lost,
    /// The Receiver dialled again while this connection looked alive.
    Replaced(Session),
}

/// Follows the Receiver's progress on one control connection.
async fn follow(
    sh: &Shared,
    info: &TransferInfo,
    session: &mut Session,
    grant: &mut Option<Grant>,
    resumes: &mut Resumes,
    cancel: &CancellationToken,
    stall: &mut Stall,
) -> Result<Followed, Failure> {
    loop {
        tokio::select! {
            biased;
            msg = session.incoming.recv() => match msg {
                Some(Ok(Message::Progress { bytes })) => {
                    sh.progress(info, bytes);
                    made_progress(sh, info, stall).await;
                }
                Some(Ok(Message::Completed)) => {
                    // Nothing may be fetched once the Transfer is reported finished.
                    drop(grant.take());
                    sh.transition(info, TransferState::Completed { saved_to: None }).await;
                    // Tell the Receiver nothing more is coming, then close; it waits for this
                    // before dropping its side, so its last frame is never cut off.
                    let _ = session.send.finish();
                    session.conn.close(0u32.into(), b"done");
                    return Ok(Followed::Finished);
                }
                Some(Ok(Message::Cancel)) => {
                    drop(grant.take());
                    stop(sh, info, Some(session), Stop::PeerCancelled).await;
                    return Ok(Followed::Finished);
                }
                Some(Ok(_)) => return Err(Failure::with(UNEXPECTED, "out-of-order message")),
                Some(Err(e)) => {
                    tracing::debug!(transfer = %info.id, "control connection lost: {e}");
                    return Ok(Followed::Lost);
                }
                None => return Ok(Followed::Lost),
            },
            Some(back) = resumes.rx.recv() => return Ok(Followed::Replaced(back)),
            () = cancel.cancelled() => {
                // Nothing may be fetched once the Transfer is reported stopped.
                drop(grant.take());
                stop(sh, info, Some(session), Stop::Cancelled).await;
                return Ok(Followed::Finished);
            }
        }
    }
}

/// Answers a Receiver that dialled back with `Resume`. Checks the files are as they were,
/// takes a fresh grant and only then says `ResumeOk`, the go-ahead to fetch (as for the first
/// connection, an earlier go-ahead could race the grant). Fails the Transfer, telling the
/// Receiver why, if a file changed. `None` if the Receiver went away while it was being
/// answered.
async fn welcome(
    sh: &Shared,
    info: &TransferInfo,
    root: Hash,
    mut session: Session,
    keep_alive: &mut Vec<TempTag>,
) -> Result<Option<(Session, Grant)>, Failure> {
    if let Err(Failure(reason)) = check_files(sh, info, root, keep_alive).await {
        tell_failed(&mut session, &reason).await;
        return Err(Failure(reason));
    }
    let grant = sh.gate.allow(info.peer.endpoint_id(), root);
    match write_frame(&mut session.send, &Message::ResumeOk).await {
        Ok(()) => Ok(Some((session, grant))),
        Err(e) => {
            tracing::debug!(transfer = %info.id, "the Receiver left before ResumeOk: {e}");
            Ok(None)
        }
    }
}

/// Tells the Receiver the Transfer failed on this side, for this plain-language reason, and
/// waits for it to hang up so the message is not cut off.
async fn tell_failed(session: &mut Session, reason: &str) {
    let _ = write_frame(&mut session.send, &Message::Failed { reason: reason.to_owned() }).await;
    let _ = session.send.finish();
    let _ = tokio::time::timeout(CLOSE_GRACE, session.conn.closed()).await;
}

/// Before serving again: every file must be as it was when the Offer was made, and the
/// store must still hold their content. It may not if the Device was killed just after hashing
/// it, as the store commits a moment later; the files are unchanged, so hashing them again
/// gives the same hash.
async fn check_files(
    sh: &Shared,
    info: &TransferInfo,
    root: Hash,
    keep_alive: &mut Vec<TempTag>,
) -> Result<(), Failure> {
    let sources = check_sources(sh, info).await?;
    let content = HashAndFormat::hash_seq(root);
    if !sh.blobs.remote().local(content).await.is_ok_and(|local| local.is_complete()) {
        let (hash, tags) = import(sh.blobs.clone(), &sh.db, &sources).await?;
        if hash != root {
            return Err(changed(&info.name));
        }
        keep_alive.extend(tags);
    }
    Ok(())
}

/// Before the content is served, the first time or again after a resume: every file of the
/// Transfer must still be there, with the size and modification time it had when the Offer was
/// made (spec section 4). Returns the files as recorded.
async fn check_sources(sh: &Shared, info: &TransferInfo) -> Result<Vec<Source>, Failure> {
    let sources = sh.db.sources(info.id).await.map_err(fail("Could not look up the Transfer."))?;
    let (sources, gone) = tokio::task::spawn_blocking(move || {
        let gone = sources.iter().find(|source| !is_unchanged(source)).map(|s| s.name.clone());
        (sources, gone)
    })
    .await
    .map_err(fail("Could not check the files."))?;
    match gone {
        Some(name) => Err(changed(&name)),
        None => Ok(sources),
    }
}

/// Why a Transfer fails when a file `name` (as the Offer called it) is not as it was.
fn changed(name: &str) -> Failure {
    Failure::with(&format!("A file changed on the sending Device: {name}"), "source check failed")
}

fn is_unchanged(source: &Source) -> bool {
    match std::fs::metadata(&source.path) {
        Ok(meta) => meta.is_file() && meta.len() == source.size && mtime_ns(&meta) == source.mtime_ns,
        Err(_) => false,
    }
}

/// The file's modification time in nanoseconds since the Unix epoch (0 if it cannot be read
/// or is before it).
pub(crate) fn mtime_ns(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| i64::try_from(since.as_nanos()).unwrap_or(i64::MAX))
}

/// Where a Receiver that dials back to resume is handed over to the task that runs the
/// Transfer. The entry is removed when this is dropped, which is when the task ends.
pub(crate) struct Resumes {
    sh: Arc<Shared>,
    id: TransferId,
    rx: mpsc::Receiver<Session>,
}

/// A running Transfer's entry in [`Shared::resumers`].
pub(crate) struct Resumer {
    /// The only Device that may resume it.
    peer: EndpointId,
    tx: mpsc::Sender<Session>,
}

impl Resumes {
    fn register(sh: &Arc<Shared>, id: TransferId, peer: EndpointId) -> Self {
        let (tx, rx) = mpsc::channel(4);
        sh.resumers.lock().unwrap_or_else(|e| e.into_inner()).insert(id, Resumer { peer, tx });
        Self { sh: sh.clone(), id, rx }
    }
}

impl Drop for Resumes {
    fn drop(&mut self) {
        self.sh.resumers.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.id);
    }
}

/// A Receiver dialled this Device and said `Resume` for `id`. If this Device is running the
/// Transfer for that Receiver, its task takes the connection and answers. Otherwise it is
/// answered here from the Transfer record: `Cancel` if this Device cancelled it, `Failed` if
/// it failed, `Unknown` for anything else, including a Transfer that belongs to someone else.
pub(crate) async fn resume(sh: &Shared, session: Session, id: TransferId) {
    let peer = session.conn.remote_id();
    let tx = sh
        .resumers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
        .filter(|running| running.peer == peer)
        .map(|running| running.tx.clone());
    let mut session = match tx {
        Some(tx) => match tx.send(session).await {
            Ok(()) => return,
            // The Transfer's task ended just now.
            Err(refused) => refused.0,
        },
        None => session,
    };
    let answer = match sh.db.transfer(id).await {
        Ok(Some(record))
            if record.role == Role::Sender
                && record.peer == DeviceId::from_endpoint_id(peer).to_string() =>
        {
            match record.state {
                TransferState::Cancelled { by: Role::Sender } => Message::Cancel,
                TransferState::Failed { reason } => Message::Failed { reason },
                _ => Message::Unknown,
            }
        }
        _ => Message::Unknown,
    };
    let _ = write_frame(&mut session.send, &answer).await;
    let _ = session.send.finish();
    let _ = tokio::time::timeout(CLOSE_GRACE, session.conn.closed()).await;
}

/// Keeps what an expired Offer held, so `Device::resend` can make it again.
fn lapse(sh: &Shared, info: &TransferInfo, to: &DeviceAddr, payload: &Payload) {
    sh.expired
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(info.id, (to.clone(), payload.clone()));
}

/// How many files are hashed at once; a folder of small files is mostly waiting on the disk.
const IMPORT_PARALLELISM: usize = 32;

/// Imports the files by reference into the global store and wraps them in a Collection named
/// by their manifest paths, in the order given: the shape the Receiver fetches. Returns the
/// Collection's hash and the temp tags that keep everything alive.
///
/// A blob imported by reference is not copied: the store keeps the file's path. For each hash it
/// keeps a sorted set of such paths, and whenever it needs the data it opens only the first
/// (iroh-blobs 0.103, `BaoFileStorage::open`), never trying the others. Importing the same
/// content from another path adds that path but does not drop a stale one. So once the file
/// first imported has been deleted, the blob is "poisoned storage" for every read after, and
/// once it has changed the blob reads back as garbage, however good the new file is. Either
/// way a Receiver's fetch fails, and the store has no call to remove a blob. What it does do is
/// let a copy replace the paths altogether (the copy wins when the entry is merged). So the
/// files the store refers to are recorded per hash (`blob_files`), and a source whose content
/// the store also refers to a file that has gone or changed for is imported again, by copy.
async fn import(
    store: iroh_blobs::api::Store,
    db: &Db,
    sources: &[Source],
) -> Result<(Hash, Vec<TempTag>), Failure> {
    // Each import owns what it uses: futures that borrow make the whole task fail to prove
    // that it can be sent between threads.
    let jobs: Vec<_> = sources.iter().map(|source| (store.clone(), source.path.clone())).collect();
    let mut tags: Vec<TempTag> = stream::iter(jobs)
        .map(|(store, path)| async move { add(&store, path, ImportMode::TryReference).await })
        .buffered_ordered(IMPORT_PARALLELISM)
        .try_collect()
        .await
        .map_err(fail("Could not read the files."))?;
    replace_stale(&store, db, sources, &tags).await?;
    let collection = Collection::from_iter(
        sources.iter().map(|source| source.name.clone()).zip(tags.iter().map(TempTag::hash)),
    );
    let root = collection
        .store(&store)
        .await
        .map_err(fail("Could not prepare the files for sending."))?;
    let hash = root.hash();
    tags.push(root);
    Ok((hash, tags))
}

async fn add(
    store: &iroh_blobs::api::Store,
    path: PathBuf,
    mode: ImportMode,
) -> Result<TempTag, iroh_blobs::api::RequestError> {
    store
        .blobs()
        .add_path_with_opts(AddPathOptions { path, format: BlobFormat::Raw, mode })
        .temp_tag()
        .await
}

/// Records the files just imported, and imports again by copy those whose content the store
/// also refers to a file for that has gone or changed since (see [`import`]). `tags` are the
/// imports of `sources`, in order.
async fn replace_stale(
    store: &iroh_blobs::api::Store,
    db: &Db,
    sources: &[Source],
    tags: &[TempTag],
) -> Result<(), Failure> {
    let hashes: Vec<[u8; 32]> = tags.iter().map(|tag| *tag.hash().as_bytes()).collect();
    // Not knowing what the store refers to is no reason to refuse the files: it only keeps
    // the next send of the same content from being repaired, not this one.
    let known = db.blob_files(hashes.clone()).await.unwrap_or_else(|e| {
        tracing::warn!("could not look up the files the store refers to: {e}");
        HashMap::new()
    });
    let mut noted = Vec::new();
    let mut forget = Vec::new();
    for (source, hash) in sources.iter().zip(hashes) {
        let stale = known
            .get(&hash)
            .is_some_and(|files| files.iter().any(|file| file.path != source.path && !is_unchanged(file)));
        if !stale {
            noted.push((hash, source.clone()));
            continue;
        }
        let copy = add(store, source.path.clone(), ImportMode::Copy)
            .await
            .map_err(fail("Could not read the files."))?;
        if copy.hash().as_bytes() != &hash {
            // It changed between the two imports.
            return Err(changed(&source.name));
        }
        forget.push(hash);
    }
    if let Err(e) = db.set_blob_files(noted, forget).await {
        tracing::warn!("could not record the files the store refers to: {e}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use iroh_blobs::store::mem::MemStore;

    use super::*;

    fn outgoing(dir: &std::path::Path) -> Outgoing {
        let path = dir.join("a.txt");
        std::fs::write(&path, b"hello").unwrap();
        let scan = crate::scan::scan(std::slice::from_ref(&path)).unwrap();
        Outgoing::new(scan, Payload::Paths(vec![path]), Arc::new(Semaphore::new(MAX_DOWNLOADS)))
    }

    async fn db(dir: &std::path::Path) -> Db {
        Db::open(&dir.join("t.db")).await.unwrap()
    }

    #[tokio::test]
    async fn the_transfers_of_a_batch_share_one_hash() {
        let dir = tempfile::tempdir().unwrap();
        let (out, store, db) = (outgoing(dir.path()), MemStore::new(), db(dir.path()).await);

        let hash = || out.hash((*store).clone(), &db);
        let (a, b, c) = tokio::join!(hash(), hash(), hash());

        let (a, b, c) = (a.unwrap(), b.unwrap(), c.unwrap());
        assert_eq!((a, b), (b, c));
        // Asked again later it is not worked out again: the files can even be gone.
        std::fs::remove_file(dir.path().join("a.txt")).unwrap();
        assert!(out.hash((*store).clone(), &db).await.is_ok());
    }

    #[tokio::test]
    async fn hashing_carries_on_when_the_transfer_that_began_it_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let (out, store, db) = (outgoing(dir.path()), MemStore::new(), db(dir.path()).await);

        // Its Receiver declines while it is under way.
        let started = tokio::time::timeout(Duration::ZERO, out.hash((*store).clone(), &db)).await;
        assert!(started.is_err(), "still hashing when dropped");

        assert!(out.hash((*store).clone(), &db).await.is_ok());
    }

    /// Over iroh-blobs' 16 KiB inline limit, so the store refers to the file.
    const SIZE: usize = 1 << 20;

    /// `<dir>/a/f` and `<dir>/b/f`, both holding the same `SIZE` bytes.
    fn two_copies(dir: &std::path::Path) -> (PathBuf, PathBuf) {
        for folder in ["a", "b"] {
            std::fs::create_dir(dir.join(folder)).unwrap();
            std::fs::write(dir.join(folder).join("f"), vec![7u8; SIZE]).unwrap();
        }
        (dir.join("a/f"), dir.join("b/f"))
    }

    /// Imports the file at `path`, as a send of it would, and returns its Collection.
    async fn send(store: &crate::store::Store, db: &Db, path: &std::path::Path) -> Hash {
        let scan = crate::scan::scan(&[path.to_owned()]).unwrap();
        import((***store).clone(), db, &scan.sources).await.unwrap().0
    }

    /// What the store gives back for the one file in the Collection `root`.
    async fn read(store: &crate::store::Store, root: Hash) -> Result<Vec<u8>, String> {
        let collection = Collection::load(root, &***store).await.map_err(|e| e.to_string())?;
        let (_, file) = collection.iter().next().expect("one file");
        let bytes = store.blobs().get_bytes(*file).await.map_err(|e| e.to_string())?;
        Ok(bytes.to_vec())
    }

    /// What goes wrong in iroh-blobs 0.103 (see [`import`]), without anything of ours: a file
    /// imported by reference is gone, the same content is imported from another path that sorts
    /// after it, and the blob cannot be read. A minimal reproduction; if iroh-blobs is changed to
    /// try the paths it holds, this fails, and the replacing of stale files can go.
    #[tokio::test]
    async fn iroh_blobs_cannot_read_a_blob_once_the_first_file_it_refers_to_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::open(&dir.path().join("blobs")).await.unwrap();
        let (first, second) = two_copies(dir.path());
        let add = |path| async {
            let options = AddPathOptions { path, format: BlobFormat::Raw, mode: ImportMode::TryReference };
            store.blobs().add_path_with_opts(options).temp_tag().await.unwrap().hash()
        };
        let hash = add(first.clone()).await;
        std::fs::remove_file(first).unwrap();
        // The same content, from a file that is there.
        assert_eq!(add(second).await, hash);

        let read = store.blobs().get_bytes(hash).await;
        assert!(read.is_err_and(|e| format!("{e:?}").contains("poisoned storage")));
        store.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn content_imported_again_can_be_read_after_the_first_file_is_gone_or_changed() {
        for gone in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let db = db(dir.path()).await;
            let store = crate::store::open(&dir.path().join("blobs")).await.unwrap();
            let (first, second) = two_copies(dir.path());
            send(&store, &db, &first).await;
            if gone {
                std::fs::remove_file(&first).unwrap();
            } else {
                std::fs::write(&first, vec![9u8; SIZE]).unwrap();
            }

            let root = send(&store, &db, &second).await;
            assert_eq!(read(&store, root).await.unwrap(), vec![7u8; SIZE], "gone: {gone}");
            store.shutdown().await.unwrap();
        }
    }

    #[tokio::test]
    async fn a_file_sent_again_unchanged_is_still_referred_to_not_copied() {
        let dir = tempfile::tempdir().unwrap();
        let db = db(dir.path()).await;
        let store = crate::store::open(&dir.path().join("blobs")).await.unwrap();
        let (first, _) = two_copies(dir.path());

        let once = send(&store, &db, &first).await;
        let again = send(&store, &db, &first).await;

        assert_eq!(once, again);
        assert_eq!(read(&store, again).await.unwrap(), vec![7u8; SIZE]);
        let owned = |name: &str| dir.path().join("blobs").join(name);
        assert!(!owned("data").exists() || std::fs::read_dir(owned("data")).unwrap().next().is_none());
        store.shutdown().await.unwrap();
    }
}
