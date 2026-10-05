//! Errors returned by Device commands. Failures inside a running Transfer are reported as
//! a `Failed` state on the event stream instead.

use std::path::PathBuf;

use crate::{
    db::DbError,
    identity::DeviceId,
    store::StoreError,
    transfer::{BatchId, TransferId},
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0} is not a file or folder")]
    NotAFile(PathBuf),
    /// What was chosen to send cannot be sent: a limit is passed, or a name is not usable.
    #[error("{0}")]
    Manifest(#[from] crate::manifest::ManifestError),
    #[error("no pending Offer with Transfer ID {0}")]
    UnknownTransfer(TransferId),
    #[error("{0} is not a folder")]
    NotAFolder(PathBuf),
    #[error("not enough free space: needs {needed} bytes, only {free} free")]
    NotEnoughSpace { needed: u64, free: u64 },
    #[error("Some paths are too long for this save folder")]
    PathsTooLong,
    #[error("Transfer {0} is not running, so it cannot be cancelled")]
    NotRunning(TransferId),
    #[error("Transfer {0} did not expire on this Device, so there is nothing to send again")]
    NothingToResend(TransferId),
    #[error("A Batch needs at least one Receiver.")]
    NoReceivers,
    #[error("{} is chosen twice as a Receiver.", .0.fingerprint())]
    DuplicateReceiver(DeviceId),
    #[error("this Device has no Batch {0}")]
    UnknownBatch(BatchId),
    #[error("Transfer {0} cannot be retried: {1}")]
    NotRetryable(TransferId, &'static str),
    #[error("{} is not a Contact", .0.fingerprint())]
    UnknownContact(DeviceId),
    #[error("{} is already a Contact", .0.fingerprint())]
    AlreadyContact(DeviceId),
    #[error("That is this Device's own ID.")]
    OwnDeviceId,
    #[error("{0}")]
    InvalidContactName(&'static str),
    #[error("A Device Name cannot be empty.")]
    EmptyDeviceName,
    #[error("the Device is shutting down")]
    ShuttingDown,
    #[error("{context}: {source}")]
    Io { context: String, source: std::io::Error },
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Binding the network endpoint.
    #[error("{0}")]
    Network(String),
}

impl Error {
    pub(crate) fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io { context: context.into(), source }
    }

    pub(crate) fn network(context: &str, e: impl std::fmt::Display) -> Self {
        Self::Network(format!("{context}: {e}"))
    }
}
