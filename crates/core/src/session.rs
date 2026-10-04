//! Pieces shared by the Sender and Receiver flows.

use std::{fmt::Display, time::Duration};

use iroh::endpoint::{Connection, SendStream};
use tokio::sync::mpsc;

use crate::{
    device::{Shared, TransferInfo},
    protocol::{FrameError, Message, PROTOCOL_VERSION, write_frame},
    transfer::{Role, TransferState},
};

/// How long to wait for the other side to close the connection after our last frame, so the
/// frame is not cut off by us hanging up first.
pub(crate) const CLOSE_GRACE: Duration = Duration::from_secs(5);

pub(crate) const LOST: &str = "The other Device went away.";
pub(crate) const UNEXPECTED: &str = "The other Device sent something unexpected.";
pub(crate) const BUSY: &str =
    "The other Device already has too many Offers from you waiting for an answer. Try again once it has answered some.";
pub(crate) const INCOMPATIBLE: &str = "The other Device runs an incompatible version of BhayanakShare.";

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

/// Reads the peer's `Hello` and checks that the protocol versions match.
pub(crate) async fn expect_hello(
    incoming: &mut mpsc::Receiver<Result<Message, FrameError>>,
) -> Result<(), Failure> {
    match incoming.recv().await {
        Some(Ok(Message::Hello(hello))) if hello.protocol_version == PROTOCOL_VERSION => Ok(()),
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
/// From the moment this is called the Transfer can no longer be answered or cancelled.
pub(crate) async fn stop(
    sh: &Shared,
    info: &TransferInfo,
    conn: &Connection,
    send: &mut SendStream,
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
