//! Errors returned by Device commands. Failures inside a running Transfer are reported as
//! a `Failed` state on the event stream instead.

use std::path::PathBuf;

use crate::{db::DbError, identity::DeviceId, store::StoreError, transfer::TransferId};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0} is not a file")]
    NotAFile(PathBuf),
    #[error("{0}")]
    InvalidName(#[from] crate::names::NameError),
    #[error("no pending Offer with Transfer ID {0}")]
    UnknownTransfer(TransferId),
    #[error("{0} is not a folder")]
    NotAFolder(PathBuf),
    #[error("not enough free space: needs {needed} bytes, only {free} free")]
    NotEnoughSpace { needed: u64, free: u64 },
    #[error("Transfer {0} is not running, so it cannot be cancelled")]
    NotRunning(TransferId),
    #[error("Transfer {0} did not expire on this Device, so there is nothing to send again")]
    NothingToResend(TransferId),
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
