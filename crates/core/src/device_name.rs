//! The Device Name: the label an owner gives their Device, shown to others in place of the
//! Device ID. It travels in `Hello`, so only a Device that can connect (one holding this
//! Device's ID) ever sees it. Names from other Devices are untrusted text.

use crate::contacts::MAX_NAME_CHARS;

/// The setting the Device Name is stored under.
pub(crate) const SETTING: &str = "device_name";

/// Used when the machine has no usable hostname.
const FALLBACK: &str = "My Device";

/// Cleans a name for storing or showing: control characters dropped, whitespace trimmed, cut to
/// [`MAX_NAME_CHARS`] characters. `None` if nothing is left. Unlike a Contact's Nickname, a
/// too-long name is shortened rather than refused, since the user did not type a peer's.
pub(crate) fn sanitize(name: &str) -> Option<String> {
    let cleaned: String = name.chars().filter(|c| !c.is_control()).collect();
    let cut: String = cleaned.trim().chars().take(MAX_NAME_CHARS).collect();
    let cut = cut.trim_end();
    (!cut.is_empty()).then(|| cut.to_owned())
}

/// `name` cut to at most `max` bytes, on a character boundary.
pub(crate) fn truncate_bytes(name: &str, max: usize) -> &str {
    let mut end = name.len().min(max);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    &name[..end]
}

/// What a new install calls itself: the machine's hostname.
pub(crate) fn default_name() -> String {
    hostname().as_deref().and_then(sanitize).unwrap_or_else(|| FALLBACK.to_owned())
}

#[cfg(unix)]
fn hostname() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: `buf` is valid for `buf.len()` bytes; gethostname writes at most that many.
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    Some(String::from_utf8_lossy(&buf[..end]).into_owned())
}

#[cfg(not(unix))]
fn hostname() -> Option<String> {
    std::env::var("COMPUTERNAME").ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_trimmed_stripped_and_shortened() {
        assert_eq!(sanitize("  Mum's laptop \n"), Some("Mum's laptop".to_owned()));
        assert_eq!(sanitize("a\u{0}b\tc"), Some("abc".to_owned()));
        assert_eq!(sanitize(" \n "), None);
        assert_eq!(sanitize(&"é".repeat(100)).unwrap().chars().count(), 64);
        // Cutting must not leave trailing whitespace.
        assert_eq!(sanitize(&format!("{} tail", "x".repeat(63))), Some("x".repeat(63)));
    }

    #[test]
    fn the_default_name_is_never_empty() {
        let name = default_name();
        assert!(!name.is_empty() && name.chars().count() <= MAX_NAME_CHARS);
    }
}
