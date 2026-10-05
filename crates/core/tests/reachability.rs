//! Reaching a Contact when no lookup service can place it (spec section 3): its last known
//! address is kept and dialled, and a Device nothing can place fails at once. These Devices have
//! no relays, DNS or DHT at all, so the stored address is the only way a bare Device ID resolves.
//! The public DHT is tested against the real one in `crates/core/src/dht.rs`, on demand.

mod support;

use std::{path::PathBuf, time::{Duration, Instant}};

use bhayanakshare_core::{DeviceAddr, DeviceId, TransferState};
use support::TestDevice;

/// Far more than a failure that is meant to be immediate takes, even on a machine under load.
const IMMEDIATELY: Duration = Duration::from_secs(10);

fn write_source(name: &str, bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
    let src = tempfile::tempdir().unwrap();
    let path = src.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    (src, path)
}

/// Alice has Bob as a Contact and has connected to him once, with his address in hand, so his
/// last known address is stored. Returns Bob's ID.
async fn alice_has_reached_bob(alice: &mut TestDevice, bob: &mut TestDevice) -> DeviceId {
    let bob_id = bob.device.device_id();
    alice.device.add_contact(bob_id, None).await.unwrap();
    let (_src, path) = write_source("first.txt", b"first");
    let first = alice.device.send_file(bob.addr(), &path).await.unwrap();
    bob.wait_offer().await;
    bob.device.decline(first).await.unwrap();
    alice.wait_state(first, "declined").await;
    bob_id
}

/// Sends a small file to `to` and waits until it is saved on Bob.
async fn send_and_receive(alice: &mut TestDevice, bob: &mut TestDevice, to: DeviceAddr) {
    let (_src, path) = write_source("hello.txt", b"hello over the last known address");
    let id = alice.device.send_file(to, &path).await.unwrap();
    bob.wait_offer().await;
    bob.device.accept(id).await.unwrap();
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;
}

#[tokio::test]
async fn a_contact_is_reached_from_its_last_known_address_alone() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let bob_id = alice_has_reached_bob(&mut alice, &mut bob).await;

    // Just the Device ID now: nothing but the stored address can say where Bob is.
    send_and_receive(&mut alice, &mut bob, DeviceAddr::from(bob_id)).await;
    assert_eq!(std::fs::read(bob.save_dir.join("hello.txt")).unwrap(), b"hello over the last known address");
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn the_last_known_address_survives_a_restart() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let bob_id = alice_has_reached_bob(&mut alice, &mut bob).await;

    alice.restart().await;
    send_and_receive(&mut alice, &mut bob, DeviceAddr::from(bob_id)).await;
    alice.shutdown().await;
    bob.shutdown().await;
}

/// Waits for Transfer `id` to fail, and returns the reason and how long that took.
async fn failure_of(alice: &mut TestDevice, id: bhayanakshare_core::TransferId, since: Instant) -> (String, Duration) {
    let failed = alice.wait_state(id, "failed").await;
    let TransferState::Failed { reason } = failed.state else { unreachable!() };
    (reason, since.elapsed())
}

#[tokio::test]
async fn a_device_nothing_can_place_fails_at_once_with_the_unreachable_message() {
    // Alice has never connected to Bob and he is not her Contact: his ID alone says nothing.
    let mut alice = TestDevice::start("alice").await;
    let bob = TestDevice::start("bob").await;
    let (_src, path) = write_source("a.txt", b"hi");

    let started = Instant::now();
    let id = alice.device.send_file(DeviceAddr::from(bob.device.device_id()), &path).await.unwrap();
    let (reason, took) = failure_of(&mut alice, id, started).await;
    assert!(took < IMMEDIATELY, "took {took:?}");
    assert!(reason.starts_with("Could not reach the receiving Device."), "{reason}");
    assert!(reason.contains("offline"), "{reason}");
    alice.shutdown().await;
}

#[tokio::test]
async fn a_removed_contacts_address_is_forgotten() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let bob_id = alice_has_reached_bob(&mut alice, &mut bob).await;
    alice.device.remove_contact(bob_id).await.unwrap();
    // A restart, so nothing is left of the connection already made (iroh remembers the paths
    // of Devices it has connected to for as long as it runs).
    alice.restart().await;

    let (_src, path) = write_source("a.txt", b"hi");
    let started = Instant::now();
    let id = alice.device.send_file(DeviceAddr::from(bob_id), &path).await.unwrap();
    let (_, took) = failure_of(&mut alice, id, started).await;
    assert!(took < IMMEDIATELY, "took {took:?}");
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn a_contact_that_went_offline_fails_the_send_with_the_unreachable_message() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let bob_id = alice_has_reached_bob(&mut alice, &mut bob).await;
    bob.shutdown().await;
    alice.restart().await;

    let (_src, path) = write_source("a.txt", b"hi");
    let started = Instant::now();
    let id = alice.device.send_file(DeviceAddr::from(bob_id), &path).await.unwrap();
    let (reason, took) = failure_of(&mut alice, id, started).await;
    eprintln!("an offline Contact failed the send after {took:?}");
    assert!(reason.starts_with("Could not reach the receiving Device."), "{reason}");
    alice.shutdown().await;
}

#[tokio::test]
async fn the_public_dht_is_on_until_switched_off_and_stays_off_after_a_restart() {
    let mut alice = TestDevice::start("alice").await;
    assert!(alice.device.public_dht());

    alice.device.set_public_dht(false).await.unwrap();
    assert!(!alice.device.public_dht());
    alice.restart().await;
    assert!(!alice.device.public_dht());

    alice.device.set_public_dht(true).await.unwrap();
    alice.restart().await;
    assert!(alice.device.public_dht());
    alice.shutdown().await;
}
