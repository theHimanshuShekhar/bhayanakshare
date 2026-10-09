//! Extended-length paths for received content.
//!
//! Windows reads a path of 260 characters or more only if it is written `\\?\C:\...` (or
//! `\\?\UNC\server\share\...`); the prefix also turns off the conversion of `/` and `..`, so
//! the path has to be complete and normal first. Everything the Receiver creates, writes,
//! exports into or moves of a Transfer goes through [`long`]. The paths it keeps, shows and
//! records stay as they were: [`long`] is applied where a path is handed to the filesystem, not
//! where it is stored. Elsewhere it changes nothing.
//!
//! <https://learn.microsoft.com/en-us/windows/win32/fileio/maximum-file-path-limitation>

use std::path::{Path, PathBuf};

/// `path` as the filesystem should be given it: on Windows, the absolute, normalised path with
/// the `\\?\` (or `\\?\UNC\`) prefix, unless it has one already; on other systems, `path`.
///
/// A path that cannot be made absolute (the empty path) is returned as it is, for the
/// filesystem to refuse.
#[cfg(not(windows))]
pub(crate) fn long(path: &Path) -> PathBuf {
    path.to_owned()
}

#[cfg(windows)]
pub(crate) fn long(path: &Path) -> PathBuf {
    use std::{
        ffi::OsString,
        os::windows::ffi::{OsStrExt, OsStringExt},
        path::{Component, Prefix},
    };

    // `GetFullPathNameW`: the drive, `/` as `\`, and no `.` or `..`. It leaves a path that
    // starts `\\?\` alone.
    let Ok(absolute) = std::path::absolute(path) else { return path.to_owned() };
    let Some(Component::Prefix(prefix)) = absolute.components().next() else { return absolute };
    let wide = absolute.as_os_str().encode_wide();
    let extended: Vec<u16> = match prefix.kind() {
        Prefix::Disk(_) => r"\\?\".encode_utf16().chain(wide).collect(),
        // `\\server\share` becomes `\\?\UNC\server\share`: the two backslashes are replaced.
        Prefix::UNC(..) => r"\\?\UNC\".encode_utf16().chain(wide.skip(2)).collect(),
        // Already extended-length, or a device path.
        _ => return absolute,
    };
    PathBuf::from(OsString::from_wide(&extended))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn off_windows_a_path_is_left_as_it_is() {
        for path in ["/home/me/a/../b", "relative/x", ""] {
            assert_eq!(long(Path::new(path)), Path::new(path));
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_drive_path_gets_the_extended_prefix_with_backslashes_only_and_no_dots() {
        assert_eq!(long(Path::new(r"C:\Users\me\a")), Path::new(r"\\?\C:\Users\me\a"));
        // What the Receiver builds: a base joined with the manifest's `/` path.
        assert_eq!(long(&Path::new(r"C:\save\out").join("d/e/f.txt")), Path::new(r"\\?\C:\save\out\d\e\f.txt"));
        assert_eq!(long(Path::new(r"C:\save\x\..\y\.\z")), Path::new(r"\\?\C:\save\y\z"));
    }

    #[cfg(windows)]
    #[test]
    fn a_network_path_gets_the_unc_form() {
        assert_eq!(long(Path::new(r"\\server\share\dir\a")), Path::new(r"\\?\UNC\server\share\dir\a"));
    }

    #[cfg(windows)]
    #[test]
    fn a_path_that_is_extended_already_is_not_changed_again() {
        for path in [r"\\?\C:\Users\me\a", r"\\?\UNC\server\share\a"] {
            assert_eq!(long(Path::new(path)), Path::new(path));
            assert_eq!(long(&long(Path::new(path))), Path::new(path));
        }
    }

    /// The paths this is for: a tree past 260 characters is written and read through it.
    #[test]
    fn a_path_past_260_characters_can_be_written_and_read_through_it() {
        let tmp = tempfile::tempdir().unwrap();
        let deep = (0..6).fold(tmp.path().to_owned(), |path, level| path.join(format!("{level}{}", "d".repeat(49))));
        let file = deep.join("f.txt");
        assert!(file.as_os_str().len() > 260);

        std::fs::create_dir_all(long(&deep)).unwrap();
        std::fs::write(long(&file), b"deep").unwrap();

        assert_eq!(std::fs::read(long(&file)).unwrap(), b"deep");
        let moved = deep.join("g.txt");
        std::fs::rename(long(&file), long(&moved)).unwrap();
        assert_eq!(std::fs::read(long(&moved)).unwrap(), b"deep");
    }
}
