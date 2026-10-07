//! Carrying a Device ID to another install: an identity export is made under a password and
//! imported elsewhere, where the Device has the same Device ID after it restarts. Only the
//! secret key travels; Contacts and History stay where they are. These tests use a key file
//! (`KeySource::File`); where else a key can live is covered by the unit tests of the key store.

mod support;

use bhayanakshare_core::{Device, Error, HistoryQuery, IdentityFileError};
use support::TestDevice;

const PASSWORD: &str = "correct horse battery";

#[tokio::test]
async fn an_exported_identity_is_the_same_device_id_when_imported_and_restarted() {
    let mut old = TestDevice::start("old install").await;
    let id = old.device.device_id();
    let file = old.device.export_identity(PASSWORD).await.unwrap();
    old.shutdown().await;

    let mut fresh = TestDevice::start("fresh install").await;
    let before = fresh.device.device_id();
    assert_ne!(before, id);

    assert_eq!(fresh.device.import_identity(&file, PASSWORD).await.unwrap(), id);
    // The running endpoint is bound to the old key until the app restarts.
    assert_eq!(fresh.device.device_id(), before);
    fresh.restart().await;
    assert_eq!(fresh.device.device_id(), id);
    // And it stays: the key was stored, not just used once.
    fresh.restart().await;
    assert_eq!(fresh.device.device_id(), id);
    fresh.shutdown().await;
}

#[tokio::test]
async fn a_wrong_password_fails_and_the_device_id_is_unchanged() {
    let mut old = TestDevice::start("old install").await;
    let file = old.device.export_identity(PASSWORD).await.unwrap();
    old.shutdown().await;

    let mut fresh = TestDevice::start("fresh install").await;
    let id = fresh.device.device_id();
    let key_file = std::fs::read(fresh.data_dir.join("secret.key")).unwrap();

    let err = fresh.device.import_identity(&file, "not the password").await.unwrap_err();
    assert!(matches!(err, Error::IdentityFile(IdentityFileError::WrongPassword)), "{err:?}");
    let err = Device::identity_file_owner(&file, "not the password").await.unwrap_err();
    assert!(matches!(err, Error::IdentityFile(IdentityFileError::WrongPassword)), "{err:?}");

    // Nothing was written, and the next start is the same Device.
    assert_eq!(std::fs::read(fresh.data_dir.join("secret.key")).unwrap(), key_file);
    fresh.restart().await;
    assert_eq!(fresh.device.device_id(), id);
    fresh.shutdown().await;
}

#[tokio::test]
async fn a_file_that_is_not_an_identity_export_fails_and_the_device_id_is_unchanged() {
    let mut fresh = TestDevice::start("fresh install").await;
    let id = fresh.device.device_id();

    for junk in [&b""[..], b"just some text", &[0u8; 93]] {
        let err = fresh.device.import_identity(junk, PASSWORD).await.unwrap_err();
        assert!(matches!(err, Error::IdentityFile(IdentityFileError::NotAnIdentityFile)), "{err:?}");
    }
    let mut export = fresh.device.export_identity(PASSWORD).await.unwrap();
    let last = export.len() - 1;
    export[last] ^= 1;
    let err = fresh.device.import_identity(&export, PASSWORD).await.unwrap_err();
    assert!(matches!(err, Error::IdentityFile(IdentityFileError::WrongPassword)), "{err:?}");

    fresh.restart().await;
    assert_eq!(fresh.device.device_id(), id);
    fresh.shutdown().await;
}

#[tokio::test]
async fn an_export_needs_a_password() {
    let mut device = TestDevice::start("device").await;
    let err = device.device.export_identity("").await.unwrap_err();
    assert!(matches!(err, Error::IdentityFile(IdentityFileError::EmptyPassword)), "{err:?}");
    let file = device.device.export_identity(PASSWORD).await.unwrap();
    let err = device.device.import_identity(&file, "").await.unwrap_err();
    assert!(matches!(err, Error::IdentityFile(IdentityFileError::EmptyPassword)), "{err:?}");
    device.shutdown().await;
}

#[tokio::test]
async fn importing_a_devices_own_identity_changes_nothing() {
    let mut device = TestDevice::start("device").await;
    let id = device.device.device_id();
    let key_file = std::fs::read(device.data_dir.join("secret.key")).unwrap();
    let file = device.device.export_identity(PASSWORD).await.unwrap();

    assert_eq!(device.device.import_identity(&file, PASSWORD).await.unwrap(), id);
    assert_eq!(std::fs::read(device.data_dir.join("secret.key")).unwrap(), key_file);
    device.restart().await;
    assert_eq!(device.device.device_id(), id);
    device.shutdown().await;
}

#[tokio::test]
async fn an_identity_file_can_be_checked_without_importing_it() {
    let mut old = TestDevice::start("old install").await;
    let id = old.device.device_id();
    let file = old.device.export_identity(PASSWORD).await.unwrap();
    old.shutdown().await;

    let mut fresh = TestDevice::start("fresh install").await;
    let before = fresh.device.device_id();
    assert_eq!(Device::identity_file_owner(&file, PASSWORD).await.unwrap(), id);
    fresh.restart().await;
    assert_eq!(fresh.device.device_id(), before, "checking a file replaced the identity");
    fresh.shutdown().await;
}

#[tokio::test]
async fn contacts_and_history_are_kept_by_an_import_and_are_not_in_the_export() {
    let mut old = TestDevice::start("old install").await;
    let id = old.device.device_id();
    let bare = old.device.export_identity(PASSWORD).await.unwrap();

    // A Device with a Contact and a Transfer in its History, as a person who has used the app.
    let mut fresh = TestDevice::start("fresh install").await;
    let mut bob = TestDevice::start("bob").await;
    fresh.device.add_contact(bob.device.device_id(), Some("Bob's laptop")).await.unwrap();
    let src = tempfile::tempdir().unwrap();
    let path = src.path().join("hello.txt");
    std::fs::write(&path, b"hello").unwrap();
    let transfer = bob.device.send_file(fresh.addr(), &path).await.unwrap();
    fresh.wait_offer().await;
    fresh.device.accept(transfer).await.unwrap();
    fresh.wait_state(transfer, "completed").await;
    bob.wait_state(transfer, "completed").await;
    let contacts = fresh.device.contacts().await.unwrap();
    let history = fresh.device.history(&HistoryQuery::default()).await.unwrap();
    assert_eq!((contacts.len(), history.len()), (1, 1));

    // Its export has no more in it than an unused Device's.
    let file = fresh.device.export_identity(PASSWORD).await.unwrap();
    assert_eq!(file.len(), bare.len());
    assert!(!file.windows(5).any(|w| w == b"hello"));

    old.shutdown().await;
    assert_ne!(fresh.device.device_id(), id);
    // Import the old install's identity into the used one.
    assert_eq!(fresh.device.import_identity(&bare, PASSWORD).await.unwrap(), id);
    fresh.restart().await;
    assert_eq!(fresh.device.device_id(), id);
    assert_eq!(fresh.device.contacts().await.unwrap(), contacts);
    assert_eq!(fresh.device.history(&HistoryQuery::default()).await.unwrap(), history);
    fresh.shutdown().await;
    bob.shutdown().await;
}
