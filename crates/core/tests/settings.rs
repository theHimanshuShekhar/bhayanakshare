//! The settings of first run and Settings → Receiving through the Device API: whether first run
//! is done, and the save folder, which is a setting that is kept and applies to the next Offer
//! at once. (The Device Name, Visibility, public DHT and debug logging have their own files.)

mod support;

use std::path::PathBuf;

use bhayanakshare_core::{Error, SaveFolderProblem};
use support::TestDevice;

fn write_source(name: &str, bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
    let src = tempfile::tempdir().unwrap();
    let path = src.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    (src, path)
}

/// Sends `name` from `alice` to `bob` and has `bob` accept it without naming a folder.
async fn send_and_accept(alice: &mut TestDevice, bob: &mut TestDevice, name: &str) {
    let (_src, path) = write_source(name, b"some bytes");
    let id = alice.device.send_file(bob.addr(), &path).await.unwrap();
    bob.wait_offer().await;
    bob.device.accept(id).await.unwrap();
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;
}

#[tokio::test]
async fn first_run_is_needed_until_it_is_finished_and_stays_finished_after_a_restart() {
    let mut alice = TestDevice::start("alice").await;
    assert!(alice.device.needs_first_run().await.unwrap());
    // Looking does not finish it.
    assert!(alice.device.needs_first_run().await.unwrap());

    alice.device.finish_first_run().await.unwrap();
    assert!(!alice.device.needs_first_run().await.unwrap());
    alice.device.finish_first_run().await.unwrap();
    assert!(!alice.device.needs_first_run().await.unwrap(), "finishing twice undoes nothing");

    alice.restart().await;
    assert!(!alice.device.needs_first_run().await.unwrap(), "the flag is lost by a restart");
    alice.shutdown().await;
}

#[tokio::test]
async fn the_save_folder_is_the_configured_one_until_set() {
    let mut alice = TestDevice::start("alice").await;
    assert_eq!(alice.device.save_folder().await.unwrap(), alice.save_dir);
    alice.shutdown().await;
}

#[tokio::test]
async fn a_set_save_folder_is_made_if_missing_kept_across_a_restart_and_can_be_set_again() {
    let mut alice = TestDevice::start("alice").await;
    let elsewhere = tempfile::tempdir().unwrap();
    let new = elsewhere.path().join("received").join("files");
    assert!(!new.exists());

    let set = alice.device.set_save_folder(&new).await.unwrap();
    assert_eq!(set, new);
    assert!(new.is_dir(), "the folder is made");
    assert_eq!(alice.device.save_folder().await.unwrap(), new);

    alice.restart().await;
    assert_eq!(alice.device.save_folder().await.unwrap(), new, "the setting is lost by a restart");

    // A folder that was removed in the meantime is made again at the next start.
    std::fs::remove_dir_all(elsewhere.path().join("received")).unwrap();
    alice.restart().await;
    assert!(new.is_dir());

    let other = elsewhere.path().join("other");
    alice.device.set_save_folder(&other).await.unwrap();
    assert_eq!(alice.device.save_folder().await.unwrap(), other);
    alice.shutdown().await;
}

#[tokio::test]
async fn a_relative_save_folder_is_kept_as_the_absolute_path_it_means() {
    let mut alice = TestDevice::start("alice").await;
    let set = alice.device.set_save_folder(std::path::Path::new("bhs-relative-test")).await.unwrap();
    let wanted = std::path::absolute("bhs-relative-test").unwrap();
    // Made where the process runs: clean up before asserting.
    let _ = std::fs::remove_dir(&wanted);
    assert_eq!(set, wanted);
    assert!(set.is_absolute());
    alice.shutdown().await;
}

#[tokio::test]
async fn a_folder_that_cannot_be_used_is_refused_with_the_reason_and_changes_nothing() {
    let mut alice = TestDevice::start("alice").await;
    let before = alice.device.save_folder().await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("a-file");
    std::fs::write(&file, b"x").unwrap();

    // A file is not a folder.
    let err = alice.device.set_save_folder(&file).await.unwrap_err();
    assert!(matches!(err, Error::SaveFolder(SaveFolderProblem::NotAFolder)), "{err:?}");
    // Nor can a folder be made inside one.
    let err = alice.device.set_save_folder(&file.join("inside")).await.unwrap_err();
    assert!(matches!(err, Error::SaveFolder(SaveFolderProblem::CannotCreate)), "{err:?}");
    // Nothing at all is not a folder either.
    let err = alice.device.set_save_folder(std::path::Path::new("")).await.unwrap_err();
    assert!(matches!(err, Error::SaveFolder(SaveFolderProblem::NotAFolder)), "{err:?}");

    // The reasons are plain sentences that name no path.
    let said = err.to_string();
    assert!(!said.contains(tmp.path().to_str().unwrap()), "{said}");

    assert_eq!(alice.device.save_folder().await.unwrap(), before);
    alice.restart().await;
    assert_eq!(alice.device.save_folder().await.unwrap(), before);
    alice.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn a_folder_that_cannot_be_written_to_is_refused() {
    use std::os::unix::fs::PermissionsExt;

    // The superuser writes anywhere, so there is nothing to refuse.
    // SAFETY: `geteuid` has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let mut alice = TestDevice::start("alice").await;
    let tmp = tempfile::tempdir().unwrap();
    let locked = tmp.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();

    let err = alice.device.set_save_folder(&locked).await.unwrap_err();
    assert!(matches!(err, Error::SaveFolder(SaveFolderProblem::NotWritable)), "{err:?}");
    // The check left nothing behind.
    assert_eq!(std::fs::read_dir(&locked).unwrap().count(), 0);

    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    alice.device.set_save_folder(&locked).await.unwrap();
    assert_eq!(std::fs::read_dir(&locked).unwrap().count(), 0, "the check left something behind");
    alice.shutdown().await;
}

#[tokio::test]
async fn an_accept_that_names_no_folder_saves_to_the_current_setting_without_a_restart() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let tmp = tempfile::tempdir().unwrap();
    let (first, second) = (tmp.path().join("first"), tmp.path().join("second"));

    send_and_accept(&mut alice, &mut bob, "default.txt").await;
    assert_eq!(std::fs::read(bob.save_dir.join("default.txt")).unwrap(), b"some bytes");

    bob.device.set_save_folder(&first).await.unwrap();
    send_and_accept(&mut alice, &mut bob, "one.txt").await;
    assert_eq!(std::fs::read(first.join("one.txt")).unwrap(), b"some bytes");
    assert!(!bob.save_dir.join("one.txt").exists());

    // The next Offer after a change goes to the new folder: the Device was not restarted.
    bob.device.set_save_folder(&second).await.unwrap();
    send_and_accept(&mut alice, &mut bob, "two.txt").await;
    assert_eq!(std::fs::read(second.join("two.txt")).unwrap(), b"some bytes");
    assert!(!first.join("two.txt").exists());

    // And after a restart.
    bob.restart().await;
    send_and_accept(&mut alice, &mut bob, "three.txt").await;
    assert_eq!(std::fs::read(second.join("three.txt")).unwrap(), b"some bytes");

    // An Offer still pending when the folder changes goes to the folder as it is at the accept.
    let (_src, path) = write_source("pending.txt", b"waiting");
    let id = alice.device.send_file(bob.addr(), &path).await.unwrap();
    bob.wait_offer().await;
    bob.device.set_save_folder(&first).await.unwrap();
    assert!(bob.device.check_offer(id, None).await.unwrap().fits());
    bob.device.accept(id).await.unwrap();
    bob.wait_state(id, "completed").await;
    assert_eq!(std::fs::read(first.join("pending.txt")).unwrap(), b"waiting");

    // A folder named for one Offer still wins over the setting.
    let (_src, path) = write_source("named.txt", b"named");
    let id = alice.device.send_file(bob.addr(), &path).await.unwrap();
    bob.wait_offer().await;
    bob.device.accept_into(id, Some(&bob.save_dir)).await.unwrap();
    bob.wait_state(id, "completed").await;
    assert_eq!(std::fs::read(bob.save_dir.join("named.txt")).unwrap(), b"named");

    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn auto_accept_saves_to_the_current_setting_too() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let folder = tempfile::tempdir().unwrap();
    bob.device.add_contact(alice.device.device_id(), Some("Alice")).await.unwrap();
    bob.device.set_auto_accept(alice.device.device_id(), true).await.unwrap();
    bob.device.set_save_folder(folder.path()).await.unwrap();

    let (_src, path) = write_source("auto.txt", b"auto");
    let id = alice.device.send_file(bob.addr(), &path).await.unwrap();
    bob.wait_state(id, "completed").await;
    assert_eq!(std::fs::read(folder.path().join("auto.txt")).unwrap(), b"auto");
    assert!(!bob.save_dir.join("auto.txt").exists());

    alice.shutdown().await;
    bob.shutdown().await;
}
