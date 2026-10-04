//! Moving a file into place without ever replacing an existing one.

use std::{io, path::Path};

/// Renames `from` to `to`, failing with `AlreadyExists` instead of replacing `to`.
///
/// Tries the strongest atomic primitive the platform and filesystem offer:
/// 1. Linux `renameat2(RENAME_NOREPLACE)`;
/// 2. a hard link (which fails if `to` exists) followed by removing `from`;
/// 3. check-then-rename, which has a small window in which another program could create
///    `to`. It is only used where neither of the above works (some FUSE and FAT mounts).
pub fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    match linux::renameat2_no_replace(from, to) {
        Err(e) if linux::is_unsupported(&e) => {}
        result => return result,
    }
    match move_by_link(from, to) {
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(e),
        Err(_) => check_then_rename(from, to),
        Ok(()) => Ok(()),
    }
}

fn move_by_link(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::hard_link(from, to)?;
    if let Err(e) = std::fs::remove_file(from) {
        // Do not leave the file under two names.
        let _ = std::fs::remove_file(to);
        return Err(e);
    }
    Ok(())
}

fn check_then_rename(from: &Path, to: &Path) -> io::Result<()> {
    if to.symlink_metadata().is_ok() {
        return Err(io::ErrorKind::AlreadyExists.into());
    }
    std::fs::rename(from, to)
}

#[cfg(target_os = "linux")]
mod linux {
    use std::{ffi::CString, io, os::unix::ffi::OsStrExt, path::Path};

    // Not every libc exposes the wrapper, so call the syscall directly.
    pub fn renameat2_no_replace(from: &Path, to: &Path) -> io::Result<()> {
        let c = |p: &Path| CString::new(p.as_os_str().as_bytes()).map_err(io::Error::other);
        let (from, to) = (c(from)?, c(to)?);
        // SAFETY: both pointers are valid NUL-terminated strings for the whole call.
        let rc = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if rc == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
    }

    /// The kernel or filesystem does not implement `RENAME_NOREPLACE`.
    pub fn is_unsupported(e: &io::Error) -> bool {
        matches!(e.raw_os_error(), Some(libc::EINVAL | libc::ENOSYS | libc::EOPNOTSUPP))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Move = fn(&Path, &Path) -> io::Result<()>;

    const STRATEGIES: [(&str, Move); 3] = [
        ("rename_no_replace", rename_no_replace),
        ("move_by_link", move_by_link),
        ("check_then_rename", check_then_rename),
    ];

    #[test]
    fn moving_onto_an_existing_file_fails_and_leaves_both_intact() {
        for (name, mv) in STRATEGIES {
            let dir = tempfile::tempdir().unwrap();
            let (from, to) = (dir.path().join("from"), dir.path().join("to"));
            std::fs::write(&from, b"new").unwrap();
            std::fs::write(&to, b"mine").unwrap();

            let err = mv(&from, &to).unwrap_err();

            assert_eq!(err.kind(), io::ErrorKind::AlreadyExists, "{name}");
            assert_eq!(std::fs::read(&from).unwrap(), b"new", "{name}");
            assert_eq!(std::fs::read(&to).unwrap(), b"mine", "{name}");
        }
    }

    #[test]
    fn moving_onto_a_dangling_symlink_fails_too() {
        #[cfg(unix)]
        for (name, mv) in STRATEGIES {
            let dir = tempfile::tempdir().unwrap();
            let (from, to) = (dir.path().join("from"), dir.path().join("to"));
            std::fs::write(&from, b"new").unwrap();
            std::os::unix::fs::symlink(dir.path().join("nowhere"), &to).unwrap();

            let err = mv(&from, &to).unwrap_err();

            assert_eq!(err.kind(), io::ErrorKind::AlreadyExists, "{name}");
            assert!(from.exists(), "{name}");
        }
    }

    /// Guards against the fast path being silently dead: on a filesystem that supports it,
    /// the syscall itself must refuse to replace.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_linux_syscall_refuses_to_replace() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("from"), dir.path().join("to"));
        std::fs::write(&from, b"new").unwrap();
        std::fs::write(&to, b"mine").unwrap();
        match linux::renameat2_no_replace(&from, &to) {
            Err(e) if linux::is_unsupported(&e) => eprintln!("renameat2 unsupported here: {e}"),
            Err(e) => assert_eq!(e.kind(), io::ErrorKind::AlreadyExists),
            Ok(()) => panic!("replaced an existing file"),
        }
        assert_eq!(std::fs::read(&to).unwrap(), b"mine");
        assert_eq!(std::fs::read(&from).unwrap(), b"new");
    }

    #[test]
    fn moving_to_a_free_name_moves_the_file() {
        for (name, mv) in STRATEGIES {
            let dir = tempfile::tempdir().unwrap();
            let (from, to) = (dir.path().join("from"), dir.path().join("to"));
            std::fs::write(&from, b"new").unwrap();

            mv(&from, &to).unwrap();

            assert!(!from.exists(), "{name}");
            assert_eq!(std::fs::read(&to).unwrap(), b"new", "{name}");
        }
    }
}
