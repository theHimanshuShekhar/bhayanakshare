//! Device identity: the Device ID, its Fingerprint, and where the secret key comes from.

use std::{
    fmt,
    io::{self, Write},
    path::{Path, PathBuf},
    str::FromStr,
};

use data_encoding::BASE32_NOPAD;
use iroh::{EndpointId, SecretKey};
use serde::{Serialize, Serializer};

/// Length of a Device ID in characters (32 bytes as unpadded base32).
pub const DEVICE_ID_LEN: usize = 52;

/// The permanent, shareable identifier of a Device: the iroh `EndpointId`, shown as
/// 52-character base32.
#[derive(Clone, Copy, PartialEq, Eq, Hash, specta::Type)]
#[specta(type = String)] // serialized as its base32 text
pub struct DeviceId(EndpointId);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DeviceIdError {
    #[error("a Device ID is {DEVICE_ID_LEN} characters of base32")]
    Malformed,
    #[error("not a valid Device ID")]
    InvalidKey,
}

impl DeviceId {
    pub(crate) fn from_endpoint_id(id: EndpointId) -> Self {
        Self(id)
    }

    pub(crate) fn endpoint_id(&self) -> EndpointId {
        self.0
    }

    /// The raw 32-byte public key.
    pub fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }

    /// The first 8 characters as `XXXX-XXXX`, for checking by eye. Never used to dial.
    pub fn fingerprint(&self) -> String {
        let id = self.to_string();
        format!("{}-{}", &id[..4], &id[4..8])
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&BASE32_NOPAD.encode(self.0.as_bytes()))
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceId({})", self.fingerprint())
    }
}

impl FromStr for DeviceId {
    type Err = DeviceIdError;

    /// Parses the 52-character base32 form, case-insensitively. Unlike iroh's own parser,
    /// the 64-character hex form is not accepted.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != DEVICE_ID_LEN {
            return Err(DeviceIdError::Malformed);
        }
        let bytes = BASE32_NOPAD
            .decode(s.to_ascii_uppercase().as_bytes())
            .map_err(|_| DeviceIdError::Malformed)?;
        let bytes: [u8; 32] = bytes.try_into().map_err(|_| DeviceIdError::Malformed)?;
        EndpointId::from_bytes(&bytes).map(Self).map_err(|_| DeviceIdError::InvalidKey)
    }
}

impl Serialize for DeviceId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

/// Where a Device's secret key is read from (and created in, on first run).
#[derive(Debug, Clone)]
pub enum KeySource {
    /// A file holding the 32 raw key bytes, created with mode 0600 if missing.
    File(PathBuf),
}

impl KeySource {
    pub(crate) fn load_or_create(&self) -> io::Result<SecretKey> {
        match self {
            Self::File(path) => load_or_create_key_file(path),
        }
    }
}

fn load_or_create_key_file(path: &Path) -> io::Result<SecretKey> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let key = SecretKey::try_from(bytes.as_slice()).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "secret key file is not 32 bytes")
            })?;
            restrict_permissions(path)?;
            Ok(key)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let key = SecretKey::generate();
            write_new_key_file(path, &key.to_bytes())?;
            Ok(key)
        }
        Err(e) => Err(e),
    }
}

/// Writes through a private temp file and links it into place, so the key file is never
/// visible half-written or with wider permissions, and an existing key is never replaced.
fn write_new_key_file(path: &Path, bytes: &[u8; 32]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut file = opts.open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    let linked = std::fs::hard_link(&tmp, path);
    std::fs::remove_file(&tmp)?;
    linked
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)?.permissions().mode();
    if mode & 0o077 != 0 {
        tracing::warn!("secret key file was readable by others; restricting it to 0600");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id_from(bytes: [u8; 32]) -> DeviceId {
        DeviceId::from_endpoint_id(SecretKey::from_bytes(&bytes).public())
    }

    #[test]
    fn device_id_is_52_char_base32_and_round_trips() {
        let id = id_from([7; 32]);
        let text = id.to_string();
        assert_eq!(text.len(), 52);
        assert!(text.chars().all(|c| matches!(c, 'A'..='Z' | '2'..='7')));
        assert_eq!(text.parse::<DeviceId>().unwrap(), id);
        assert_eq!(text.to_lowercase().parse::<DeviceId>().unwrap(), id);
    }

    #[test]
    fn fingerprint_is_first_eight_characters_as_xxxx_xxxx() {
        let id = id_from([7; 32]);
        let text = id.to_string();
        let fp = id.fingerprint();
        assert_eq!(fp.len(), 9);
        assert_eq!(fp, format!("{}-{}", &text[..4], &text[4..8]));
        assert_eq!(fp.as_bytes()[4], b'-');
    }

    #[test]
    fn rejects_malformed_ids() {
        assert_eq!("".parse::<DeviceId>(), Err(DeviceIdError::Malformed));
        assert_eq!("ABC".parse::<DeviceId>(), Err(DeviceIdError::Malformed));
        // 52 characters but outside the base32 alphabet.
        assert_eq!("1".repeat(52).parse::<DeviceId>(), Err(DeviceIdError::Malformed));
        // iroh's 64-character hex form is not a Device ID.
        assert_eq!("ab".repeat(32).parse::<DeviceId>(), Err(DeviceIdError::Malformed));
    }

    #[test]
    fn key_file_is_created_private_and_stable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("secret.key");
        let first = KeySource::File(path.clone()).load_or_create().unwrap();
        let second = KeySource::File(path.clone()).load_or_create().unwrap();
        assert_eq!(first.to_bytes(), second.to_bytes());
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 32);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[cfg(unix)]
    #[test]
    fn loose_key_file_permissions_are_tightened() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.key");
        KeySource::File(path.clone()).load_or_create().unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        KeySource::File(path.clone()).load_or_create().unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn corrupt_key_file_is_an_error_not_a_new_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.key");
        std::fs::write(&path, b"short").unwrap();
        let err = KeySource::File(path).load_or_create().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
