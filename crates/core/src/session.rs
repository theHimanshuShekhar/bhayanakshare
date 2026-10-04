//! Pieces shared by the Sender and Receiver flows.

use std::fmt::Display;

use tokio::sync::mpsc;

use crate::protocol::{FrameError, Message, PROTOCOL_VERSION};

pub(crate) const LOST: &str = "The other Device went away.";
pub(crate) const UNEXPECTED: &str = "The other Device sent something unexpected.";
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
