//! The Sender side of a Transfer.
//!
//! Dial the Receiver, say Hello, send the Offer straight away, and hash the file while the
//! Receiver decides (the file is imported by reference, so nothing is copied). Once hashing
//! is done and the Receiver has accepted, allow that Receiver to fetch (see `gate`) and send
//! `HashReady`; the Receiver then pulls the content over iroh-blobs from this Device's global
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

use std::{path::PathBuf, sync::Arc};

use iroh::EndpointId;
use iroh_blobs::{
    BlobFormat, Hash, HashAndFormat,
    api::{
        TempTag,
        blobs::{AddPathOptions, ImportMode},
    },
    format::collection::Collection,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    clock::{UnixMillis, sleep_until},
    db::Source,
    device::{DeviceAddr, Shared, TransferInfo},
    gate::Grant,
    identity::DeviceId,
    protocol::{self, Message, Offer, write_frame},
    session::{
        BUSY, CLOSE_GRACE, Failure, LOST, STALLED, Session, Stall, Stop, UNEXPECTED, expect_hello,
        fail, made_progress, save_progress, stop,
    },
    transfer::{Role, TransferId, TransferState},
};

pub(crate) async fn run(
    sh: Arc<Shared>,
    info: TransferInfo,
    to: DeviceAddr,
    path: PathBuf,
    cancel: CancellationToken,
) {
    let outcome = tokio::select! {
        () = sh.cancel.cancelled() => None,
        outcome = flow(&sh, &info, &to, path, &cancel) => Some(outcome),
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
    let task_sh = sh.clone();
    sh.tasks.spawn(async move {
        let sh = task_sh;
        let root = Hash::from_bytes(root);
        let stall = Stall::new(progress_at);
        let outcome = tokio::select! {
            () = sh.cancel.cancelled() => None,
            outcome = transferring(&sh, &info, root, None, resumes, &cancel, stall) => Some(outcome),
        };
        sh.untrack(info.id);
        if let Some(Err(Failure(reason))) = outcome {
            sh.transition(&info, TransferState::Failed { reason }).await;
        }
    });
}

/// Dials the Receiver and exchanges `Hello`.
async fn connect(sh: &Shared, to: &DeviceAddr) -> Result<(Session, Option<String>), Failure> {
    let conn = sh
        .endpoint
        .connect(to.to_endpoint_addr(), protocol::ALPN)
        .await
        .map_err(fail("Could not reach the receiving Device."))?;
    let (mut send, recv) = conn
        .open_bi()
        .await
        .map_err(fail("Could not reach the receiving Device."))?;
    let mut incoming = protocol::spawn_reader(recv);

    write_frame(&mut send, &Message::Hello(protocol::Hello::named(sh.device_name().await)))
        .await
        .map_err(fail(LOST))?;
    let peer_name = expect_hello(&mut incoming).await?;
    Ok((Session { conn, send, incoming }, peer_name))
}

async fn flow(
    sh: &Arc<Shared>,
    info: &TransferInfo,
    to: &DeviceAddr,
    path: PathBuf,
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
    // From here on the Transfer's events carry what the Receiver calls itself.
    let info = &TransferInfo { peer_name, ..info.clone() };
    write_frame(
        &mut session.send,
        &Message::Offer(Offer {
            transfer_id: *info.id.as_bytes(),
            name: info.name.clone(),
            size: info.size,
        }),
    )
    .await
    .map_err(fail(LOST))?;

    // Hash while the Receiver decides. Not spawned: leaving this function drops it, which
    // abandons the hashing of a declined or failed Transfer.
    let source = path.clone();
    let mut import = Some(Box::pin(import(sh.blobs.clone(), path, info.name.clone())));
    // The temp tags keep the imported blobs alive for as long as the Transfer runs.
    let mut _keep_alive = Vec::new();
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
                    sh.transition(info, TransferState::Accepted).await;
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
                    lapse(sh, info, to, &source);
                    stop(sh, info, Some(&mut session), Stop::PeerExpired).await;
                    return Ok(());
                }
                Some(Ok(Message::Busy)) if !accepted => {
                    return Err(Failure::with(BUSY, "the Receiver answered Busy"));
                }
                Some(Ok(_)) => return Err(Failure::with(UNEXPECTED, "out-of-order message")),
                Some(Err(e)) => return Err(Failure::with(LOST, e)),
                None => return Err(Failure::with(LOST, "stream ended")),
            },
            () = cancel.cancelled() => {
                stop(sh, info, Some(&mut session), Stop::Cancelled).await;
                return Ok(());
            }
            () = &mut expiry, if !accepted => {
                lapse(sh, info, to, &source);
                stop(sh, info, Some(&mut session), Stop::Expired).await;
                return Ok(());
            }
            done = async { import.as_mut().expect("guarded by the if below").await },
                if import.is_some() =>
            {
                import = None;
                let (hash, tags) = done?;
                _keep_alive = tags;
                root = Some(hash);
            }
        }
    };

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
    transferring(sh, info, root, Some((session, grant)), resumes, cancel, Stall::new(now)).await
}

/// Serves the Transfer until the Receiver reports it finished: with the Receiver connected it
/// follows its progress; when the connection is lost it waits, grant dropped, for the
/// Receiver to dial back. `live` is the control connection of a Transfer that just started;
/// a recovered one has none.
async fn transferring(
    sh: &Arc<Shared>,
    info: &TransferInfo,
    root: Hash,
    live: Option<(Session, Grant)>,
    mut resumes: Resumes,
    cancel: &CancellationToken,
    mut stall: Stall,
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
        if let Some(back) = returned.take() {
            if let Some((session, fresh)) = welcome(sh, info, root, back, &mut keep_alive).await? {
                live = Some(session);
                grant = Some(fresh);
                if !announced {
                    announced = true;
                    sh.transition(info, TransferState::Transferring).await;
                }
            }
            // Otherwise the Receiver went away again before it was answered.
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
                save_progress(sh, info, &mut stall).await;
            }
            // The Receiver is back before we noticed it had gone: the old connection is dead.
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
        let _ = write_frame(&mut session.send, &Message::Failed { reason: reason.clone() }).await;
        let _ = session.send.finish();
        let _ = tokio::time::timeout(CLOSE_GRACE, session.conn.closed()).await;
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

/// Before serving again: the files must be as they were when the Offer was made, and the
/// store must still hold their content. It may not if the Device was killed just after hashing
/// it, as the store commits a moment later; the files are unchanged, so hashing them again
/// gives the same hash.
async fn check_files(
    sh: &Shared,
    info: &TransferInfo,
    root: Hash,
    keep_alive: &mut Vec<TempTag>,
) -> Result<(), Failure> {
    let changed = || {
        Failure::with(
            &format!("A file changed on the sending Device: {}", info.name),
            "source check failed",
        )
    };
    let sources = sh.db.sources(info.id).await.map_err(fail("Could not look up the Transfer."))?;
    let [source] = sources.as_slice() else { return Err(changed()) };
    if !is_unchanged(source).await {
        return Err(changed());
    }
    let content = HashAndFormat::hash_seq(root);
    if !sh.blobs.remote().local(content).await.is_ok_and(|local| local.is_complete()) {
        let (hash, tags) = import(sh.blobs.clone(), source.path.clone(), info.name.clone()).await?;
        if hash != root {
            return Err(changed());
        }
        keep_alive.extend(tags);
    }
    Ok(())
}

async fn is_unchanged(source: &Source) -> bool {
    match tokio::fs::metadata(&source.path).await {
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
fn lapse(sh: &Shared, info: &TransferInfo, to: &DeviceAddr, path: &std::path::Path) {
    sh.expired
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(info.id, (to.clone(), path.to_owned()));
}

/// Imports the file by reference into the global store and wraps it in a one-entry
/// Collection, the shape the Receiver fetches (folders will add entries).
async fn import(
    store: iroh_blobs::api::Store,
    path: PathBuf,
    name: String,
) -> Result<(Hash, Vec<TempTag>), Failure> {
    let file = store
        .blobs()
        .add_path_with_opts(AddPathOptions {
            path,
            format: BlobFormat::Raw,
            mode: ImportMode::TryReference,
        })
        .temp_tag()
        .await
        .map_err(fail("Could not read the file."))?;
    let collection = Collection::from_iter([(name, file.hash())]);
    let root = collection
        .store(&store)
        .await
        .map_err(fail("Could not prepare the file for sending."))?;
    Ok((root.hash(), vec![file, root]))
}
