//! The Receiver's "will it fit" checks before accepting an Offer: free space and path length.
//!
//! The probe is a seam on [`crate::DeviceConfig`], so a test can make an Offer too large
//! without filling a real disk. The default asks the operating system.

use std::{
    io,
    path::{Path, PathBuf},
};

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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct SpaceCheck {
    /// The folder checked, as an absolute path: where the Offer would be saved.
    pub folder: PathBuf,
    /// The Offer's total size in bytes.
    pub needed: u64,
    /// Bytes free in the save folder; `None` when the platform cannot say.
    pub free: Option<u64>,
    /// Some path in the Offer would be longer than the filesystem allows once it is under the
    /// save folder, so the Offer cannot be accepted into this folder.
    pub paths_too_long: bool,
}

impl SpaceCheck {
    /// Whether the Offer fits. Unknown free space is no reason to refuse.
    pub fn fits(&self) -> bool {
        self.free.is_none_or(|free| free >= self.needed)
    }

    /// Whether every check passes: the Offer fits and its paths are not too long.
    pub fn passes(&self) -> bool {
        self.fits() && !self.paths_too_long
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_offer_fits_when_it_is_no_bigger_than_the_free_space() {
        let check = |needed, free| SpaceCheck { folder: "/save".into(), needed, free, paths_too_long: false };
        assert!(check(10, Some(10)).fits());
        assert!(!check(11, Some(10)).fits());
        assert!(check(u64::MAX, None).fits());
    }

    #[test]
    fn long_paths_fail_the_checks_whatever_the_space() {
        let roomy = SpaceCheck { folder: "/save".into(), needed: 1, free: Some(100), paths_too_long: false };
        assert!(roomy.passes());
        let long = SpaceCheck { paths_too_long: true, ..roomy.clone() };
        assert!(long.fits());
        assert!(!long.passes());
        assert!(!SpaceCheck { needed: 2, free: Some(1), ..roomy }.passes());
    }

    #[test]
    fn a_closure_is_a_probe() {
        let probe = |_: &Path| Ok(7);
        assert_eq!(probe.available(Path::new("/")).unwrap(), 7);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Windows has no free-space probe yet; #54 adds one and re-enables this"]
    fn the_system_probe_reads_a_real_folder_and_fails_for_a_missing_one() {}

    #[cfg(unix)]
    #[test]
    fn the_system_probe_reads_a_real_folder_and_fails_for_a_missing_one() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(SystemFreeSpace.available(tmp.path()).unwrap() > 0);
        assert!(SystemFreeSpace.available(&tmp.path().join("missing")).is_err());
    }
}
