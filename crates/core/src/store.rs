//! iroh-blobs fs stores, opened through a process-wide registry.
//!
//! Opening the same store directory twice in one process hangs inside iroh-blobs, so every
//! open goes through [`open`], which refuses a directory that is already open.

use std::{
    collections::HashSet,
    ops::Deref,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
};

use iroh_blobs::store::fs::FsStore;

static OPEN_DIRS: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(Mutex::default);

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("store directory {0} is already open in this process")]
    AlreadyOpen(PathBuf),
    #[error("store directory {path}: {source}")]
    Dir { path: PathBuf, source: std::io::Error },
    #[error("opening store {path}: {reason}")]
    Open { path: PathBuf, reason: String },
    #[error("closing store: {0}")]
    Close(String),
}

impl StoreError {
    /// What went wrong, without the folder it went wrong in: for the log, where a store under a
    /// save folder must not have the folder's path.
    pub(crate) fn cause(&self) -> String {
        match self {
            Self::AlreadyOpen(_) => "the store is already open".to_owned(),
            Self::Dir { source, .. } => source.to_string(),
            Self::Open { reason, .. } => reason.clone(),
            Self::Close(reason) => reason.clone(),
        }
    }
}

/// An open fs store. Holds its directory in the registry until closed or dropped.
#[derive(Debug)]
pub struct Store {
    store: FsStore,
    // Declared after `store` so the directory is released only once the store handle is gone.
    _guard: DirGuard,
}

#[derive(Debug)]
struct DirGuard(PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        OPEN_DIRS.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.0);
    }
}

/// Opens (creating if needed) the fs store in `dir`.
pub async fn open(dir: &Path) -> Result<Store, StoreError> {
    let dir_err = |source| StoreError::Dir { path: dir.to_owned(), source };
    tokio::fs::create_dir_all(dir).await.map_err(dir_err)?;
    let canonical = tokio::fs::canonicalize(dir).await.map_err(dir_err)?;
    if !OPEN_DIRS.lock().unwrap_or_else(|e| e.into_inner()).insert(canonical.clone()) {
        return Err(StoreError::AlreadyOpen(canonical));
    }
    // From here the guard releases the directory on every exit path, including a failed load.
    let guard = DirGuard(canonical.clone());
    let store = FsStore::load(&canonical)
        .await
        .map_err(|e| StoreError::Open { path: canonical, reason: e.to_string() })?;
    Ok(Store { store, _guard: guard })
}

impl Store {
    /// Flushes and shuts the store down, then releases its directory.
    pub async fn shutdown(self) -> Result<(), StoreError> {
        let result = self.store.shutdown().await.map_err(|e| StoreError::Close(e.to_string()));
        drop(self);
        result
    }
}

impl Deref for Store {
    type Target = FsStore;

    fn deref(&self) -> &FsStore {
        &self.store
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_same_directory_cannot_be_opened_twice() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("s");
        let first = open(&dir).await.unwrap();
        // A different spelling of the same directory is caught too.
        let alias = dir.join("..").join("s");
        assert!(matches!(open(&alias).await, Err(StoreError::AlreadyOpen(_))));
        first.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_directory_can_be_reopened_after_shutdown() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("s");
        open(&dir).await.unwrap().shutdown().await.unwrap();
        open(&dir).await.unwrap().shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn different_directories_open_side_by_side() {
        let tmp = tempfile::tempdir().unwrap();
        let a = open(&tmp.path().join("a")).await.unwrap();
        let b = open(&tmp.path().join("b")).await.unwrap();
        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
    }
}
