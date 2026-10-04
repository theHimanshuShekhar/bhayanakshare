//! Walking skeleton: one file from a Sender Device to a Receiver Device, driven entirely
//! through the Device API.

mod support;

use bhayanakshare_core::{DeviceAddr, Error, Role, TransferId, TransferState};
use support::{TestDevice, list_dir, pseudo_random_bytes};

const INCOMING: &str = ".bhayanakshare-incoming";

fn write_source(dir: &std::path::Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

/// Sends `bytes` as `name` from alice to bob, bob accepts, and returns the transfer id.
async fn accepted_transfer(
    alice: &mut TestDevice,
    bob: &mut TestDevice,
    name: &str,
    bytes: &[u8],
) -> TransferId {
    let src = tempfile::tempdir().unwrap();
    let path = write_source(src.path(), name, bytes);

    let id = alice.device.send_file(bob.addr(), &path).await.unwrap();
    let offer = bob.wait_offer().await;
    assert_eq!(offer.transfer_id, id);
    assert_eq!(offer.name, name);
    assert_eq!(offer.size, bytes.len() as u64);
    assert_eq!(offer.peer, alice.device.device_id());
    bob.device.accept(id).await.unwrap();

    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;
    id
}

#[tokio::test]
async fn accepted_file_arrives_byte_identical_and_both_sides_complete() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    // Larger than iroh-blobs' 16 KiB inline limit, so it takes the on-disk path.
    let bytes = pseudo_random_bytes(3 * 1024 * 1024 + 123, 7);

    let id = accepted_transfer(&mut alice, &mut bob, "photo.bin", &bytes).await;

    assert_eq!(std::fs::read(bob.save_dir.join("photo.bin")).unwrap(), bytes);
    assert_eq!(alice.history(id), ["offered", "accepted", "completed"]);
    assert_eq!(
        bob.history(id),
        ["offered", "accepted", "transferring", "saving", "completed"]
    );

    // The Receiver's Completed event reports where the file went; the Sender's does not.
    let saved = bob.save_dir.join("photo.bin").to_string_lossy().into_owned();
    let completed = bob.log.iter().rev().find_map(|e| match &e.kind {
        bhayanakshare_core::EventKind::Transfer(t) if t.state.label() == "completed" => Some(t),
        _ => None,
    });
    assert_eq!(completed.unwrap().state, TransferState::Completed { saved_to: Some(saved) });

    // Clean shutdown waits for the incoming store to be removed.
    alice.shutdown().await;
    bob.shutdown().await;
    assert!(list_dir(&bob.save_dir.join(INCOMING)).is_empty(), "incoming store left behind");
    assert_eq!(list_dir(&bob.save_dir), [INCOMING, "photo.bin"]);
}

#[tokio::test]
async fn small_and_empty_files_arrive_intact() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;

    accepted_transfer(&mut alice, &mut bob, "note.txt", b"hello, bob\n").await;
    accepted_transfer(&mut alice, &mut bob, "empty", b"").await;

    assert_eq!(std::fs::read(bob.save_dir.join("note.txt")).unwrap(), b"hello, bob\n");
    assert_eq!(std::fs::read(bob.save_dir.join("empty")).unwrap(), b"");
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn declined_offer_leaves_nothing_in_the_save_folder() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let src = tempfile::tempdir().unwrap();
    let path = write_source(src.path(), "secret.bin", &pseudo_random_bytes(100_000, 3));

    let id = alice.device.send_file(bob.addr(), &path).await.unwrap();
    bob.wait_offer().await;
    bob.device.decline(id).await.unwrap();

    bob.wait_state(id, "declined").await;
    alice.wait_state(id, "declined").await;
    alice.shutdown().await;
    bob.shutdown().await;

    assert_eq!(list_dir(&bob.save_dir), Vec::<String>::new());
    assert_eq!(alice.history(id), ["offered", "declined"]);
    assert_eq!(bob.history(id), ["offered", "declined"]);
}

#[tokio::test]
async fn a_name_already_in_the_save_folder_is_never_overwritten() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    std::fs::write(bob.save_dir.join("report.txt"), b"mine").unwrap();

    accepted_transfer(&mut alice, &mut bob, "report.txt", b"theirs").await;

    assert_eq!(std::fs::read(bob.save_dir.join("report.txt")).unwrap(), b"mine");
    assert_eq!(std::fs::read(bob.save_dir.join("report (1).txt")).unwrap(), b"theirs");
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn one_sender_serves_two_receivers_from_its_single_store() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let mut carol = TestDevice::start("carol").await;
    let bytes = pseudo_random_bytes(512 * 1024, 11);
    let src = tempfile::tempdir().unwrap();
    let path = write_source(src.path(), "shared.bin", &bytes);

    let to_bob = alice.device.send_file(bob.addr(), &path).await.unwrap();
    let to_carol = alice.device.send_file(carol.addr(), &path).await.unwrap();
    assert_ne!(to_bob, to_carol);
    bob.wait_offer().await;
    carol.wait_offer().await;
    bob.device.accept(to_bob).await.unwrap();
    carol.device.accept(to_carol).await.unwrap();

    bob.wait_state(to_bob, "completed").await;
    carol.wait_state(to_carol, "completed").await;
    alice.wait_state(to_bob, "completed").await;
    alice.wait_state(to_carol, "completed").await;
    assert_eq!(std::fs::read(bob.save_dir.join("shared.bin")).unwrap(), bytes);
    assert_eq!(std::fs::read(carol.save_dir.join("shared.bin")).unwrap(), bytes);
    alice.shutdown().await;
    bob.shutdown().await;
    carol.shutdown().await;
}

#[tokio::test]
async fn transfer_records_are_persisted_with_clock_times() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    alice.clock.advance(5_000);
    let id = accepted_transfer(&mut alice, &mut bob, "a.txt", b"abc").await;

    let sent = alice.device.transfers().await.unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].id, id);
    assert_eq!(sent[0].role, Role::Sender);
    assert_eq!(sent[0].peer, bob.device.device_id().to_string());
    assert_eq!((sent[0].name.as_str(), sent[0].size), ("a.txt", 3));
    assert_eq!(sent[0].state, TransferState::Completed { saved_to: None });
    assert_eq!(sent[0].created_at, 1_005_000);

    let received = bob.device.transfers().await.unwrap();
    assert_eq!(received[0].role, Role::Receiver);
    let saved = bob.save_dir.join("a.txt").to_string_lossy().into_owned();
    assert_eq!(received[0].state, TransferState::Completed { saved_to: Some(saved) });
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn commands_reject_bad_input() {
    let mut alice = TestDevice::start("alice").await;
    let bob = TestDevice::start("bob").await;
    let src = tempfile::tempdir().unwrap();

    let missing = src.path().join("nope.txt");
    assert!(matches!(alice.device.send_file(bob.addr(), &missing).await, Err(Error::NotAFile(_))));
    assert!(matches!(
        alice.device.send_file(bob.addr(), src.path()).await,
        Err(Error::NotAFile(_))
    ));
    assert!(matches!(
        alice.device.accept(TransferId::random()).await,
        Err(Error::UnknownTransfer(_))
    ));
    assert!(matches!(
        alice.device.decline(TransferId::random()).await,
        Err(Error::UnknownTransfer(_))
    ));
    alice.shutdown().await;
}

#[tokio::test]
async fn an_unreachable_receiver_fails_the_transfer() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let src = tempfile::tempdir().unwrap();
    let path = write_source(src.path(), "a.txt", b"abc");
    // Bob's real Device ID, but no address and no relay or discovery to find one.
    let id = alice
        .device
        .send_file(DeviceAddr::from(bob.device.device_id()), &path)
        .await
        .unwrap();
    let failed = alice.wait_state(id, "failed").await;
    assert!(matches!(failed.state, TransferState::Failed { .. }));
    alice.shutdown().await;
    bob.shutdown().await;
}
