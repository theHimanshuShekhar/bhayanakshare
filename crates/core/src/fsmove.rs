//! Moving a file into place without ever replacing an existing one.

use std::{io, path::Path};

/// Renames `from` to `to`, failing with `AlreadyExists` instead of replacing `to`.
///
/// Tries the strongest atomic primitive the platform and filesystem offer:
/// 1. Windows `MoveFileExW` without `MOVEFILE_REPLACE_EXISTING`, for files and folders;
/// 2. Linux `renameat2(RENAME_NOREPLACE)`;
/// 3. a hard link (which fails if `to` exists) followed by removing `from`, for files only;
/// 4. check-then-rename, which has a small window in which another program could create
///    `to`. It is only used where none of the above works (some FUSE and FAT mounts).
///
/// Windows has only the first, and a failure there is the answer: a folder cannot be linked,
/// and a check before a rename is the race this is here to avoid.
#[cfg(windows)]
pub fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    windows::move_no_replace(from, to)
}

#[cfg(not(windows))]
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

#[cfg(not(windows))]
fn move_by_link(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::hard_link(from, to)?;
    if let Err(e) = std::fs::remove_file(from) {
        // Do not leave the file under two names.
        let _ = std::fs::remove_file(to);
        return Err(e);
    }
    Ok(())
}

#[cfg(not(windows))]
fn check_then_rename(from: &Path, to: &Path) -> io::Result<()> {
    if to.symlink_metadata().is_ok() {
        return Err(io::ErrorKind::AlreadyExists.into());
    }
    std::fs::rename(from, to)
}

#[cfg(windows)]
mod windows {
    use std::{io, os::windows::ffi::OsStrExt, path::Path};

    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

    /// Moves a file or a folder to a name that is free, and fails with `AlreadyExists` (from
    /// `ERROR_ALREADY_EXISTS` or `ERROR_FILE_EXISTS`, which `std` maps) if there is anything at
    /// it. No flags: `MOVEFILE_REPLACE_EXISTING` is what would replace, and without
    /// `MOVEFILE_COPY_ALLOWED` a move to another volume fails (`ERROR_NOT_SAME_DEVICE`) instead
    /// of being copied and deleted. A folder moves within a volume in one step. Callers pass
    /// extended-length paths (see `long_path`), which are used as given.
    pub fn move_no_replace(from: &Path, to: &Path) -> io::Result<()> {
        let wide = |p: &Path| -> io::Result<Vec<u16>> {
            let mut wide: Vec<u16> = p.as_os_str().encode_wide().collect();
            if wide.contains(&0) {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "the path holds a NUL"));
            }
            wide.push(0);
            Ok(wide)
        };
        let (from, to) = (wide(from)?, wide(to)?);
        // SAFETY: both are NUL-terminated UTF-16 strings that live for the whole call.
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
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

    /// Every way of moving that is built here, so that each is held to the same rules.
    fn strategies() -> Vec<(&'static str, Move)> {
        let mut all: Vec<(&'static str, Move)> = vec![("rename_no_replace", rename_no_replace)];
        // Windows has neither: it moves with its own call.
        #[cfg(not(windows))]
        all.extend([("move_by_link", move_by_link as Move), ("check_then_rename", check_then_rename)]);
        all
    }

    #[test]
    fn moving_onto_an_existing_file_fails_and_leaves_both_intact() {
        for (name, mv) in strategies() {
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
        for (name, mv) in strategies() {
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
        for (name, mv) in strategies() {
            let dir = tempfile::tempdir().unwrap();
            let (from, to) = (dir.path().join("from"), dir.path().join("to"));
            std::fs::write(&from, b"new").unwrap();

            mv(&from, &to).unwrap();

            assert!(!from.exists(), "{name}");
            assert_eq!(std::fs::read(&to).unwrap(), b"new", "{name}");
        }
    }

    /// A folder is moved as a unit, so what is under it goes with it; it cannot be hard-linked,
    /// so this is the case a platform without a no-replace rename gets wrong.
    #[test]
    fn a_folder_moves_whole_to_a_free_name() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("from"), dir.path().join("to"));
        std::fs::create_dir_all(from.join("sub/empty")).unwrap();
        std::fs::write(from.join("sub/a.txt"), b"new").unwrap();

        rename_no_replace(&from, &to).unwrap();

        assert!(!from.exists());
        assert_eq!(std::fs::read(to.join("sub/a.txt")).unwrap(), b"new");
        assert!(to.join("sub/empty").is_dir());
    }

    /// What is already at the name: a file, an empty folder, a folder with a file in it.
    fn taken(to: &Path, kind: &str) {
        match kind {
            "file" => std::fs::write(to, b"mine").unwrap(),
            "empty folder" => std::fs::create_dir_all(to).unwrap(),
            _ => {
                std::fs::create_dir_all(to).unwrap();
                std::fs::write(to.join("mine.txt"), b"mine").unwrap();
            }
        }
    }

    /// Everything under `path`, as (relative path, content); a folder has `None`.
    fn tree(path: &Path) -> Vec<(String, Option<Vec<u8>>)> {
        fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, Option<Vec<u8>>)>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                let name = format!("{rel}{}", entry.file_name().to_string_lossy());
                if entry.file_type().unwrap().is_dir() {
                    out.push((name.clone(), None));
                    walk(&entry.path(), &format!("{name}/"), out);
                } else {
                    out.push((name, Some(std::fs::read(entry.path()).unwrap())));
                }
            }
        }
        let mut out = Vec::new();
        if path.is_dir() {
            walk(path, "", &mut out);
        } else {
            out.push((String::new(), Some(std::fs::read(path).unwrap())));
        }
        out.sort();
        out
    }

    #[test]
    fn a_folder_onto_a_file_or_a_folder_that_is_there_fails_and_leaves_both_intact() {
        for kind in ["file", "empty folder", "folder"] {
            let dir = tempfile::tempdir().unwrap();
            let (from, to) = (dir.path().join("from"), dir.path().join("to"));
            std::fs::create_dir_all(from.join("sub")).unwrap();
            std::fs::write(from.join("sub/new.txt"), b"new").unwrap();
            taken(&to, kind);
            let (before_from, before_to) = (tree(&from), tree(&to));

            let err = rename_no_replace(&from, &to).unwrap_err();

            assert_eq!(err.kind(), io::ErrorKind::AlreadyExists, "onto a {kind}: {err}");
            assert_eq!(tree(&from), before_from, "onto a {kind}");
            assert_eq!(tree(&to), before_to, "onto a {kind}");
        }
    }

    /// Guards against the Windows call being silently the replacing kind: the system call itself
    /// must refuse, for a file and for a folder, however it is named.
    #[cfg(windows)]
    #[test]
    fn the_windows_call_refuses_to_replace_a_file_or_a_folder() {
        for kind in ["file", "empty folder", "folder"] {
            let dir = tempfile::tempdir().unwrap();
            let (from, to) = (dir.path().join("from"), dir.path().join("to"));
            std::fs::create_dir_all(from.join("sub")).unwrap();
            std::fs::write(from.join("sub/new.txt"), b"new").unwrap();
            taken(&to, kind);
            let (before_from, before_to) = (tree(&from), tree(&to));

            let err = windows::move_no_replace(&from, &to).unwrap_err();

            assert_eq!(err.kind(), io::ErrorKind::AlreadyExists, "onto a {kind}: {err}");
            assert_eq!(tree(&from), before_from, "onto a {kind}");
            assert_eq!(tree(&to), before_to, "onto a {kind}");
        }
    }
}
