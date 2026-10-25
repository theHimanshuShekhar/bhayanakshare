//! Why a Transfer fails, in the plain English the core stores and sends: every reason in one
//! list.
//!
//! A reason is free text in [`TransferState::Failed`](crate::TransferState), kept in the
//! database and sent to the other Device in a `Failed` frame, so old History rows and other
//! versions hold these very texts: **never reword one**. The UI and the shell do not show them.
//! They find a reason in [`FAILURE_REASONS`] and say its cause in the error-message catalogue's
//! words (`ui/src/failureReasons.ts` is this list, generated for the UI).

use crate::event::Outdated;

pub(crate) const LOST: &str = "The other Device went away.";
pub(crate) const UNEXPECTED: &str = "The other Device sent something unexpected.";
pub(crate) const BUSY: &str =
    "The other Device already has too many Offers from you waiting for an answer. Try again once it has answered some.";
pub(crate) const INVALID_NAMES: &str = "Couldn't be sent: invalid file names";
pub(crate) const STALLED: &str = "The Transfer made no progress for 24 hours, so it was given up.";
pub(crate) const RESTARTED: &str = "This Device restarted before the Transfer could carry on.";
pub(crate) const FORGOTTEN: &str = "The other Device no longer has this Transfer.";
pub(crate) const UNREACHABLE: &str = "Could not reach the receiving Device. It may be offline, or one of you may have no internet connection.";
pub(crate) const BAD_OFFER: &str = "The other Device sent an invalid Offer.";
pub(crate) const CANT_FETCH: &str = "Could not download the files from the sending Device.";
pub(crate) const WRONG_FILE: &str = "The sending Device sent different files than it offered.";
pub(crate) const TOO_MUCH: &str = "The sending Device sent more than it offered.";
pub(crate) const CANT_SAVE: &str = "Could not save the files to the save folder.";
pub(crate) const CANT_KEEP_TEXT: &str = "Could not keep the text.";
pub(crate) const CANT_OPEN: &str = "Could not reach the receiving Device.";
pub(crate) const CANT_RECORD: &str = "Could not record the Offer.";
pub(crate) const CANT_PREPARE_SPACE: &str = "Could not prepare space to receive the file.";
pub(crate) const CANT_LOOK_UP: &str = "Could not look up the Transfer.";
pub(crate) const CANT_CHECK_FILES: &str = "Could not check the files.";
pub(crate) const CANT_READ_FILES: &str = "Could not read the files.";
pub(crate) const CANT_PREPARE_FILES: &str = "Could not prepare the files for sending.";

/// What follows `A file changed on the sending Device: ` is the name of the file.
pub(crate) const CHANGED_PREFIX: &str = "A file changed on the sending Device: ";
/// A refusal for the version names the Device (its own name, or "The other Device") first.
pub(crate) const PEER_OLDER_SUFFIX: &str = " is running an older BhayanakShare. Ask them to update.";
pub(crate) const THIS_OLDER_SUFFIX: &str = " is running a newer BhayanakShare. Update this Device, then try again.";

/// What a reason is written as, and so how it is found in a stored text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Matching {
    /// The reason is exactly this text.
    Exact,
    /// The reason starts with this text, and ends with something that varies (a file name).
    Prefix,
    /// The reason ends with this text, and starts with something that varies (a Device's name).
    Suffix,
}

/// One kind of reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FailureReason {
    /// Stable, snake_case: what the UI and the shell key their wording on.
    pub id: &'static str,
    /// The text as stored and sent, or the fixed part of it.
    pub text: &'static str,
    pub matching: Matching,
}

const fn exact(id: &'static str, text: &'static str) -> FailureReason {
    FailureReason { id, text, matching: Matching::Exact }
}

/// Every reason the core gives, as it is stored and sent.
pub const FAILURE_REASONS: &[FailureReason] = &[
    exact("unreachable", UNREACHABLE),
    exact("lost", LOST),
    exact("unexpected", UNEXPECTED),
    exact("busy", BUSY),
    exact("invalid_names", INVALID_NAMES),
    exact("stalled", STALLED),
    exact("restarted", RESTARTED),
    exact("forgotten", FORGOTTEN),
    exact("bad_offer", BAD_OFFER),
    exact("cant_fetch", CANT_FETCH),
    exact("wrong_file", WRONG_FILE),
    exact("too_much", TOO_MUCH),
    exact("cant_save", CANT_SAVE),
    exact("cant_keep_text", CANT_KEEP_TEXT),
    exact("cant_open", CANT_OPEN),
    exact("cant_record", CANT_RECORD),
    exact("cant_prepare_space", CANT_PREPARE_SPACE),
    exact("cant_look_up", CANT_LOOK_UP),
    exact("cant_check_files", CANT_CHECK_FILES),
    exact("cant_read_files", CANT_READ_FILES),
    exact("cant_prepare_files", CANT_PREPARE_FILES),
    FailureReason { id: "file_changed", text: CHANGED_PREFIX, matching: Matching::Prefix },
    FailureReason { id: "peer_older", text: PEER_OLDER_SUFFIX, matching: Matching::Suffix },
    FailureReason { id: "this_older", text: THIS_OLDER_SUFFIX, matching: Matching::Suffix },
];

/// Which kind of reason `reason` is, and the part of it that varies (the file's name, the
/// Device's name; empty for an exact one). `None` for a reason of any other kind, such as one a
/// newer version made or one that is only in the log.
pub fn identify_failure(reason: &str) -> Option<(&'static FailureReason, &str)> {
    FAILURE_REASONS.iter().find_map(|known| {
        let rest = match known.matching {
            Matching::Exact => (reason == known.text).then_some(""),
            Matching::Prefix => reason.strip_prefix(known.text),
            Matching::Suffix => reason.strip_suffix(known.text),
        };
        rest.map(|rest| (known, rest))
    })
}

/// What a Transfer that was refused for its versions says, naming the Device if it has a name.
pub(crate) fn mismatch_reason(outdated: Outdated, name: Option<&str>) -> String {
    let who = name.unwrap_or("The other Device");
    match outdated {
        Outdated::Peer => format!("{who}{PEER_OLDER_SUFFIX}"),
        Outdated::ThisDevice => format!("{who}{THIS_OLDER_SUFFIX}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_snake_case() {
        for (i, a) in FAILURE_REASONS.iter().enumerate() {
            assert!(!a.id.is_empty() && a.id.chars().all(|c| c.is_ascii_lowercase() || c == '_'), "{}", a.id);
            assert!(FAILURE_REASONS[i + 1..].iter().all(|b| b.id != a.id), "{} twice", a.id);
        }
    }

    #[test]
    fn every_reason_is_found_by_its_own_text() {
        for known in FAILURE_REASONS {
            let sample = match known.matching {
                Matching::Exact => known.text.to_owned(),
                Matching::Prefix => format!("{}notes.txt", known.text),
                Matching::Suffix => format!("Alice's Laptop{}", known.text),
            };
            let (found, rest) = identify_failure(&sample).unwrap();
            assert_eq!(found.id, known.id);
            assert_eq!(rest, match known.matching {
                Matching::Exact => "",
                Matching::Prefix => "notes.txt",
                Matching::Suffix => "Alice's Laptop",
            });
        }
        assert_eq!(identify_failure("Something nobody wrote down."), None);
    }

    #[test]
    fn the_texts_that_were_stored_before_are_unchanged() {
        let find = |id: &str| FAILURE_REASONS.iter().find(|r| r.id == id).unwrap().text;
        assert_eq!(find("lost"), "The other Device went away.");
        assert_eq!(find("invalid_names"), "Couldn't be sent: invalid file names");
        assert_eq!(find("file_changed"), "A file changed on the sending Device: ");
        assert_eq!(
            find("busy"),
            "The other Device already has too many Offers from you waiting for an answer. Try again once it has answered some."
        );
    }

    #[test]
    fn a_refusal_says_who_has_to_update() {
        assert_eq!(
            mismatch_reason(Outdated::Peer, Some("Alice's Laptop")),
            "Alice's Laptop is running an older BhayanakShare. Ask them to update."
        );
        assert_eq!(
            mismatch_reason(Outdated::ThisDevice, None),
            "The other Device is running a newer BhayanakShare. Update this Device, then try again."
        );
    }

    #[test]
    fn a_refusal_for_the_version_is_found_whoever_it_names() {
        for (outdated, id) in [(Outdated::Peer, "peer_older"), (Outdated::ThisDevice, "this_older")] {
            for name in [Some("Alice's Laptop"), None] {
                let reason = mismatch_reason(outdated, name);
                let (found, rest) = identify_failure(&reason).unwrap();
                assert_eq!((found.id, rest), (id, name.unwrap_or("The other Device")));
            }
        }
    }
}
