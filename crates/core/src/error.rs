//! Errors returned by Device commands. Failures inside a running Transfer are reported as
//! a `Failed` state on the event stream instead.

use std::path::PathBuf;

use crate::{db::DbError, store::StoreError, transfer::TransferId};

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
