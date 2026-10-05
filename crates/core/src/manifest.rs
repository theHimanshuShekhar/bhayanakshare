//! The manifest of an Offer: the tree a Transfer carries, as the Sender describes it and the
//! Receiver checks it (spec section 5). Relative paths with `/` separators, and for each file
//! its size, modification time and executable bit; folders that hold nothing are listed too,
//! as every other folder follows from the paths of the files in it. Ownership and other
//! permissions are never sent.
//!
//! Everything in a manifest is the other Device's say-so, so [`Manifest::validate`] is the
//! trust boundary: it is pure, and the Receiver runs it before an Offer is shown to anyone. The
//! Sender runs it on its own manifest too, which is how an over-limit selection is refused
//! before anything is sent.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::{
    names::{NameError, validate_file_name},
    protocol::MAX_FRAME_LEN,
};

/// Most entries (files and empty folders) one Offer may list.
pub const MAX_ENTRIES: usize = 500_000;

/// Largest encoded manifest, in bytes: the Offer is one control frame, which cannot be bigger.
pub const MAX_ENCODED_LEN: usize = MAX_FRAME_LEN as usize;

/// Longest relative path, in bytes.
pub const MAX_PATH_LEN: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Entry {
    File {
        /// Relative, `/`-separated, never absolute.
        path: String,
        size: u64,
        /// Modification time in nanoseconds since the Unix epoch.
        mtime_ns: i64,
        executable: bool,
    },
    /// A folder with nothing in it.
    EmptyDir { path: String },
}

impl Entry {
    /// A file with no modification time and no executable bit, which is all most checks need.
    pub fn file(path: impl Into<String>, size: u64) -> Self {
        Self::File { path: path.into(), size, mtime_ns: 0, executable: false }
    }

    pub fn empty_dir(path: impl Into<String>) -> Self {
        Self::EmptyDir { path: path.into() }
    }

    pub fn path(&self) -> &str {
        match self {
            Self::File { path, .. } | Self::EmptyDir { path } => path,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub entries: Vec<Entry>,
}

/// Why a manifest cannot be used. The indexes say which entry; they are not paths, because
/// what a hostile peer sends is not to be logged or shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    #[error("There is nothing to send.")]
    Empty,
    #[error("Too many files and folders: at most 500,000 can be sent at once.")]
    TooManyEntries,
    #[error("The list of files is too long to send at once.")]
    TooLarge,
    #[error("A path is longer than 4,096 bytes.")]
    PathTooLong(usize),
    #[error("A file or folder name is longer than 255 bytes.")]
    NameTooLong(usize),
    /// Absolute, `.` or `..`, an empty segment, NUL or another control character, or a
    /// backslash: anything that could name a place outside the folder it is saved in, or that
    /// is not one name per segment on every operating system.
    #[error("Couldn't be sent: invalid file names")]
    InvalidName(usize),
    #[error("Two of the chosen items have the same name.")]
    Duplicate(usize),
    /// The entry sits inside another entry, which is a file or a folder said to be empty.
    #[error("Couldn't be sent: invalid file names")]
    InsideEntry(usize),
    /// An Offer's total size or file count is not what its manifest adds up to.
    #[error("The Offer does not add up.")]
    Inconsistent,
}

impl Manifest {
    /// The files, with their sizes, in the order the Sender listed them.
    pub fn files(&self) -> impl Iterator<Item = (&str, u64)> {
        self.entries.iter().filter_map(|entry| match entry {
            Entry::File { path, size, .. } => Some((path.as_str(), *size)),
            Entry::EmptyDir { .. } => None,
        })
    }

    pub fn file_count(&self) -> u64 {
        self.files().count() as u64
    }

    /// The sum of the file sizes, or `None` if it does not fit in 64 bits.
    pub fn total_size(&self) -> Option<u64> {
        self.files().try_fold(0u64, |sum, (_, size)| sum.checked_add(size))
    }

    /// The names at the top of the tree, each once, in the order they first appear: what the
    /// user picked, and what lands in the save folder.
    pub fn top_level_items(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut items = Vec::new();
        for entry in &self.entries {
            let top = entry.path().split('/').next().unwrap_or_default();
            if seen.insert(top) {
                items.push(top.to_owned());
            }
        }
        items
    }

    /// How many bytes the manifest takes on the wire.
    pub fn encoded_len(&self) -> usize {
        // Counting, not encoding: no allocation however big it is. An entry that cannot be
        // counted would not encode either, so it is treated as too big.
        postcard::experimental::serialized_size(self).unwrap_or(usize::MAX)
    }

    /// Checks everything the Receiver must not take on trust: the limits, every path, and that
    /// no two entries clash.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.entries.is_empty() {
            return Err(ManifestError::Empty);
        }
        if self.entries.len() > MAX_ENTRIES {
            return Err(ManifestError::TooManyEntries);
        }
        if self.encoded_len() > MAX_ENCODED_LEN {
            return Err(ManifestError::TooLarge);
        }
        for (i, entry) in self.entries.iter().enumerate() {
            check_path(i, entry.path())?;
        }
        let mut seen = HashSet::with_capacity(self.entries.len());
        for (i, entry) in self.entries.iter().enumerate() {
            if !seen.insert(entry.path()) {
                return Err(ManifestError::Duplicate(i));
            }
        }
        // Every folder a path passes through is implied by it, so none of them may be an
        // entry of its own: a file is not a folder, and a folder that is listed as empty is
        // not holding this.
        for (i, entry) in self.entries.iter().enumerate() {
            let path = entry.path();
            if path.match_indices('/').any(|(at, _)| seen.contains(&path[..at])) {
                return Err(ManifestError::InsideEntry(i));
            }
        }
        Ok(())
    }
}

fn check_path(index: usize, path: &str) -> Result<(), ManifestError> {
    if path.len() > MAX_PATH_LEN {
        return Err(ManifestError::PathTooLong(index));
    }
    // An absolute path starts with an empty segment, so it is refused here as well.
    for segment in path.split('/') {
        match validate_file_name(segment) {
            Ok(()) => {}
            Err(NameError::TooLong) => return Err(ManifestError::NameTooLong(index)),
            Err(NameError::Empty | NameError::NotPlain) => {
                return Err(ManifestError::InvalidName(index));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(entries: Vec<Entry>) -> Manifest {
        Manifest { entries }
    }

    fn files(paths: &[&str]) -> Manifest {
        manifest(paths.iter().map(|p| Entry::file(*p, 1)).collect())
    }

    #[test]
    fn good_manifests_pass() {
        let good = [
            files(&["a.txt"]),
            files(&["a/b/c.txt", "a/d.txt", "e"]),
            manifest(vec![Entry::empty_dir("empty")]),
            manifest(vec![Entry::file("d/x", 0), Entry::empty_dir("d/e/f"), Entry::empty_dir("g")]),
            // Dots are fine inside a name, only a name that is just dots is not.
            files(&["..a", "a..", ".hidden", "a b/ünï.txt", "x/.../y"]),
            // Case-only differences are the received-names rules' business, not an error here.
            files(&["A.txt", "a.txt"]),
            // One entry may be a name's prefix without being its parent.
            files(&["a", "ab", "a.b"]),
        ];
        for m in good {
            assert_eq!(m.validate(), Ok(()), "{m:?}");
        }
    }

    #[test]
    fn hostile_and_malformed_paths_are_refused() {
        let bad: &[(&str, &str)] = &[
            ("empty path", ""),
            ("absolute", "/etc/passwd"),
            ("absolute, windows style", "\\windows\\system32"),
            ("drive and backslash", "C:\\x"),
            ("parent", ".."),
            ("parent first", "../evil"),
            ("parent in the middle", "a/../../evil"),
            ("parent last", "a/.."),
            ("current dir", "."),
            ("current dir in the middle", "a/./b"),
            ("empty segment", "a//b"),
            ("trailing slash", "a/"),
            ("NUL", "a\0b"),
            ("NUL at the end", "a/b\0"),
            ("newline", "a\nb"),
            ("tab", "a\tb"),
            ("escape", "a\u{1b}[31mb"),
            ("DEL", "a\u{7f}"),
            ("C1 control", "a\u{85}b"),
            ("backslash separator", "a\\b"),
            ("backslash traversal", "a\\..\\b"),
        ];
        for (what, path) in bad {
            for entry in [Entry::file(*path, 1), Entry::empty_dir(*path)] {
                let m = manifest(vec![Entry::file("fine", 1), entry]);
                assert_eq!(m.validate(), Err(ManifestError::InvalidName(1)), "{what}: {path:?}");
            }
        }
    }

    #[test]
    fn the_path_and_name_limits_are_inclusive() {
        let name = "n".repeat(255);
        assert_eq!(files(&[&name]).validate(), Ok(()));
        assert_eq!(files(&[&"n".repeat(256)]).validate(), Err(ManifestError::NameTooLong(0)));
        // Multi-byte characters count as their bytes: 128 two-byte characters are 256 bytes.
        assert_eq!(files(&[&"é".repeat(127)]).validate(), Ok(()));
        assert_eq!(files(&[&"é".repeat(128)]).validate(), Err(ManifestError::NameTooLong(0)));

        // 4096 bytes of path made of names of at most 255 bytes and separators.
        let segment = "s".repeat(255);
        let mut path = vec![segment.as_str(); 15].join("/");
        path.push_str(&format!("/{}/{}", "t".repeat(100), "u".repeat(155)));
        assert_eq!(path.len(), MAX_PATH_LEN);
        assert_eq!(files(&[&path]).validate(), Ok(()));
        path.push('u');
        assert_eq!(files(&[&path]).validate(), Err(ManifestError::PathTooLong(0)));
    }

    #[test]
    fn a_path_that_is_too_long_is_refused_before_its_names_are_looked_at() {
        let long = format!("{}/{}", "a".repeat(2500), "b".repeat(2500));
        assert_eq!(files(&[&long]).validate(), Err(ManifestError::PathTooLong(0)));
    }

    #[test]
    fn the_entry_limit_is_inclusive() {
        let at_limit = Manifest { entries: (0..MAX_ENTRIES).map(|i| Entry::file(i.to_string(), 0)).collect() };
        assert_eq!(at_limit.validate(), Ok(()));
        let mut over = at_limit;
        over.entries.push(Entry::file("one more", 0));
        assert_eq!(over.validate(), Err(ManifestError::TooManyEntries));
    }

    #[test]
    fn an_empty_manifest_is_refused() {
        assert_eq!(Manifest::default().validate(), Err(ManifestError::Empty));
    }

    #[test]
    fn a_manifest_over_64_mib_encoded_is_refused_even_within_the_other_limits() {
        // 20,000 entries with paths of 4,095 bytes: far fewer than 500,000, each path legal,
        // and over 80 MB all together.
        let long = |i: usize| {
            let mut path = vec!["p".repeat(255); 15].join("/");
            path.push_str(&format!("/{i:0>255}"));
            path
        };
        let big = Manifest { entries: (0..20_000).map(|i| Entry::file(long(i), 1)).collect() };
        assert!(big.entries.iter().all(|e| e.path().len() == MAX_PATH_LEN - 1));
        assert!(big.encoded_len() > MAX_ENCODED_LEN);
        assert_eq!(big.validate(), Err(ManifestError::TooLarge));

        let fits = Manifest { entries: (0..10_000).map(|i| Entry::file(long(i), 1)).collect() };
        assert!(fits.encoded_len() < MAX_ENCODED_LEN);
        assert_eq!(fits.validate(), Ok(()));
    }

    #[test]
    fn duplicates_are_refused() {
        let dup: &[Manifest] = &[
            files(&["a", "a"]),
            files(&["a/b", "c", "a/b"]),
            manifest(vec![Entry::empty_dir("d"), Entry::empty_dir("d")]),
            // Same path, one a file and one an empty folder.
            manifest(vec![Entry::file("x", 1), Entry::empty_dir("x")]),
        ];
        for m in dup {
            assert!(matches!(m.validate(), Err(ManifestError::Duplicate(_))), "{m:?}");
        }
        assert_eq!(files(&["a", "b", "a"]).validate(), Err(ManifestError::Duplicate(2)));
    }

    #[test]
    fn an_entry_inside_a_file_or_an_empty_folder_is_refused() {
        let nested: &[Manifest] = &[
            // A file inside a "file".
            files(&["a", "a/b"]),
            files(&["a/b/c", "a/b"]),
            files(&["a/b", "a/b/c/d"]),
            // Something inside a folder that is said to be empty.
            manifest(vec![Entry::empty_dir("d"), Entry::file("d/x", 1)]),
            manifest(vec![Entry::empty_dir("d/e"), Entry::empty_dir("d/e/f")]),
        ];
        for m in nested {
            assert!(matches!(m.validate(), Err(ManifestError::InsideEntry(_))), "{m:?}");
        }
    }

    #[test]
    fn the_first_problem_is_the_one_reported_and_nothing_slips_through_among_good_entries() {
        let mut entries: Vec<Entry> = (0..100).map(|i| Entry::file(format!("d/{i}"), 1)).collect();
        entries.push(Entry::file("d/../x", 1));
        assert_eq!(manifest(entries).validate(), Err(ManifestError::InvalidName(100)));
    }

    #[test]
    fn what_the_receiver_shows_comes_from_the_manifest() {
        let m = manifest(vec![
            Entry::file("photos/a.jpg", 10),
            Entry::file("photos/b.jpg", 5),
            Entry::empty_dir("photos/empty"),
            Entry::file("notes.txt", 7),
            Entry::empty_dir("zzz"),
        ]);
        assert_eq!(m.top_level_items(), ["photos", "notes.txt", "zzz"]);
        assert_eq!(m.file_count(), 3);
        assert_eq!(m.total_size(), Some(22));
        assert_eq!(m.files().collect::<Vec<_>>(), [("photos/a.jpg", 10), ("photos/b.jpg", 5), ("notes.txt", 7)]);
    }

    #[test]
    fn sizes_that_overflow_have_no_total() {
        let m = manifest(vec![Entry::file("a", u64::MAX), Entry::file("b", 1)]);
        assert_eq!(m.total_size(), None);
    }

    #[test]
    fn a_manifest_survives_the_wire() {
        let m = manifest(vec![
            Entry::File { path: "bin/run".into(), size: 1 << 40, mtime_ns: 1_700_000_000_123_456_789, executable: true },
            Entry::empty_dir("empty"),
        ]);
        let bytes = postcard::to_stdvec(&m).unwrap();
        assert_eq!(bytes.len(), m.encoded_len());
        assert_eq!(postcard::from_bytes::<Manifest>(&bytes).unwrap(), m);
    }
}
