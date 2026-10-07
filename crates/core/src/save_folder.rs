//! The save folder: where an accepted Transfer is saved unless the Receiver picks another folder
//! for that Offer. It is a setting, kept across restarts; until set, it is the folder the Device
//! was configured with.

use std::{
    io::{self, ErrorKind},
    path::{Path, PathBuf},
};

/// The setting the save folder is stored under, as an absolute path.
pub(crate) const SETTING: &str = "save_folder";

/// Why a folder cannot be the save folder. The reasons name no path: the user chose it, and an
/// error may be logged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SaveFolderProblem {
    /// There is something at that path that is not a folder, or no path at all.
    #[error("That is not a folder.")]
    NotAFolder,
    /// It does not exist, and could not be made.
    #[error("That folder does not exist and could not be created.")]
    CannotCreate,
    /// BhayanakShare cannot put files in it.
    #[error("BhayanakShare cannot write to that folder.")]
    NotWritable,
    /// The setting is kept as text, which that path is not.
    #[error("That folder's path cannot be used.")]
    NotText,
}

/// Makes `folder` if it is missing and checks that a file can be written in it. Returns it as an
/// absolute path, the form the setting is kept in.
pub(crate) fn prepare(folder: &Path) -> Result<PathBuf, SaveFolderProblem> {
    let folder = std::path::absolute(folder).map_err(|_| SaveFolderProblem::NotAFolder)?;
    if folder.to_str().is_none() {
        return Err(SaveFolderProblem::NotText);
    }
    if folder.exists() && !folder.is_dir() {
        return Err(SaveFolderProblem::NotAFolder);
    }
    std::fs::create_dir_all(&folder).map_err(|_| SaveFolderProblem::CannotCreate)?;
    probe(&folder).map_err(|_| SaveFolderProblem::NotWritable)?;
    Ok(folder)
}

/// Writes a file in `folder` and removes it again. A folder's permission bits do not say whether
/// writing works (a read-only mount, a quota, an ACL), so this tries.
fn probe(folder: &Path) -> io::Result<()> {
    let name = format!(".bhayanakshare-write-check-{}", std::process::id());
    let path = folder.join(name);
    loop {
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => return std::fs::remove_file(&path),
            // A leftover of an earlier check that was cut short.
            Err(e) if e.kind() == ErrorKind::AlreadyExists => std::fs::remove_file(&path)?,
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_folder_is_made_and_the_check_leaves_nothing_in_it() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("a").join("b");
        assert_eq!(prepare(&folder), Ok(folder.clone()));
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 0);
        // A folder that is there is fine, and so is a leftover of an earlier check.
        std::fs::write(folder.join(format!(".bhayanakshare-write-check-{}", std::process::id())), b"").unwrap();
        assert_eq!(prepare(&folder), Ok(folder.clone()));
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 0);
    }
}
