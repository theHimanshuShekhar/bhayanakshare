//! Free space: the Receiver's "will it fit" check before accepting an Offer.
//!
//! The probe is a seam on [`crate::DeviceConfig`], so a test can make an Offer too large
//! without filling a real disk. The default asks the operating system.

use std::{io, path::Path};

use serde::Serialize;

/// How much room a folder has.
pub trait FreeSpace: Send + Sync + 'static {
    /// Bytes this user can still write to the filesystem holding `dir`. Fails with
    /// [`io::ErrorKind::Unsupported`] where the platform cannot say.
    fn available(&self, dir: &Path) -> io::Result<u64>;
}

/// A closure is a probe, which is all a test needs.
impl<F> FreeSpace for F
where
    F: Fn(&Path) -> io::Result<u64> + Send + Sync + 'static,
{
    fn available(&self, dir: &Path) -> io::Result<u64> {
        self(dir)
    }
}

/// Asks the operating system (`statvfs`; other platforms cannot say yet, so they never block
/// an Offer on space).
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemFreeSpace;

impl FreeSpace for SystemFreeSpace {
    #[cfg(unix)]
    fn available(&self, dir: &Path) -> io::Result<u64> {
        use std::{ffi::CString, mem::MaybeUninit, os::unix::ffi::OsStrExt};

        let path = CString::new(dir.as_os_str().as_bytes()).map_err(io::Error::other)?;
        let mut stat = MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: `path` is a valid NUL-terminated string and `stat` is a valid out pointer.
        if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: statvfs returned 0, so it filled the struct in.
        let stat = unsafe { stat.assume_init() };
        // `f_bavail` counts blocks of `f_frsize` bytes available to unprivileged users. The
        // field widths differ by platform, hence the casts.
        Ok((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
    }

    #[cfg(not(unix))]
    fn available(&self, _dir: &Path) -> io::Result<u64> {
        Err(io::ErrorKind::Unsupported.into())
    }
}

/// What an Offer needs, against what its save folder has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
pub struct SpaceCheck {
    /// The Offer's total size in bytes.
    pub needed: u64,
    /// Bytes free in the save folder; `None` when the platform cannot say.
    pub free: Option<u64>,
}

impl SpaceCheck {
    /// Whether the Offer fits. Unknown free space is no reason to refuse.
    pub fn fits(&self) -> bool {
        self.free.is_none_or(|free| free >= self.needed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_offer_fits_when_it_is_no_bigger_than_the_free_space() {
        assert!(SpaceCheck { needed: 10, free: Some(10) }.fits());
        assert!(!SpaceCheck { needed: 11, free: Some(10) }.fits());
        assert!(SpaceCheck { needed: u64::MAX, free: None }.fits());
    }

    #[test]
    fn a_closure_is_a_probe() {
        let probe = |_: &Path| Ok(7);
        assert_eq!(probe.available(Path::new("/")).unwrap(), 7);
    }

    #[cfg(unix)]
    #[test]
    fn the_system_probe_reads_a_real_folder_and_fails_for_a_missing_one() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(SystemFreeSpace.available(tmp.path()).unwrap() > 0);
        assert!(SystemFreeSpace.available(&tmp.path().join("missing")).is_err());
    }
}
