//! File names that cross the trust boundary (spec section 6). [`validate_file_name`] refuses
//! what could name a place outside the save folder. What passes is then made safe to write on
//! every operating system by [`adjust_names`], the same way everywhere: a name valid only on
//! some systems would otherwise change meaning, or fail, depending on where it arrives.

use std::collections::{HashMap, HashSet};

use crate::manifest::{Entry, Manifest};

/// Longest accepted file name, in bytes.
pub const MAX_NAME_LEN: usize = 255;

/// Why a name was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NameError {
    #[error("the file name is empty")]
    Empty,
    #[error("the file name is longer than {MAX_NAME_LEN} bytes")]
    TooLong,
    #[error("the file name is not a plain file name")]
    NotPlain,
}

/// A name is acceptable only if it is one ordinary path component: it cannot climb out of the
/// save folder or carry a directory, on any operating system.
pub fn validate_file_name(name: &str) -> Result<(), NameError> {
    if name.is_empty() {
        return Err(NameError::Empty);
    }
    if name.len() > MAX_NAME_LEN {
        return Err(NameError::TooLong);
    }
    if name == "." || name == ".." || name.chars().any(|c| c == '/' || c == '\\' || c.is_control())
    {
        return Err(NameError::NotPlain);
    }
    Ok(())
}

/// The `n`th alternative to a name that is already taken: `a.txt` becomes `a (1).txt`. The
/// result is never longer than [`MAX_NAME_LEN`]: a name that is already that long loses the end
/// of its stem to make room for the number.
pub fn numbered(name: &str, n: u32) -> String {
    let marker = format!(" ({n})");
    let (stem, ext) = match name.rfind('.') {
        // A leading dot is part of the stem (".bashrc"), not an extension.
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name, ""),
    };
    // An extension so long that not even one character of the stem fits beside it is treated
    // as part of the stem, which is then cut.
    let (stem, ext) = if ext.len() + marker.len() + 4 > MAX_NAME_LEN { (name, "") } else { (stem, ext) };
    let stem = truncated(stem, MAX_NAME_LEN - ext.len() - marker.len());
    format!("{stem}{marker}{ext}")
}

/// The longest start of `s` that is at most `max` bytes and ends on a character boundary.
fn truncated(s: &str, max: usize) -> &str {
    let mut end = max.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Characters Windows does not allow in a name, besides the separators and control characters
/// that [`validate_file_name`] has already refused.
const WINDOWS_RESERVED_CHARS: [char; 7] = ['<', '>', ':', '"', '|', '?', '*'];

/// Whether `stem` (a name up to its first dot) is one of the names Windows reserves for
/// devices, whatever the case and whatever extension follows: CON, PRN, AUX, NUL, COM1-9 and
/// LPT1-9. The superscript digits (COM¹ to LPT³) are reserved on current Windows too.
fn is_device_name(stem: &str) -> bool {
    let stem = stem.trim_end_matches(' ');
    if ["CON", "PRN", "AUX", "NUL"].iter().any(|device| stem.eq_ignore_ascii_case(device)) {
        return true;
    }
    let mut chars = stem.chars();
    let Some(last) = chars.next_back() else { return false };
    let head = chars.as_str();
    (head.eq_ignore_ascii_case("COM") || head.eq_ignore_ascii_case("LPT"))
        && matches!(last, '1'..='9' | '¹' | '²' | '³')
}

/// A name that is valid on Windows as well as everywhere else. `< > : " | ? *` become `_`;
/// trailing dots and spaces, which Windows drops, are trimmed (a name that is nothing else
/// becomes `_`); a reserved device name gets a `_` after its stem (`CON.txt` becomes
/// `CON_.txt`). Anything else, Unicode included, is left alone.
///
/// The input is a validated name of at most [`MAX_NAME_LEN`] bytes and so is the result.
pub fn safe_name(name: &str) -> String {
    let replaced: String =
        name.chars().map(|c| if WINDOWS_RESERVED_CHARS.contains(&c) { '_' } else { c }).collect();
    let trimmed = replaced.trim_end_matches(['.', ' ']);
    if trimmed.is_empty() {
        return "_".to_owned();
    }
    let (stem, rest) = trimmed.split_at(trimmed.find('.').unwrap_or(trimmed.len()));
    if !is_device_name(stem) {
        return trimmed.to_owned();
    }
    // Only a device name followed by a very long extension can get past the limit here.
    let adjusted = format!("{stem}_{rest}");
    truncated(&adjusted, MAX_NAME_LEN).trim_end_matches(['.', ' ']).to_owned()
}

/// How two names are compared for clashes: a case-insensitive filesystem treats names that
/// differ only in case as one.
fn fold(name: &str) -> String {
    name.to_lowercase()
}

/// An Offer's manifest with every name made safe, and how many names that changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adjusted {
    /// Same entries in the same order, so the n-th file is still the n-th file.
    pub manifest: Manifest,
    /// Names changed, counting a folder once however many entries are inside it. Counted
    /// here are the ones [`safe_name`] changed and the ones renamed to keep two apart.
    pub count: u32,
}

/// The names that live directly in one folder of the tree being received.
#[derive(Default)]
struct Folder {
    /// Names taken, folded, so a case-only difference counts as a clash.
    taken: HashSet<String>,
    /// What each name the Sender used became, and the folder it is, if it is one.
    renamed: HashMap<String, (String, Option<usize>)>,
}

/// Makes every name in `manifest` safe to write (spec section 6): [`safe_name`] on each, then
/// within one folder a name that is already taken, ignoring case, gets a number (`a (1)`,
/// `a (2)`, ...), the later one of the two. Names that are identical only after the first step
/// (`a:b` and `a_b`) are kept apart the same way. `kept_clear` is a name nothing at the top
/// level may take, ignoring case: the incoming store.
///
/// Pure and deterministic: it gives the same answer every time it is asked about the same
/// manifest, so what the Receiver showed and what it builds cannot differ.
pub fn adjust_names(manifest: &Manifest, kept_clear: &str) -> Adjusted {
    let mut folders = vec![Folder::default()];
    folders[0].taken.insert(fold(kept_clear));
    let mut count = 0u32;
    let entries = manifest
        .entries
        .iter()
        .map(|entry| {
            let segments: Vec<&str> = entry.path().split('/').collect();
            let mut at = 0;
            let mut path = Vec::with_capacity(segments.len());
            for (i, segment) in segments.iter().enumerate() {
                if let Some((name, next)) = folders[at].renamed.get(*segment) {
                    path.push(name.clone());
                    at = next.unwrap_or(at);
                    continue;
                }
                let name = unused(&folders[at].taken, safe_name(segment));
                if name != *segment {
                    count = count.saturating_add(1);
                }
                folders[at].taken.insert(fold(&name));
                // The last segment is a file or an empty folder: nothing goes inside it.
                let next = (i + 1 < segments.len()).then(|| {
                    folders.push(Folder::default());
                    folders.len() - 1
                });
                folders[at].renamed.insert((*segment).to_owned(), (name.clone(), next));
                path.push(name);
                at = next.unwrap_or(at);
            }
            let path = path.join("/");
            match entry {
                Entry::File { size, mtime_ns, executable, .. } => {
                    Entry::File { path, size: *size, mtime_ns: *mtime_ns, executable: *executable }
                }
                Entry::EmptyDir { .. } => Entry::EmptyDir { path },
            }
        })
        .collect();
    Adjusted { manifest: Manifest { entries }, count }
}

/// `name` if no name in `taken` is the same, ignoring case, else the first of its numbered
/// alternatives that is free.
fn unused(taken: &HashSet<String>, name: String) -> String {
    if !taken.contains(&fold(&name)) {
        return name;
    }
    (1u32..)
        .map(|n| numbered(&name, n))
        .find(|candidate| !taken.contains(&fold(candidate)))
        .expect("there are more numbers than names")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_names() {
        for name in ["a.txt", "photo (1).jpg", ".bashrc", "ünï cödé.txt", "no extension", "..a", "a.."] {
            assert_eq!(validate_file_name(name), Ok(()), "{name}");
        }
    }

    #[test]
    fn rejects_traversal_separators_and_control_characters() {
        for name in [
            "", ".", "..", "../x", "a/b", "/abs", "a\\b", "..\\x", "C:\\x", "nul\0byte", "tab\tname",
            "new\nline",
        ] {
            assert!(validate_file_name(name).is_err(), "{name:?}");
        }
    }

    #[test]
    fn rejects_over_long_names() {
        assert_eq!(validate_file_name(&"a".repeat(255)), Ok(()));
        assert_eq!(validate_file_name(&"a".repeat(256)), Err(NameError::TooLong));
    }

    #[test]
    fn numbering_keeps_the_extension() {
        assert_eq!(numbered("a.txt", 1), "a (1).txt");
        assert_eq!(numbered("archive.tar.gz", 2), "archive.tar (2).gz");
        assert_eq!(numbered("README", 1), "README (1)");
        assert_eq!(numbered(".bashrc", 1), ".bashrc (1)");
    }

    #[test]
    fn numbering_stays_within_the_name_limit() {
        let cases: Vec<(&str, String, u32, String)> = vec![
            ("plain", "n".repeat(255), 1, format!("{} (1)", "n".repeat(251))),
            ("with an extension", format!("{}.txt", "n".repeat(251)), 12, format!("{} (12).txt", "n".repeat(246))),
            // The cut is on a character boundary: 251 bytes allow 125 two-byte characters.
            ("multi-byte", format!("{}x", "é".repeat(127)), 3, format!("{} (3)", "é".repeat(125))),
            // An extension that leaves no room is not kept apart from the stem.
            ("huge extension", format!("a.{}", "e".repeat(253)), 1, format!("a.{} (1)", "e".repeat(249))),
            ("fits as it is", "a.txt".into(), 1, "a (1).txt".into()),
        ];
        for (what, name, n, want) in cases {
            let got = numbered(&name, n);
            assert!(got.len() <= MAX_NAME_LEN, "{what}: {} bytes", got.len());
            assert_eq!(got, want, "{what}");
        }
    }

    /// Every rule of `safe_name`, one row each: the name as sent, and what it becomes.
    #[test]
    fn safe_names_table() {
        let cases: &[(&str, &str, &str)] = &[
            // Names that are fine are untouched.
            ("ordinary", "report.pdf", "report.pdf"),
            ("spaces inside", "my file .txt", "my file .txt"),
            ("leading dot", ".bashrc", ".bashrc"),
            ("leading space", " lead", " lead"),
            ("dots inside", "a..b", "a..b"),
            ("a name containing a device name", "console.txt", "console.txt"),
            ("device name plus a letter", "CONX", "CONX"),
            ("device name with a dot first", ".CON", ".CON"),
            ("COM0 is not a device", "COM0", "COM0"),
            ("COM10 is not a device", "COM10", "COM10"),
            ("COM alone is not a device", "COM", "COM"),
            ("LPT alone", "lpt", "lpt"),
            ("a device name in the middle", "a.CON", "a.CON"),
            // The reserved characters, each on its own and all together.
            ("less than", "a<b", "a_b"),
            ("greater than", "a>b", "a_b"),
            ("colon", "a:b", "a_b"),
            ("double quote", "a\"b", "a_b"),
            ("pipe", "a|b", "a_b"),
            ("question mark", "what?.txt", "what_.txt"),
            ("asterisk", "*.txt", "_.txt"),
            ("all of them", "<>:\"|?*", "_______"),
            ("a time of day", "12:30:45.log", "12_30_45.log"),
            // Trailing dots and spaces.
            ("one trailing dot", "a.", "a"),
            ("many trailing dots", "a...", "a"),
            ("one trailing space", "a ", "a"),
            ("trailing dots and spaces mixed", "a. . ", "a"),
            ("an extension then a dot", "a.txt.", "a.txt"),
            ("only dots", "...", "_"),
            ("only spaces", "   ", "_"),
            ("dots and spaces", ". .", "_"),
            ("a reserved character that is then trailing", "a:", "a_"),
            // Reserved device names, any case, with and without an extension.
            ("CON", "CON", "CON_"),
            ("PRN", "PRN", "PRN_"),
            ("AUX", "AUX", "AUX_"),
            ("NUL", "NUL", "NUL_"),
            ("lower case", "con", "con_"),
            ("mixed case", "NuL", "NuL_"),
            ("with an extension", "CON.txt", "CON_.txt"),
            ("with two extensions", "aux.tar.gz", "aux_.tar.gz"),
            ("with an empty-looking extension", "prn.", "prn_"),
            ("COM1", "COM1", "COM1_"),
            ("COM9 with an extension", "com9.log", "com9_.log"),
            ("LPT1", "LPT1", "LPT1_"),
            ("LPT9 with an extension", "Lpt9.txt", "Lpt9_.txt"),
            ("a space before the dot", "NUL .txt", "NUL _.txt"),
            ("a trailing dot is trimmed first", "CON.", "CON_"),
            ("superscript digits", "COM¹", "COM¹_"),
            ("superscript digit with an extension", "lpt³.txt", "lpt³_.txt"),
            // Unicode is not touched, whatever it looks like.
            ("accents", "café.txt", "café.txt"),
            ("CJK", "文件.txt", "文件.txt"),
            ("emoji", "party 🎉.png", "party 🎉.png"),
            ("right-to-left", "שלום.txt", "שלום.txt"),
            ("full-width look-alikes are fine", "a＜b＞c：d.txt", "a＜b＞c：d.txt"),
            ("full-width device name", "ＣＯＮ", "ＣＯＮ"),
            ("a Unicode space at the end is not trimmed", "a\u{a0}", "a\u{a0}"),
            ("an emoji then dots", "🎉..", "🎉"),
        ];
        for (what, name, want) in cases {
            assert_eq!(safe_name(name), *want, "{what}: {name:?}");
        }
    }

    #[test]
    fn safe_names_stay_within_the_length_limit_and_are_stable() {
        // The longest name there is, with every rule that can change its length.
        let long_ext = format!("CON.{}", "x".repeat(251));
        assert_eq!(long_ext.len(), 255);
        let got = safe_name(&long_ext);
        assert!(got.len() <= MAX_NAME_LEN, "{} bytes", got.len());
        assert!(got.starts_with("CON_.xxx"));
        // A cut that would end on a dot does not leave one.
        let dotted = format!("CON.{}.y", "x".repeat(249));
        let got = safe_name(&dotted);
        assert!(got.len() <= MAX_NAME_LEN && !got.ends_with('.'), "{got}");

        // Names that need no change stay as they are, at the limit too.
        for name in ["a".repeat(255), "é".repeat(127), format!("{}.txt", "é".repeat(125))] {
            assert_eq!(safe_name(&name), name);
        }
        // Adjusting twice changes nothing more.
        for name in ["CON", "a:b?", "...", "aux.txt", "ok", "x. "] {
            let once = safe_name(name);
            assert_eq!(safe_name(&once), once, "{name:?}");
        }
    }

    #[test]
    fn device_names_are_matched_whole_and_by_their_stem() {
        for yes in ["CON", "con", "NUL", "COM1", "com9", "LPT5", "COM²", "CON "] {
            assert!(is_device_name(yes), "{yes:?}");
        }
        for no in ["", "CO", "CONN", "COM0", "COM10", "COMA", "LPT", "XCON", "COM1X", "ＣＯＮ"] {
            assert!(!is_device_name(no), "{no:?}");
        }
    }

    fn files(paths: &[&str]) -> Manifest {
        Manifest { entries: paths.iter().map(|p| Entry::file(*p, 1)).collect() }
    }

    fn adjusted(paths: &[&str]) -> (Vec<String>, u32) {
        let Adjusted { manifest, count } = adjust_names(&files(paths), ".incoming");
        (manifest.entries.iter().map(|e| e.path().to_owned()).collect(), count)
    }

    /// Whole trees: what each entry becomes and how many names changed.
    #[test]
    fn adjusting_a_tree_table() {
        let cases: &[(&str, &[&str], &[&str], u32)] = &[
            ("nothing to do", &["a.txt", "d/b.txt"], &["a.txt", "d/b.txt"], 0),
            ("one file", &["a:b.txt"], &["a_b.txt"], 1),
            // A folder is one name, however many files are inside it.
            ("a folder counts once", &["d:/a", "d:/b", "d:/c/x"], &["d_/a", "d_/b", "d_/c/x"], 1),
            ("every segment of a path", &["CON/aux.txt/q?"], &["CON_/aux_.txt/q_"], 3),
            ("a trailing dot on a folder", &["d./f"], &["d/f"], 1),
            // Names that become identical must not collide.
            ("two names that become one", &["a:b", "a_b"], &["a_b", "a_b (1)"], 2),
            ("three names that become one", &["a?", "a*", "a_"], &["a_", "a_ (1)", "a_ (2)"], 3),
            ("a trailing dot and the plain name", &["a.", "a"], &["a", "a (1)"], 2),
            ("two folders that become one stay two", &["d:/x", "d_/y"], &["d_/x", "d_ (1)/y"], 2),
            // Case-only clashes: the later one is numbered.
            ("case-only clash", &["a.txt", "A.txt"], &["a.txt", "A (1).txt"], 1),
            ("later one is the one numbered", &["B", "b"], &["B", "b (1)"], 1),
            ("three in a row", &["ab", "AB", "aB"], &["ab", "AB (1)", "aB (2)"], 2),
            ("non-ASCII case", &["Ünï.txt", "ünï.txt"], &["Ünï.txt", "ünï (1).txt"], 1),
            ("an extension in another case is another name", &["a.TXT", "a.txt"], &["a.TXT", "a (1).txt"], 1),
            ("folders differing in case stay two folders", &["Photos/a", "photos/b", "photos/c", "Photos/d"],
                &["Photos/a", "photos (1)/b", "photos (1)/c", "Photos/d"], 1),
            ("clashing at depth", &["d/a", "d/A", "e/A", "e/a"], &["d/a", "d/A (1)", "e/A", "e/a (1)"], 2),
            ("the same name in different folders is no clash", &["a/x", "b/x", "X"], &["a/x", "b/x", "X"], 0),
            ("a number that is already used", &["a", "A", "A (1)"], &["a", "A (1)", "A (1) (1)"], 2),
            ("a number taken by another case", &["a (1)", "a", "A"], &["a (1)", "a", "A (2)"], 1),
            // Both steps together.
            ("a reserved name and a case clash", &["con", "CON"], &["con_", "CON_ (1)"], 2),
            ("device name beside its adjusted spelling", &["CON", "CON_"], &["CON_", "CON_ (1)"], 2),
            // The incoming store's name is kept clear at the top level only.
            ("the incoming store", &[".incoming/x"], &[".incoming (1)/x"], 1),
            ("the incoming store in another case", &[".INCOMING", "d/.incoming"], &[".INCOMING (1)", "d/.incoming"], 1),
        ];
        for (what, paths, want, changed) in cases {
            let (got, count) = adjusted(paths);
            assert_eq!(got, *want, "{what}");
            assert_eq!(count, *changed, "{what}");
        }
    }

    #[test]
    fn adjusting_keeps_what_is_not_a_name() {
        let manifest = Manifest {
            entries: vec![
                Entry::File { path: "d:/run".into(), size: 7, mtime_ns: 99, executable: true },
                Entry::empty_dir("d:/e?"),
                Entry::empty_dir("x"),
            ],
        };
        let Adjusted { manifest: got, count } = adjust_names(&manifest, ".incoming");
        assert_eq!(count, 2);
        assert_eq!(
            got.entries,
            [
                Entry::File { path: "d_/run".into(), size: 7, mtime_ns: 99, executable: true },
                Entry::empty_dir("d_/e_"),
                Entry::empty_dir("x"),
            ]
        );
        // The same entries in the same order: the n-th file is still the n-th file.
        assert!(manifest.files().map(|(_, size)| size).eq(got.files().map(|(_, size)| size)));
    }

    #[test]
    fn adjusting_is_deterministic_and_leaves_a_valid_unique_tree() {
        let paths = ["CON", "con", "a:b", "a_b", "A", "a", "d./x", "d/x", "D/y", "...", "_", "x.", "x"];
        let (first, count) = adjusted(&paths);
        assert_eq!(adjusted(&paths), (first.clone(), count));
        let mut seen = HashSet::new();
        for path in &first {
            for segment in path.split('/') {
                assert_eq!(validate_file_name(segment), Ok(()), "{path}");
                assert_eq!(safe_name(segment), segment, "{path}");
            }
            // No two entries are the same, ignoring case.
            assert!(seen.insert(fold(path)), "{path} twice");
        }
    }

    #[test]
    fn a_number_that_makes_a_name_too_long_is_cut_from_the_stem() {
        let long = "n".repeat(255);
        let (got, count) = adjusted(&[&long, &long.to_uppercase()]);
        assert_eq!(got[0], long);
        assert!(got[1].len() <= MAX_NAME_LEN);
        assert!(got[1].ends_with(" (1)"));
        assert_ne!(fold(&got[1]), fold(&got[0]));
        assert_eq!(count, 1);
    }
}
