//! Pieces shared by the Sender and Receiver flows.

use std::{fmt::Display, time::Duration};

use iroh::endpoint::{Connection, SendStream};
use tokio::sync::mpsc;

use crate::{
    clock::UnixMillis,
    device::{Shared, TransferInfo},
    device_name,
    protocol::{FrameError, Message, PROTOCOL_VERSION, write_frame},
    transfer::{Role, STALL_TTL_MS, TransferState},
};

/// How long to wait for the other side to close the connection after our last frame, so the
/// frame is not cut off by us hanging up first.
pub(crate) const CLOSE_GRACE: Duration = Duration::from_secs(5);

pub(crate) const LOST: &str = "The other Device went away.";
pub(crate) const UNEXPECTED: &str = "The other Device sent something unexpected.";
pub(crate) const BUSY: &str =
    "The other Device already has too many Offers from you waiting for an answer. Try again once it has answered some.";
pub(crate) const INCOMPATIBLE: &str = "The other Device runs an incompatible version of BhayanakShare.";
pub(crate) const STALLED: &str = "The Transfer made no progress for 24 hours, so it was given up.";
pub(crate) const RESTARTED: &str = "This Device restarted before the Transfer could carry on.";
pub(crate) const FORGOTTEN: &str = "The other Device no longer has this Transfer.";

/// Frames from the other Device, read on a task so they can be waited on with `select!`.
pub(crate) type Incoming = mpsc::Receiver<Result<Message, FrameError>>;

/// One control connection of a Transfer, after `Hello`. A Transfer has at most one at a time:
/// the one the Sender dialled for the Offer, or a later one the Receiver dialled to resume.
pub(crate) struct Session {
    pub conn: Connection,
    pub send: SendStream,
    pub incoming: Incoming,
}

/// Why a Transfer failed: one plain sentence for the user. The technical cause goes to the log.
#[derive(Debug)]
pub(crate) struct Failure(pub String);

impl Failure {
    pub fn with(reason: &str, cause: impl Display) -> Self {
        tracing::warn!("Transfer failed: {reason} ({cause})");
        Self(reason.to_owned())
    }
}

/// `map_err(fail(REASON))` turns any displayable error into a [`Failure`].
pub(crate) fn fail<E: Display>(reason: &'static str) -> impl Fn(E) -> Failure {
    move |cause| Failure::with(reason, cause)
}

/// Reads the peer's `Hello` and checks that the protocol versions match. Returns the peer's
/// Device Name, cleaned, if it sent one.
pub(crate) async fn expect_hello(incoming: &mut Incoming) -> Result<Option<String>, Failure> {
    match incoming.recv().await {
        Some(Ok(Message::Hello(hello))) if hello.protocol_version == PROTOCOL_VERSION => {
            Ok(device_name::sanitize(&hello.device_name))
        }
        Some(Ok(Message::Hello(hello))) => Err(Failure::with(
            INCOMPATIBLE,
            format!(
                "protocol {} vs {}, app {}",
                hello.protocol_version, PROTOCOL_VERSION, hello.app_version
            ),
        )),
        Some(Ok(_)) => Err(Failure::with(UNEXPECTED, "expected Hello")),
        Some(Err(e)) => Err(Failure::with(LOST, e)),
        None => Err(Failure::with(LOST, "closed before Hello")),
    }
}

/// How a Transfer ended without completing, failing or being declined.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Stop {
    /// This Device's user cancelled it.
    Cancelled,
    /// The other Device sent `Cancel`.
    PeerCancelled,
    /// Nobody answered the Offer in time, by this Device's clock.
    Expired,
    /// The other Device sent `Expired`.
    PeerExpired,
}

/// Records and announces how the Transfer ended and settles the connection: a Device that
/// ends it tells the other and waits for it to hang up, one that is told hangs up itself.
/// Without a `session` (the connection is down and being redialled) nobody can be told. From
/// the moment this is called the Transfer can no longer be answered or cancelled.
pub(crate) async fn stop(
    sh: &Shared,
    info: &TransferInfo,
    session: Option<&mut Session>,
    how: Stop,
) {
    let peer = match info.role {
        Role::Sender => Role::Receiver,
        Role::Receiver => Role::Sender,
    };
    let (state, tell) = match how {
        Stop::Cancelled => (TransferState::Cancelled { by: info.role }, Some(Message::Cancel)),
        Stop::PeerCancelled => (TransferState::Cancelled { by: peer }, None),
        Stop::Expired => (TransferState::Expired, Some(Message::Expired)),
        Stop::PeerExpired => (TransferState::Expired, None),
    };
    sh.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&info.id);
    sh.untrack(info.id);
    sh.transition(info, state).await;
    let Some(Session { conn, send, .. }) = session else { return };
    match tell {
        Some(msg) => {
            let _ = write_frame(send, &msg).await;
            let _ = send.finish();
            let _ = tokio::time::timeout(CLOSE_GRACE, conn.closed()).await;
        }
        None => {
            let _ = send.finish();
            conn.close(0u32.into(), b"done");
        }
    }
}

/// When a Transfer last made progress; the 24-hour stall expiry runs from there (spec
/// section 4). Redialling alone is not progress, or a Transfer that connects and fails again
/// and again would never expire.
pub(crate) struct Stall {
    last: UnixMillis,
    saved: UnixMillis,
}

/// How often progress is written to the database. The limit is 24 hours, so a minute's
/// lag does not matter, and SQLite is not hit on every progress report.
const SAVE_EVERY_MS: i64 = 60_000;

impl Stall {
    pub fn new(last_progress: UnixMillis) -> Self {
        Self { last: last_progress, saved: last_progress }
    }

    /// When the Transfer last made progress.
    pub fn last(&self) -> UnixMillis {
        self.last
    }

    /// When the Transfer gives up, if nothing more happens.
    pub fn deadline(&self) -> UnixMillis {
        self.last + STALL_TTL_MS
    }

    /// Notes progress at `now`; true when it is time to write it to the database.
    pub fn progressed(&mut self, now: UnixMillis) -> bool {
        self.last = now;
        let save = now - self.saved >= SAVE_EVERY_MS;
        if save {
            self.saved = now;
        }
        save
    }
}

/// Notes that the Transfer made progress, and saves it now and then so a restarted Device
/// counts its 24 hours from the right moment.
pub(crate) async fn made_progress(sh: &Shared, info: &TransferInfo, stall: &mut Stall) {
    if stall.progressed(sh.now()) {
        save_progress(sh, info, stall).await;
    }
}

/// Writes the time of the last progress to the database, e.g. when the connection is lost.
pub(crate) async fn save_progress(sh: &Shared, info: &TransferInfo, stall: &mut Stall) {
    stall.saved = stall.last;
    if let Err(e) = sh.db.set_progress_at(info.id, stall.last).await {
        tracing::warn!(transfer = %info.id, "could not record progress: {e}");
    }
}

/// How long a Receiver waits before its `attempt`th try to reach the Sender again, counting
/// from 0: at once, then fast, then about once a minute (spec section 4). Real time, not the
/// injected clock: this paces network attempts, it is not part of the Transfer's lifecycle.
pub(crate) fn retry_delay(attempt: u32) -> Duration {
    const FAST_MS: [u64; 8] = [0, 250, 500, 1_000, 2_000, 5_000, 10_000, 30_000];
    Duration::from_millis(FAST_MS.get(attempt as usize).copied().unwrap_or(60_000))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redialling_starts_fast_and_settles_at_about_once_a_minute() {
        let delays: Vec<Duration> = (0..12).map(retry_delay).collect();
        assert_eq!(delays[0], Duration::ZERO);
        assert!(delays.windows(2).all(|w| w[0] <= w[1]), "never speeds up: {delays:?}");
        assert!(delays[1] < Duration::from_secs(1), "the first retries are quick");
        assert_eq!(delays[8..], [Duration::from_secs(60); 4]);
    }

    #[test]
    fn a_transfer_gives_up_24_hours_after_its_last_progress() {
        let mut stall = Stall::new(1_000);
        assert_eq!(stall.deadline(), 1_000 + 24 * 60 * 60 * 1000);
        stall.progressed(5_000);
        assert_eq!(stall.deadline(), 5_000 + 24 * 60 * 60 * 1000);
    }

    #[test]
    fn progress_is_saved_at_most_once_a_minute() {
        let mut stall = Stall::new(0);
        assert!(!stall.progressed(100));
        assert!(!stall.progressed(59_999));
        assert!(stall.progressed(60_000));
        assert!(!stall.progressed(60_001));
        assert!(stall.progressed(120_000));
    }
}
