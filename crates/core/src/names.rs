//! File names that cross the trust boundary. The skeleton handles one file, so a name is a
//! single path component; the full received-names policy (spec section 6) comes later.

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

/// The `n`th alternative to a name that is already taken: `a.txt` becomes `a (1).txt`.
pub fn numbered(name: &str, n: u32) -> String {
    match name.rfind('.') {
        // A leading dot is part of the stem (".bashrc"), not an extension.
        Some(dot) if dot > 0 => format!("{} ({n}){}", &name[..dot], &name[dot..]),
        _ => format!("{name} ({n})"),
    }
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
}
