//! Where a Device's secret key lives: the OS secret store when there is one, else a 0600 file.
//!
//! The one rule behind all of this is that the Device ID never changes without the user
//! asking. Starting up looks at three things: the `key-location` marker beside the fallback
//! file (where the key was last put), the secret store, and the file. A new key is made only
//! when none of them holds one and the marker does not promise there is one; a locked
//! keychain or a stopped daemon on a Device whose marker says "os-store" is an error, not a
//! reason to start over.

use std::{
    io,
    path::{Path, PathBuf},
};

use iroh::SecretKey;
use zeroize::Zeroizing;

use crate::identity;

const SERVICE: &str = "bhayanakshare";
const USER: &str = "device-secret-key";
const MARKER_FILE: &str = "key-location";

/// Why the OS secret store could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StoreError {
    /// This system has no secret store (no Secret Service on the session bus, say).
    Unavailable(String),
    /// There is one, and this failed: locked, access denied, a daemon that stopped.
    Failed(String),
}

/// The secret store, for one fixed entry. A seam so the decisions below can be tested.
pub(crate) trait SecretStore {
    /// `Ok(None)`: the store works and has no such entry.
    fn get(&self) -> Result<Option<Zeroizing<Vec<u8>>>, StoreError>;
    fn set(&self, secret: &[u8]) -> Result<(), StoreError>;
    fn delete(&self) -> Result<(), StoreError>;
}

/// The platform's secret store: Secret Service on Linux, Keychain on macOS, Credential Manager
/// on Windows.
pub(crate) struct OsSecretStore;

impl OsSecretStore {
    fn entry() -> Result<keyring::Entry, StoreError> {
        keyring::Entry::new(SERVICE, USER).map_err(|e| match e {
            keyring::Error::NoDefaultStore => StoreError::Unavailable(match keyring::Entry::store_status() {
                Err(why) => why.to_string(),
                Ok(()) => e.to_string(),
            }),
            e => StoreError::Failed(e.to_string()),
        })
    }
}

impl SecretStore for OsSecretStore {
    fn get(&self) -> Result<Option<Zeroizing<Vec<u8>>>, StoreError> {
        match Self::entry()?.get_secret() {
            Ok(secret) => Ok(Some(Zeroizing::new(secret))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(StoreError::Failed(e.to_string())),
        }
    }

    fn set(&self, secret: &[u8]) -> Result<(), StoreError> {
        Self::entry()?.set_secret(secret).map_err(|e| StoreError::Failed(e.to_string()))
    }

    fn delete(&self) -> Result<(), StoreError> {
        match Self::entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(StoreError::Failed(e.to_string())),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    /// The OS secret store holds this Device's key and cannot be used just now, or its state is
    /// unknown on a first start. Nothing was changed.
    #[error(
        "The OS secret store is not available, so this Device's secret key cannot be read ({0}). \
         Unlock it or start its service, then start BhayanakShare again."
    )]
    StoreUnavailable(String),
    /// The marker says the key is in the secret store and the store says it is not.
    #[error(
        "The OS secret store no longer has this Device's secret key. Restore it by importing an \
         identity export, or delete {} to start with a new Device ID.",
        .marker.display()
    )]
    MissingFromStore { marker: PathBuf },
    /// The marker says the key is in a file and it is not there.
    #[error(
        "The secret key file {} is missing. Put it back, or delete {} to start with a new Device ID.",
        .file.display(),
        .marker.display()
    )]
    MissingKeyFile { file: PathBuf, marker: PathBuf },
    /// Two different keys, and nothing that says which one is this Device's.
    #[error(
        "The OS secret store and {} hold different secret keys, and {} does not say which is this \
         Device's. Remove the one that is not.",
        .file.display(),
        .marker.display()
    )]
    Conflict { file: PathBuf, marker: PathBuf },
    #[error("The stored secret key is not 32 bytes.")]
    Corrupt,
    #[error("{0}")]
    Io(#[from] io::Error),
}

/// Where the marker says the key is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Location {
    OsStore,
    File,
}

pub(crate) fn marker_path(fallback: &Path) -> PathBuf {
    fallback.with_file_name(MARKER_FILE)
}

fn read_marker(fallback: &Path) -> Result<Option<Location>, KeyError> {
    match std::fs::read_to_string(marker_path(fallback)) {
        Ok(text) => match text.trim() {
            "os-store" => Ok(Some(Location::OsStore)),
            "file" => Ok(Some(Location::File)),
            _ => Err(io::Error::new(io::ErrorKind::InvalidData, "key-location is not \"os-store\" or \"file\"").into()),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn write_marker(fallback: &Path, location: Location) -> Result<(), KeyError> {
    let text = match location {
        Location::OsStore => "os-store\n",
        Location::File => "file\n",
    };
    Ok(identity::write_replacing(&marker_path(fallback), text.as_bytes())?)
}

/// What the secret store holds, as startup sees it.
enum Stored {
    Key(SecretKey),
    Missing,
    NoStore,
    Failed(String),
}

fn read_store(store: &dyn SecretStore) -> Result<Stored, KeyError> {
    Ok(match store.get() {
        Ok(Some(bytes)) => Stored::Key(SecretKey::try_from(bytes.as_slice()).map_err(|_| KeyError::Corrupt)?),
        Ok(None) => Stored::Missing,
        Err(StoreError::Unavailable(_)) => Stored::NoStore,
        Err(StoreError::Failed(why)) => Stored::Failed(why),
    })
}

/// Puts `key` in the store and reads it back. Anything but an equal key is a failure, and
/// leaves nothing behind.
fn put_in_store(store: &dyn SecretStore, key: &SecretKey) -> bool {
    let bytes = Zeroizing::new(key.to_bytes());
    let stored = store.set(bytes.as_slice()).is_ok()
        && matches!(store.get(), Ok(Some(back)) if back.as_slice() == bytes.as_slice());
    if !stored {
        let _ = store.delete();
    }
    stored
}

/// Moves the key from `fallback` into the store, deleting the file only once the store gives
/// back what was put in. When it cannot, the key stays in the file, which the marker says.
fn migrate(store: &dyn SecretStore, fallback: &Path, key: &SecretKey) -> Result<(), KeyError> {
    if put_in_store(store, key) {
        write_marker(fallback, Location::OsStore)?;
        identity::remove_key_file(fallback);
    } else {
        tracing::warn!("could not move the secret key into the OS secret store; keeping it in {}", fallback.display());
        write_marker(fallback, Location::File)?;
    }
    Ok(())
}

/// Loads the key from wherever it is, making one on a first start.
pub(crate) fn load_or_create(store: &dyn SecretStore, fallback: &Path) -> Result<SecretKey, KeyError> {
    let marker = read_marker(fallback)?;
    let stored = read_store(store)?;
    let file = identity::read_key_file(fallback)?;
    let unavailable = |why: &str| KeyError::StoreUnavailable(why.to_owned());
    let missing_file = || KeyError::MissingKeyFile { file: fallback.to_owned(), marker: marker_path(fallback) };

    match (marker, stored, file) {
        // Said to be in the store: there or nowhere. A file left over from a move that was
        // interrupted goes once it is shown to be the same key.
        (Some(Location::OsStore), Stored::Key(key), file) => {
            if file.is_some_and(|f| f.to_bytes() == key.to_bytes()) {
                identity::remove_key_file(fallback);
            }
            Ok(key)
        }
        (Some(Location::OsStore), Stored::Missing, _) => {
            Err(KeyError::MissingFromStore { marker: marker_path(fallback) })
        }
        (Some(Location::OsStore), Stored::NoStore, _) => Err(unavailable("there is no secret store on this system")),
        (Some(Location::OsStore), Stored::Failed(why), _) => Err(unavailable(&why)),

        // Said to be in the file: it is the file's key even if the store has another (an old
        // entry, say); it moves to the store when the store has room for it.
        (Some(Location::File), _, None) => Err(missing_file()),
        (Some(Location::File), Stored::Missing, Some(key)) => {
            migrate(store, fallback, &key)?;
            Ok(key)
        }
        (Some(Location::File), Stored::Key(other), Some(key)) => {
            if other.to_bytes() == key.to_bytes() {
                migrate(store, fallback, &key)?;
            } else {
                tracing::warn!("the OS secret store holds another key than {}; using the file's, as key-location says", fallback.display());
            }
            Ok(key)
        }
        (Some(Location::File), Stored::NoStore | Stored::Failed(_), Some(key)) => Ok(key),

        // No marker: an install from before the store, or a data folder that was cleared.
        (None, Stored::Key(key), Some(file)) if key.to_bytes() != file.to_bytes() => {
            Err(KeyError::Conflict { file: fallback.to_owned(), marker: marker_path(fallback) })
        }
        (None, Stored::Key(key), file) => {
            write_marker(fallback, Location::OsStore)?;
            if file.is_some() {
                identity::remove_key_file(fallback);
            }
            Ok(key)
        }
        (None, Stored::Missing, Some(key)) => {
            migrate(store, fallback, &key)?;
            Ok(key)
        }
        (None, Stored::NoStore | Stored::Failed(_), Some(key)) => {
            write_marker(fallback, Location::File)?;
            Ok(key)
        }
        // Nowhere: a first start, unless the store has a key it will not show.
        (None, Stored::Missing, None) => {
            let key = SecretKey::generate();
            if put_in_store(store, &key) {
                write_marker(fallback, Location::OsStore)?;
            } else {
                identity::write_new_key_file(fallback, &key.to_bytes())?;
                write_marker(fallback, Location::File)?;
            }
            Ok(key)
        }
        (None, Stored::NoStore, None) => {
            let key = SecretKey::generate();
            identity::write_new_key_file(fallback, &key.to_bytes())?;
            write_marker(fallback, Location::File)?;
            Ok(key)
        }
        (None, Stored::Failed(why), None) => Err(unavailable(&why)),
    }
}

/// Makes `key` this Device's key, in the place the marker names. Nothing is changed unless
/// the new key can be stored.
pub(crate) fn replace(store: &dyn SecretStore, fallback: &Path, key: &SecretKey) -> Result<(), KeyError> {
    match read_marker(fallback)? {
        Some(Location::OsStore) => {
            let bytes = Zeroizing::new(key.to_bytes());
            store.set(bytes.as_slice()).map_err(|e| KeyError::StoreUnavailable(store_reason(e)))?;
            match store.get() {
                Ok(Some(back)) if back.as_slice() == bytes.as_slice() => Ok(()),
                Ok(_) => Err(KeyError::StoreUnavailable("the store did not keep the key".to_owned())),
                Err(e) => Err(KeyError::StoreUnavailable(store_reason(e))),
            }
        }
        Some(Location::File) | None => {
            identity::write_replacing(fallback, &key.to_bytes())?;
            write_marker(fallback, Location::File)
        }
    }
}

fn store_reason(e: StoreError) -> String {
    match e {
        StoreError::Unavailable(why) | StoreError::Failed(why) => why,
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use super::*;

    /// A stand-in secret store a test can break in each way a real one can be broken.
    #[derive(Clone, Default)]
    struct Fake(Rc<RefCell<FakeState>>);

    #[derive(Default)]
    struct FakeState {
        entry: Option<Vec<u8>>,
        /// Every call says there is no store on this system.
        absent: bool,
        /// Every call fails as a locked keychain does.
        locked: bool,
        /// `set` fails, `get` still works.
        read_only: bool,
        /// What is set is not what `get` gives back.
        forgetful: bool,
        sets: usize,
    }

    impl Fake {
        fn holding(key: &SecretKey) -> Self {
            let fake = Self::default();
            fake.0.borrow_mut().entry = Some(key.to_bytes().to_vec());
            fake
        }
        fn absent() -> Self {
            let fake = Self::default();
            fake.0.borrow_mut().absent = true;
            fake
        }
        fn locked() -> Self {
            let fake = Self::default();
            fake.0.borrow_mut().locked = true;
            fake
        }
        fn entry(&self) -> Option<Vec<u8>> {
            self.0.borrow().entry.clone()
        }
        fn set_locked(&self, on: bool) {
            self.0.borrow_mut().locked = on;
        }
        fn gate(&self) -> Result<(), StoreError> {
            let s = self.0.borrow();
            if s.absent {
                Err(StoreError::Unavailable("no Secret Service".into()))
            } else if s.locked {
                Err(StoreError::Failed("the keychain is locked".into()))
            } else {
                Ok(())
            }
        }
    }

    impl SecretStore for Fake {
        fn get(&self) -> Result<Option<Zeroizing<Vec<u8>>>, StoreError> {
            self.gate()?;
            Ok(self.0.borrow().entry.clone().map(Zeroizing::new))
        }
        fn set(&self, secret: &[u8]) -> Result<(), StoreError> {
            self.gate()?;
            let mut s = self.0.borrow_mut();
            if s.read_only {
                return Err(StoreError::Failed("read-only".into()));
            }
            s.sets += 1;
            s.entry = Some(if s.forgetful { vec![0; 32] } else { secret.to_vec() });
            Ok(())
        }
        fn delete(&self) -> Result<(), StoreError> {
            self.gate()?;
            self.0.borrow_mut().entry = None;
            Ok(())
        }
    }

    struct Dir(tempfile::TempDir);

    impl Dir {
        fn new() -> Self {
            Self(tempfile::tempdir().unwrap())
        }
        fn file(&self) -> PathBuf {
            self.0.path().join("data").join("secret.key")
        }
        fn marker(&self) -> Option<String> {
            std::fs::read_to_string(marker_path(&self.file())).ok().map(|s| s.trim().to_owned())
        }
        fn put_file(&self, key: &SecretKey) {
            identity::write_new_key_file(&self.file(), &key.to_bytes()).unwrap();
        }
        fn put_marker(&self, text: &str) {
            std::fs::create_dir_all(self.file().parent().unwrap()).unwrap();
            std::fs::write(marker_path(&self.file()), text).unwrap();
        }
        fn file_key(&self) -> Option<[u8; 32]> {
            std::fs::read(self.file()).ok().map(|b| b.try_into().unwrap())
        }
        fn load(&self, store: &Fake) -> Result<SecretKey, KeyError> {
            load_or_create(store, &self.file())
        }
    }

    fn key(n: u8) -> SecretKey {
        SecretKey::from_bytes(&[n; 32])
    }

    // A first start.

    #[test]
    fn a_first_start_keeps_the_key_in_the_store_and_nowhere_else() {
        let (dir, store) = (Dir::new(), Fake::default());
        let made = dir.load(&store).unwrap();
        assert_eq!(store.entry().unwrap(), made.to_bytes());
        assert_eq!(dir.marker().as_deref(), Some("os-store"));
        assert!(dir.file_key().is_none());
        assert_eq!(dir.load(&store).unwrap().to_bytes(), made.to_bytes());
    }

    #[test]
    fn a_first_start_with_no_store_on_the_system_keeps_the_key_in_a_private_file() {
        let (dir, store) = (Dir::new(), Fake::absent());
        let made = dir.load(&store).unwrap();
        assert_eq!(dir.file_key().unwrap(), made.to_bytes());
        assert_eq!(dir.marker().as_deref(), Some("file"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(dir.file()).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert_eq!(dir.load(&store).unwrap().to_bytes(), made.to_bytes());
    }

    #[test]
    fn a_first_start_falls_back_to_the_file_when_the_store_will_not_take_the_key() {
        let (dir, store) = (Dir::new(), Fake::default());
        store.0.borrow_mut().read_only = true;
        let made = dir.load(&store).unwrap();
        assert_eq!(dir.file_key().unwrap(), made.to_bytes());
        assert_eq!(dir.marker().as_deref(), Some("file"));
        assert!(store.entry().is_none());
    }

    #[test]
    fn a_first_start_falls_back_to_the_file_when_the_store_forgets_the_key() {
        let (dir, store) = (Dir::new(), Fake::default());
        store.0.borrow_mut().forgetful = true;
        let made = dir.load(&store).unwrap();
        assert_eq!(dir.file_key().unwrap(), made.to_bytes());
        assert_eq!(dir.marker().as_deref(), Some("file"));
        assert!(store.entry().is_none(), "a key the store cannot give back is not left in it");
    }

    #[test]
    fn a_first_start_with_a_store_that_errors_makes_no_key_at_all() {
        // A locked keychain may hold the key of a Device whose data folder was cleared.
        let (dir, store) = (Dir::new(), Fake::locked());
        assert!(matches!(dir.load(&store), Err(KeyError::StoreUnavailable(_))));
        assert!(dir.file_key().is_none());
        assert!(dir.marker().is_none());
    }

    // An install from before the store: a file, no marker.

    #[test]
    fn a_key_file_moves_into_the_store_and_the_file_goes() {
        let (dir, store) = (Dir::new(), Fake::default());
        dir.put_file(&key(1));
        let loaded = dir.load(&store).unwrap();
        assert_eq!(loaded.to_bytes(), key(1).to_bytes());
        assert_eq!(store.entry().unwrap(), key(1).to_bytes());
        assert_eq!(dir.marker().as_deref(), Some("os-store"));
        assert!(dir.file_key().is_none());
        // The Device ID is the same on the start after the move.
        assert_eq!(dir.load(&store).unwrap().to_bytes(), key(1).to_bytes());
    }

    #[test]
    fn the_file_stays_when_the_store_does_not_give_the_moved_key_back() {
        let (dir, store) = (Dir::new(), Fake::default());
        store.0.borrow_mut().forgetful = true;
        dir.put_file(&key(1));
        assert_eq!(dir.load(&store).unwrap().to_bytes(), key(1).to_bytes());
        assert_eq!(dir.file_key().unwrap(), key(1).to_bytes());
        assert_eq!(dir.marker().as_deref(), Some("file"));
        assert!(store.entry().is_none());
    }

    #[test]
    fn the_file_stays_when_the_store_will_not_take_the_moved_key() {
        let (dir, store) = (Dir::new(), Fake::default());
        store.0.borrow_mut().read_only = true;
        dir.put_file(&key(1));
        assert_eq!(dir.load(&store).unwrap().to_bytes(), key(1).to_bytes());
        assert_eq!(dir.file_key().unwrap(), key(1).to_bytes());
        assert_eq!(dir.marker().as_deref(), Some("file"));
    }

    #[test]
    fn a_key_file_is_used_where_there_is_no_store() {
        let (dir, store) = (Dir::new(), Fake::absent());
        dir.put_file(&key(1));
        assert_eq!(dir.load(&store).unwrap().to_bytes(), key(1).to_bytes());
        assert_eq!(dir.file_key().unwrap(), key(1).to_bytes());
        assert_eq!(dir.marker().as_deref(), Some("file"));
    }

    #[test]
    fn a_key_file_is_used_untouched_while_the_store_is_failing() {
        let (dir, store) = (Dir::new(), Fake::locked());
        dir.put_file(&key(1));
        assert_eq!(dir.load(&store).unwrap().to_bytes(), key(1).to_bytes());
        assert_eq!(dir.file_key().unwrap(), key(1).to_bytes());
        assert_eq!(dir.marker().as_deref(), Some("file"));
        assert_eq!(store.0.borrow().sets, 0);
    }

    // The data folder was cleared, or a file and the store disagree: no marker.

    #[test]
    fn a_key_in_the_store_is_this_devices_even_when_the_data_folder_is_new() {
        let (dir, store) = (Dir::new(), Fake::holding(&key(7)));
        assert_eq!(dir.load(&store).unwrap().to_bytes(), key(7).to_bytes());
        assert_eq!(dir.marker().as_deref(), Some("os-store"));
    }

    #[test]
    fn the_same_key_in_the_store_and_a_file_leaves_only_the_store() {
        let (dir, store) = (Dir::new(), Fake::holding(&key(7)));
        dir.put_file(&key(7));
        assert_eq!(dir.load(&store).unwrap().to_bytes(), key(7).to_bytes());
        assert!(dir.file_key().is_none());
        assert_eq!(dir.marker().as_deref(), Some("os-store"));
    }

    #[test]
    fn two_different_keys_and_no_marker_is_an_error_that_changes_nothing() {
        let (dir, store) = (Dir::new(), Fake::holding(&key(7)));
        dir.put_file(&key(1));
        assert!(matches!(dir.load(&store), Err(KeyError::Conflict { .. })));
        assert_eq!(dir.file_key().unwrap(), key(1).to_bytes());
        assert_eq!(store.entry().unwrap(), key(7).to_bytes());
        assert!(dir.marker().is_none());
    }

    // The marker says os-store.

    #[test]
    fn the_marker_says_os_store_and_the_store_has_the_key() {
        let (dir, store) = (Dir::new(), Fake::holding(&key(7)));
        dir.put_marker("os-store\n");
        assert_eq!(dir.load(&store).unwrap().to_bytes(), key(7).to_bytes());
    }

    #[test]
    fn a_failing_or_absent_store_never_makes_a_new_key_when_the_marker_says_os_store() {
        for store in [Fake::locked(), Fake::absent()] {
            let dir = Dir::new();
            dir.put_marker("os-store\n");
            assert!(matches!(dir.load(&store), Err(KeyError::StoreUnavailable(_))));
            assert!(dir.file_key().is_none(), "a key was made in a file");
            assert!(store.entry().is_none(), "a key was made in the store");
            assert_eq!(dir.marker().as_deref(), Some("os-store"));
        }
    }

    #[test]
    fn a_store_that_comes_back_gives_the_same_key() {
        let (dir, store) = (Dir::new(), Fake::default());
        let made = dir.load(&store).unwrap();
        store.set_locked(true);
        assert!(dir.load(&store).is_err());
        store.set_locked(false);
        assert_eq!(dir.load(&store).unwrap().to_bytes(), made.to_bytes());
    }

    #[test]
    fn a_store_that_lost_the_key_is_an_error_when_the_marker_says_os_store() {
        let (dir, store) = (Dir::new(), Fake::default());
        dir.put_marker("os-store\n");
        let err = dir.load(&store).unwrap_err();
        assert!(matches!(err, KeyError::MissingFromStore { .. }));
        assert!(err.to_string().contains("key-location"), "the way out is in the message: {err}");
        assert!(dir.file_key().is_none());
        assert!(store.entry().is_none());
    }

    #[test]
    fn a_file_left_by_an_interrupted_move_goes_when_it_is_the_stored_key() {
        let (dir, store) = (Dir::new(), Fake::holding(&key(7)));
        dir.put_marker("os-store\n");
        dir.put_file(&key(7));
        assert_eq!(dir.load(&store).unwrap().to_bytes(), key(7).to_bytes());
        assert!(dir.file_key().is_none());
    }

    #[test]
    fn a_file_that_is_not_the_stored_key_is_left_alone() {
        let (dir, store) = (Dir::new(), Fake::holding(&key(7)));
        dir.put_marker("os-store\n");
        dir.put_file(&key(1));
        assert_eq!(dir.load(&store).unwrap().to_bytes(), key(7).to_bytes());
        assert_eq!(dir.file_key().unwrap(), key(1).to_bytes());
    }

    // The marker says file.

    #[test]
    fn the_marker_says_file_and_the_file_is_used_and_moves_once_a_store_appears() {
        let (dir, store) = (Dir::new(), Fake::default());
        dir.put_marker("file\n");
        dir.put_file(&key(1));
        assert_eq!(dir.load(&store).unwrap().to_bytes(), key(1).to_bytes());
        assert_eq!(store.entry().unwrap(), key(1).to_bytes());
        assert_eq!(dir.marker().as_deref(), Some("os-store"));
        assert!(dir.file_key().is_none());
    }

    #[test]
    fn the_marker_says_file_so_another_key_in_the_store_does_not_win() {
        let (dir, store) = (Dir::new(), Fake::holding(&key(7)));
        dir.put_marker("file\n");
        dir.put_file(&key(1));
        assert_eq!(dir.load(&store).unwrap().to_bytes(), key(1).to_bytes());
        assert_eq!(dir.file_key().unwrap(), key(1).to_bytes());
        assert_eq!(store.entry().unwrap(), key(7).to_bytes());
        assert_eq!(dir.marker().as_deref(), Some("file"));
    }

    #[test]
    fn the_marker_says_file_and_the_file_is_gone_so_there_is_no_new_key() {
        for store in [Fake::default(), Fake::absent()] {
            let dir = Dir::new();
            dir.put_marker("file\n");
            let err = dir.load(&store).unwrap_err();
            assert!(matches!(err, KeyError::MissingKeyFile { .. }));
            assert!(dir.file_key().is_none());
            assert!(store.entry().is_none());
        }
    }

    // Damaged state is never repaired by guessing.

    #[test]
    fn a_stored_key_of_the_wrong_length_is_an_error_and_is_not_replaced() {
        let (dir, store) = (Dir::new(), Fake::default());
        store.0.borrow_mut().entry = Some(vec![1, 2, 3]);
        assert!(matches!(dir.load(&store), Err(KeyError::Corrupt)));
        assert_eq!(store.entry().unwrap(), vec![1, 2, 3]);
        assert!(dir.file_key().is_none());
    }

    #[test]
    fn a_marker_that_says_neither_is_an_error() {
        let (dir, store) = (Dir::new(), Fake::default());
        dir.put_marker("somewhere else\n");
        assert!(matches!(dir.load(&store), Err(KeyError::Io(_))));
        assert!(store.entry().is_none());
        assert!(dir.file_key().is_none());
    }

    // Replacing the key (an import).

    #[test]
    fn replacing_goes_where_the_marker_says_and_survives_a_restart() {
        let (dir, store) = (Dir::new(), Fake::default());
        dir.load(&store).unwrap();
        replace(&store, &dir.file(), &key(9)).unwrap();
        assert_eq!(store.entry().unwrap(), key(9).to_bytes());
        assert!(dir.file_key().is_none());
        assert_eq!(dir.load(&store).unwrap().to_bytes(), key(9).to_bytes());

        let (dir, store) = (Dir::new(), Fake::absent());
        dir.load(&store).unwrap();
        replace(&store, &dir.file(), &key(9)).unwrap();
        assert_eq!(dir.file_key().unwrap(), key(9).to_bytes());
        assert_eq!(dir.marker().as_deref(), Some("file"));
        assert_eq!(dir.load(&store).unwrap().to_bytes(), key(9).to_bytes());
    }

    #[test]
    fn a_replacement_the_store_refuses_leaves_the_old_key() {
        let (dir, store) = (Dir::new(), Fake::default());
        let old = dir.load(&store).unwrap();
        store.0.borrow_mut().read_only = true;
        assert!(matches!(replace(&store, &dir.file(), &key(9)), Err(KeyError::StoreUnavailable(_))));
        assert_eq!(dir.load(&store).unwrap().to_bytes(), old.to_bytes());

        store.0.borrow_mut().read_only = false;
        store.set_locked(true);
        assert!(replace(&store, &dir.file(), &key(9)).is_err());
        store.set_locked(false);
        assert_eq!(dir.load(&store).unwrap().to_bytes(), old.to_bytes());
    }
}
