//! Device identity: the Device ID, its Fingerprint, and where the secret key comes from.

use std::{fmt, path::PathBuf, str::FromStr};

use data_encoding::BASE32_NOPAD;
use iroh::{EndpointId, SecretKey};
use serde::{Serialize, Serializer};

use crate::{
    keyfile,
    keystore::{self, KeyError, OsSecretStore},
};

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

    /// The first 8 characters as `XXXX-XXXX`, for checking by eye. Never used to dial. This is
    /// also how a Device is named in the log: never `Display` a `DeviceId` there (that is the
    /// whole ID), nor an `EndpointId`, whose `Display` and `Debug` are the whole ID in hex. A
    /// `DeviceId`'s `Debug` is this.
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
    /// The OS secret store (Secret Service on Linux, Keychain on macOS, Credential Manager on
    /// Windows), or this file where there is none. See [`crate::keystore`] for how the two are
    /// kept from ever giving a Device two identities.
    OsStore { fallback: PathBuf },
}

impl KeySource {
    pub(crate) fn load_or_create(&self) -> Result<SecretKey, KeyError> {
        match self {
            Self::File(path) => keyfile::load_or_create(path),
            Self::OsStore { fallback } => keystore::load_or_create(&OsSecretStore, fallback),
        }
    }

    /// Makes `key` the key the next start loads, where the current one is kept.
    pub(crate) fn replace(&self, key: &SecretKey) -> Result<(), KeyError> {
        match self {
            Self::File(path) => Ok(keyfile::write_replacing(path, &key.to_bytes())?),
            Self::OsStore { fallback } => keystore::replace(&OsSecretStore, fallback, key),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;

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

    #[cfg(windows)]
    #[test]
    #[ignore = "Windows has no 0600 mode: the key file relies on the user profile folder's permissions"]
    fn loose_key_file_permissions_are_tightened() {}

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
        assert!(matches!(err, KeyError::Io(e) if e.kind() == io::ErrorKind::InvalidData));
    }
}
