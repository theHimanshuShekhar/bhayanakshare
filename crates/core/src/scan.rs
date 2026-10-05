//! Reading what the user chose to send off the disk and turning it into a manifest.
//!
//! Each chosen path is one top-level item of the Transfer, named as it is on disk: a file is
//! one entry, a folder is walked. Symlinks found inside a folder are skipped and counted, never
//! followed: they could lead out of the folder chosen, or round in circles. Anything else that
//! is not a plain file or folder (a socket, a device) is left out without a count. A path the
//! user chose themselves is followed if it is a symlink, as it always was, because they named
//! it.

use std::{
    fs::Metadata,
    path::{Path, PathBuf},
};

use crate::{
    db::Source,
    error::Error,
    manifest::{Entry, MAX_ENCODED_LEN, MAX_ENTRIES, MAX_PATH_LEN, Manifest, ManifestError},
    names::MAX_NAME_LEN,
    sender::mtime_ns,
};

/// What was found: the manifest, the file behind each of its files in the same order, and how
/// many symlinks were left out.
pub(crate) struct Scan {
    pub manifest: Manifest,
    pub sources: Vec<Source>,
    pub skipped_links: u32,
}

/// Room an entry takes in the encoded manifest besides its path: the variant, the length of
/// the path, the size, the time and the flag, each at their longest.
const ENTRY_OVERHEAD: usize = 1 + 2 + 10 + 10 + 1;

struct Walk {
    /// Every entry with the file it was made from, if it is one.
    found: Vec<(Entry, Option<PathBuf>)>,
    skipped_links: u32,
    /// What the entries found so far would take on the wire, at most.
    encoded: usize,
}

/// Lists `roots`, each a file or a folder. Fails if one is neither, if something cannot be
/// read, or if the selection breaks a limit: that is found as the walk goes, so a huge tree
/// is refused without being listed first.
pub(crate) fn scan(roots: &[PathBuf]) -> Result<Scan, Error> {
    let mut walk = Walk { found: Vec::new(), skipped_links: 0, encoded: 0 };
    for root in roots {
        let meta = std::fs::metadata(root).map_err(|_| Error::NotAFile(root.clone()))?;
        let name = root
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| Error::NotAFile(root.clone()))?;
        if meta.is_file() {
            walk.add(name.to_owned(), Some((root, &meta)))?;
        } else if meta.is_dir() {
            walk.dir(root, name)?;
        } else {
            return Err(Error::NotAFile(root.clone()));
        }
    }
    // The same order on every run, whatever order the disk lists in.
    walk.found.sort_by(|a, b| a.0.path().cmp(b.0.path()));
    let (entries, files): (Vec<_>, Vec<_>) = walk.found.into_iter().unzip();
    let manifest = Manifest { entries };
    manifest.validate()?;
    let sources = manifest
        .entries
        .iter()
        .zip(files)
        .filter_map(|(entry, path)| match (entry, path) {
            (Entry::File { path: name, size, mtime_ns, .. }, Some(path)) => {
                Some(Source { path, size: *size, mtime_ns: *mtime_ns, name: name.clone() })
            }
            _ => None,
        })
        .collect();
    Ok(Scan { manifest, sources, skipped_links: walk.skipped_links })
}

impl Walk {
    /// Adds an entry for `rel`: a file if `file` is given, else a folder with nothing in it.
    fn add(&mut self, rel: String, file: Option<(&Path, &Metadata)>) -> Result<(), ManifestError> {
        let index = self.found.len();
        if rel.len() > MAX_PATH_LEN {
            return Err(ManifestError::PathTooLong(index));
        }
        if rel.rsplit('/').next().is_some_and(|name| name.len() > MAX_NAME_LEN) {
            return Err(ManifestError::NameTooLong(index));
        }
        if index >= MAX_ENTRIES {
            return Err(ManifestError::TooManyEntries);
        }
        self.encoded += rel.len() + ENTRY_OVERHEAD;
        if self.encoded > MAX_ENCODED_LEN {
            return Err(ManifestError::TooLarge);
        }
        self.found.push(match file {
            Some((path, meta)) => (
                Entry::File {
                    path: rel,
                    size: meta.len(),
                    mtime_ns: mtime_ns(meta),
                    executable: is_executable(meta),
                },
                Some(path.to_owned()),
            ),
            None => (Entry::EmptyDir { path: rel }, None),
        });
        Ok(())
    }

    /// Walks the folder at `dir`, called `rel` in the manifest. A folder that ends up with
    /// nothing recorded in it is recorded as empty.
    fn dir(&mut self, dir: &Path, rel: &str) -> Result<(), Error> {
        let before = self.found.len();
        let read = |e| Error::io(format!("reading {}", dir.display()), e);
        for item in std::fs::read_dir(dir).map_err(read)? {
            let item = item.map_err(read)?;
            let kind = item.file_type().map_err(read)?;
            if kind.is_symlink() {
                self.skipped_links = self.skipped_links.saturating_add(1);
                continue;
            }
            // Not a name that can be written down as text: it cannot be sent.
            let Some(name) = item.file_name().to_str().map(str::to_owned) else {
                return Err(ManifestError::InvalidName(self.found.len()).into());
            };
            let child = format!("{rel}/{name}");
            if kind.is_dir() {
                self.dir(&item.path(), &child)?;
            } else if kind.is_file() {
                let meta = item.metadata().map_err(read)?;
                self.add(child, Some((&item.path(), &meta)))?;
            }
        }
        if self.found.len() == before {
            self.add(rel.to_owned(), None)?;
        }
        Ok(())
    }
}

#[cfg(unix)]
fn is_executable(meta: &Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_: &Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn paths(scan: &Scan) -> Vec<String> {
        scan.manifest.entries.iter().map(|e| e.path().to_owned()).collect()
    }

    #[test]
    fn a_tree_becomes_sorted_entries_with_sizes_and_empty_folders() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("album");
        write(&root.join("b.txt"), b"bb");
        write(&root.join("sub/deep/c.txt"), b"ccc");
        std::fs::create_dir_all(root.join("empty")).unwrap();
        std::fs::create_dir_all(root.join("sub/also empty")).unwrap();

        let scan = scan(&[root.clone()]).unwrap();

        assert_eq!(
            paths(&scan),
            ["album/b.txt", "album/empty", "album/sub/also empty", "album/sub/deep/c.txt"]
        );
        assert_eq!(scan.manifest.total_size(), Some(5));
        assert_eq!(scan.manifest.file_count(), 2);
        assert_eq!(scan.skipped_links, 0);
        // One source per file, in manifest order, each with where it came from.
        let sources: Vec<_> = scan.sources.iter().map(|s| (s.name.as_str(), s.size, s.path.clone())).collect();
        assert_eq!(
            sources,
            [("album/b.txt", 2, root.join("b.txt")), ("album/sub/deep/c.txt", 3, root.join("sub/deep/c.txt"))]
        );
    }

    #[test]
    fn several_chosen_items_are_each_a_top_level_item() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("one.txt"), b"1");
        write(&tmp.path().join("dir/two.txt"), b"22");
        std::fs::create_dir_all(tmp.path().join("nothing")).unwrap();

        let scan = scan(&[
            tmp.path().join("one.txt"),
            tmp.path().join("dir"),
            tmp.path().join("nothing"),
        ])
        .unwrap();

        assert_eq!(paths(&scan), ["dir/two.txt", "nothing", "one.txt"]);
        assert_eq!(scan.manifest.top_level_items(), ["dir", "nothing", "one.txt"]);
    }

    #[test]
    fn the_modification_time_and_executable_bit_are_read() {
        let tmp = tempfile::tempdir().unwrap();
        let (plain, tool) = (tmp.path().join("d/plain"), tmp.path().join("d/tool"));
        write(&plain, b"p");
        write(&tool, b"#!/bin/sh\n");
        let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
        std::fs::OpenOptions::new().write(true).open(&plain).unwrap().set_modified(when).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o750)).unwrap();
            std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o604)).unwrap();
        }

        let scan = scan(&[tmp.path().join("d")]).unwrap();

        let [Entry::File { mtime_ns: plain_time, executable: plain_x, .. }, Entry::File { executable: tool_x, .. }] =
            scan.manifest.entries.as_slice()
        else {
            panic!("{:?}", scan.manifest)
        };
        assert_eq!(*plain_time, 1_600_000_000_000_000_000);
        assert!(!plain_x);
        assert_eq!(*tool_x, cfg!(unix));
        // What the Sender will check again later is exactly what it offered.
        assert_eq!(scan.sources[0].mtime_ns, 1_600_000_000_000_000_000);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_inside_a_folder_are_skipped_and_counted_not_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside");
        write(&outside.join("secret.txt"), b"s");
        let root = tmp.path().join("pack");
        write(&root.join("real.txt"), b"r");
        std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("link-to-file")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link-to-dir")).unwrap();
        std::os::unix::fs::symlink(&root, root.join("loop")).unwrap();
        std::os::unix::fs::symlink("nowhere", root.join("dangling")).unwrap();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("sub/link")).unwrap();

        let scan = scan(&[root]).unwrap();

        // A folder with only a link in it holds nothing that is sent.
        assert_eq!(paths(&scan), ["pack/real.txt", "pack/sub"]);
        assert!(matches!(scan.manifest.entries[1], Entry::EmptyDir { .. }));
        assert_eq!(scan.skipped_links, 5);
    }

    #[cfg(unix)]
    #[test]
    fn a_path_the_user_chose_is_followed_even_if_it_is_a_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("real/f.txt"), b"f");
        std::os::unix::fs::symlink(tmp.path().join("real"), tmp.path().join("alias")).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("real/f.txt"), tmp.path().join("one.txt")).unwrap();

        let scan = scan(&[tmp.path().join("alias"), tmp.path().join("one.txt")]).unwrap();

        assert_eq!(paths(&scan), ["alias/f.txt", "one.txt"]);
        assert_eq!(scan.skipped_links, 0);
    }

    #[cfg(unix)]
    #[test]
    fn sockets_and_pipes_are_left_out_without_being_counted_as_links() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("d");
        write(&root.join("f"), b"x");
        std::os::unix::net::UnixListener::bind(root.join("sock")).unwrap();

        let scan = scan(&[root]).unwrap();

        assert_eq!(paths(&scan), ["d/f"]);
        assert_eq!(scan.skipped_links, 0);
    }

    #[test]
    fn something_that_is_not_there_or_has_no_name_is_not_sendable() {
        let tmp = tempfile::tempdir().unwrap();
        for bad in [tmp.path().join("missing"), PathBuf::from("/"), tmp.path().join("a/..")] {
            assert!(matches!(scan(&[bad.clone()]), Err(Error::NotAFile(_))), "{bad:?}");
        }
        // One bad item spoils the whole selection.
        write(&tmp.path().join("ok"), b"1");
        assert!(scan(&[tmp.path().join("ok"), tmp.path().join("missing")]).is_err());
    }

    #[test]
    fn nothing_chosen_is_nothing_to_send() {
        assert!(matches!(scan(&[]), Err(Error::Manifest(ManifestError::Empty))));
    }

    #[test]
    fn two_items_with_the_same_name_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("a/x.txt"), b"1");
        write(&tmp.path().join("b/x.txt"), b"2");
        let result = scan(&[tmp.path().join("a/x.txt"), tmp.path().join("b/x.txt")]);
        assert!(matches!(result, Err(Error::Manifest(ManifestError::Duplicate(_)))));
        // The same item twice is the same thing.
        let result = scan(&[tmp.path().join("a"), tmp.path().join("a")]);
        assert!(matches!(result, Err(Error::Manifest(ManifestError::Duplicate(_)))));
    }

    #[cfg(unix)]
    #[test]
    fn names_that_cannot_be_sent_refuse_the_selection() {
        use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("d");
        // Control characters, a backslash and bytes that are not UTF-8 are all legal on Linux.
        for name in [OsStr::new("a\nb"), OsStr::new("a\\b"), OsStr::from_bytes(b"bad\xff")] {
            write(&root.join(name), b"x");
            assert!(
                matches!(scan(&[root.clone()]), Err(Error::Manifest(ManifestError::InvalidName(_)))),
                "{name:?}"
            );
            std::fs::remove_file(root.join(name)).unwrap();
        }
    }

    #[test]
    fn a_selection_over_a_limit_is_refused_as_the_walk_goes() {
        let new_walk = || Walk { found: Vec::new(), skipped_links: 0, encoded: 0 };

        let mut walk = new_walk();
        let longest = vec!["x".repeat(255); 16].join("/");
        assert_eq!(longest.len(), MAX_PATH_LEN - 1);
        assert_eq!(walk.add(longest.clone(), None), Ok(()));
        assert_eq!(walk.add(format!("{longest}/y"), None), Err(ManifestError::PathTooLong(1)));
        assert_eq!(walk.add(format!("d/{}", "y".repeat(256)), None), Err(ManifestError::NameTooLong(1)));
        assert_eq!(walk.found.len(), 1);

        let mut walk = new_walk();
        walk.found = (0..MAX_ENTRIES).map(|i| (Entry::file(i.to_string(), 0), None)).collect();
        assert_eq!(walk.add("one more".into(), None), Err(ManifestError::TooManyEntries));

        let mut walk = new_walk();
        walk.encoded = MAX_ENCODED_LEN - 10;
        assert_eq!(walk.add("some path".into(), None), Err(ManifestError::TooLarge));
    }
}
