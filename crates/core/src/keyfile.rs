//! The key as a file of 32 raw bytes, mode 0600: the whole of `KeySource::File`, and the
//! fallback of `KeySource::OsStore` (see `keystore.rs`).

use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};

use iroh::SecretKey;

use crate::keystore::KeyError;

pub(crate) fn load_or_create(path: &Path) -> Result<SecretKey, KeyError> {
    match read_key_file(path)? {
        Some(key) => Ok(key),
        None => {
            let key = SecretKey::generate();
            write_new_key_file(path, &key.to_bytes())?;
            Ok(key)
        }
    }
}

/// `None` if there is no file; an error if it is not a key.
pub(crate) fn read_key_file(path: &Path) -> io::Result<Option<SecretKey>> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let key = SecretKey::try_from(bytes.as_slice()).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "secret key file is not 32 bytes")
            })?;
            restrict_permissions(path)?;
            Ok(Some(key))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub(crate) fn remove_key_file(path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        tracing::warn!("could not remove {}: {e}", path.display());
    }
}

/// Writes through a private temp file and links it into place, so the key file is never
/// visible half-written or with wider permissions, and an existing key is never replaced.
pub(crate) fn write_new_key_file(path: &Path, bytes: &[u8; 32]) -> io::Result<()> {
    let tmp = write_private_temp(path, bytes)?;
    let linked = std::fs::hard_link(&tmp, path);
    std::fs::remove_file(&tmp)?;
    linked
}

/// Like [`write_new_key_file`], but replaces whatever is there, atomically: a reader sees the
/// old contents or the new, never half of either.
pub(crate) fn write_replacing(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = write_private_temp(path, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Writes `bytes` to a new 0600 file next to `path` and returns where.
fn write_private_temp(path: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
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
    Ok(tmp)
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
