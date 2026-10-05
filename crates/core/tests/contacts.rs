//! Contacts through the Device API: saving, Nicknames, Auto-accept, the last known address,
//! and removal. Auto-accept is the Receiver's setting, so most tests give Bob a Contact (Alice)
//! and watch what Bob does with her Offers.

mod support;

use std::path::{Path, PathBuf};

use bhayanakshare_core::{Error, TransferState};
use support::{TestDevice, pseudo_random_bytes};

fn write_source(name: &str, bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
    let src = tempfile::tempdir().unwrap();
    let path = src.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    (src, path)
}

#[tokio::test]
async fn a_saved_device_is_a_contact_with_auto_accept_off() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    assert!(alice.device.contacts().await.unwrap().is_empty());

    let contact = alice.device.add_contact(bob.device.device_id(), Some("  Bob's laptop ")).await.unwrap();
    assert_eq!(contact.id, bob.device.device_id());
    assert_eq!(contact.device_name.as_deref(), Some("Bob's laptop"));
    assert_eq!(contact.nickname, None);
    assert!(!contact.auto_accept);
    assert_eq!(contact.display_name(), Some("Bob's laptop"));
    assert_eq!(alice.device.contacts().await.unwrap(), [contact]);

    // One-sided: Bob has not saved Alice.
    assert!(bob.device.contacts().await.unwrap().is_empty());
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn adding_twice_or_adding_yourself_is_refused() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let id = bob.device.device_id();

    alice.device.add_contact(id, None).await.unwrap();
    alice.device.set_nickname(id, Some("Bobby")).await.unwrap();
    assert!(matches!(alice.device.add_contact(id, Some("Other")).await, Err(Error::AlreadyContact(_))));
    // The refused add changed nothing.
    let kept = &alice.device.contacts().await.unwrap()[0];
    assert_eq!((kept.nickname.as_deref(), kept.device_name.as_deref()), (Some("Bobby"), None));

    assert!(matches!(
        alice.device.add_contact(alice.device.device_id(), None).await,
        Err(Error::OwnDeviceId)
    ));
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn a_nickname_beats_the_device_name_and_can_be_cleared() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let id = bob.device.device_id();
    alice.device.add_contact(id, Some("DESKTOP-7")).await.unwrap();

    let named = alice.device.set_nickname(id, Some(" Bob ")).await.unwrap();
    assert_eq!(named.nickname.as_deref(), Some("Bob"));
    assert_eq!(named.device_name.as_deref(), Some("DESKTOP-7"));
    assert_eq!(named.display_name(), Some("Bob"));

    let cleared = alice.device.set_nickname(id, Some("   ")).await.unwrap();
    assert_eq!(cleared.nickname, None);
    assert_eq!(cleared.display_name(), Some("DESKTOP-7"));

    assert!(matches!(
        alice.device.set_nickname(id, Some(&"x".repeat(65))).await,
        Err(Error::InvalidContactName(_))
    ));
    assert!(matches!(
        alice.device.set_nickname(bob.device.device_id(), Some("a\nb")).await,
        Err(Error::InvalidContactName(_))
    ));
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn changing_a_device_that_is_not_a_contact_fails() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let id = bob.device.device_id();
    assert!(matches!(alice.device.set_nickname(id, Some("Bob")).await, Err(Error::UnknownContact(_))));
    assert!(matches!(alice.device.set_auto_accept(id, true).await, Err(Error::UnknownContact(_))));
    assert!(matches!(alice.device.remove_contact(id).await, Err(Error::UnknownContact(_))));
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn without_auto_accept_a_contacts_offer_waits_for_an_answer() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    bob.device.add_contact(alice.device.device_id(), Some("Alice")).await.unwrap();
    let (_src, path) = write_source("a.txt", b"hello");

    let id = alice.device.send_file(bob.addr(), &path).await.unwrap();
    bob.wait_offer().await;
    // Nobody answered, so nothing moves.
    assert_eq!(bob.history(id), ["offered"]);
    bob.device.decline(id).await.unwrap();
    alice.wait_state(id, "declined").await;
    assert_eq!(support::list_dir(&bob.save_dir), Vec::<String>::new());
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn auto_accept_starts_the_transfer_without_a_prompt() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    bob.device.add_contact(alice.device.device_id(), None).await.unwrap();
    let on = bob.device.set_auto_accept(alice.device.device_id(), true).await.unwrap();
    assert!(on.auto_accept);
    let bytes = pseudo_random_bytes(300_000, 11);
    let (_src, path) = write_source("photo.bin", &bytes);

    let id = alice.device.send_file(bob.addr(), &path).await.unwrap();
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;
    alice.shutdown().await;
    bob.shutdown().await;

    // Bob never had an Offer to answer: the Transfer begins Accepted.
    assert_eq!(bob.history(id), ["accepted", "transferring", "saving", "completed"]);
    assert_eq!(std::fs::read(bob.save_dir.join("photo.bin")).unwrap(), bytes);
    // Alice sees the ordinary sequence.
    assert_eq!(alice.history(id), ["offered", "accepted", "transferring", "completed"]);
}

#[tokio::test]
async fn auto_accept_is_only_for_the_contact_that_has_it() {
    let mut alice = TestDevice::start("alice").await;
    let mut carol = TestDevice::start("carol").await;
    let mut bob = TestDevice::start("bob").await;
    // Bob trusts Alice and has Carol as a Contact without Auto-accept.
    bob.device.add_contact(alice.device.device_id(), None).await.unwrap();
    bob.device.set_auto_accept(alice.device.device_id(), true).await.unwrap();
    bob.device.add_contact(carol.device.device_id(), None).await.unwrap();
    let (_src, path) = write_source("c.txt", b"from carol");

    let id = carol.device.send_file(bob.addr(), &path).await.unwrap();
    bob.wait_offer().await;
    assert_eq!(bob.history(id), ["offered"]);

    // Turning Auto-accept off again for Alice makes her Offers wait too.
    bob.device.set_auto_accept(alice.device.device_id(), false).await.unwrap();
    let (_src2, path2) = write_source("a.txt", b"from alice");
    let from_alice = alice.device.send_file(bob.addr(), &path2).await.unwrap();
    bob.wait_for("Alice's Offer", |t| t.transfer_id == from_alice).await;
    assert_eq!(bob.history(from_alice), ["offered"]);

    bob.device.decline(id).await.unwrap();
    bob.device.decline(from_alice).await.unwrap();
    alice.shutdown().await;
    carol.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn a_space_warning_holds_auto_accept_back_and_the_offer_is_prompted() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start_with_free_space("bob", |_: &Path| Ok(1024)).await;
    bob.device.add_contact(alice.device.device_id(), None).await.unwrap();
    bob.device.set_auto_accept(alice.device.device_id(), true).await.unwrap();
    let (_src, path) = write_source("big.bin", &pseudo_random_bytes(4096, 12));

    let id = alice.device.send_file(bob.addr(), &path).await.unwrap();
    bob.wait_offer().await;

    // A normal prompt, with the warning the sheet shows and Accept refused.
    assert_eq!(bob.history(id), ["offered"]);
    assert!(!bob.device.check_offer(id, None).await.unwrap().fits());
    assert!(matches!(bob.device.accept(id).await, Err(Error::NotEnoughSpace { .. })));
    assert_eq!(support::list_dir(&bob.save_dir), Vec::<String>::new());

    bob.device.decline(id).await.unwrap();
    alice.wait_state(id, "declined").await;
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn a_connection_updates_the_last_known_address() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let (alice_id, bob_id) = (alice.device.device_id(), bob.device.device_id());
    alice.device.add_contact(bob_id, None).await.unwrap();
    bob.device.add_contact(alice_id, None).await.unwrap();
    assert_eq!(alice.device.contacts().await.unwrap()[0].last_known_address.direct, []);
    let (_src, path) = write_source("a.txt", b"hi");

    let first = alice.device.send_file(bob.addr(), &path).await.unwrap();
    bob.wait_offer().await;
    // Both ends noted where the other was reached, before the Offer even arrived.
    let seen_by_alice = alice.device.contacts().await.unwrap().remove(0).last_known_address;
    assert!(
        seen_by_alice.direct.iter().any(|a| bob.addr().direct.contains(a)),
        "{seen_by_alice:?} vs {:?}",
        bob.addr().direct
    );
    let seen_by_bob = bob.device.contacts().await.unwrap().remove(0).last_known_address;
    assert!(!seen_by_bob.direct.is_empty(), "{seen_by_bob:?}");
    bob.device.decline(first).await.unwrap();
    alice.wait_state(first, "declined").await;
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn a_non_contact_that_connects_is_not_recorded() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let (_src, path) = write_source("a.txt", b"hi");
    let id = alice.device.send_file(bob.addr(), &path).await.unwrap();
    bob.wait_offer().await;
    assert!(bob.device.contacts().await.unwrap().is_empty());
    assert!(alice.device.contacts().await.unwrap().is_empty());
    bob.device.decline(id).await.unwrap();
    alice.wait_state(id, "declined").await;
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn removing_a_contact_keeps_its_transfer_records() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let bob_id = bob.device.device_id();
    alice.device.add_contact(bob_id, Some("Bob")).await.unwrap();
    let (_src, path) = write_source("a.txt", b"hi");
    let id = alice.device.send_file(bob.addr(), &path).await.unwrap();
    bob.wait_offer().await;
    bob.device.accept(id).await.unwrap();
    alice.wait_state(id, "completed").await;

    alice.device.remove_contact(bob_id).await.unwrap();
    assert!(alice.device.contacts().await.unwrap().is_empty());
    let records = alice.device.transfers().await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].peer, bob_id.to_string());
    assert!(matches!(records[0].state, TransferState::Completed { .. }));

    // Once removed, Bob is just a Device ID again, and can be added anew.
    alice.device.add_contact(bob_id, None).await.unwrap();
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn a_contacts_device_name_refreshes_whenever_they_connect() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let (alice_id, bob_id) = (alice.device.device_id(), bob.device.device_id());
    alice.device.add_contact(bob_id, Some("old name")).await.unwrap();
    alice.device.set_nickname(bob_id, Some("Bobby")).await.unwrap();
    bob.device.add_contact(alice_id, None).await.unwrap();
    bob.device.set_device_name("Bob's laptop").await.unwrap();
    alice.device.set_device_name("Alice's desktop").await.unwrap();
    let (_src, path) = write_source("a.txt", b"hi");

    // Alice dials Bob: she learns his name, and he learns hers (he had none for her).
    let first = alice.device.send_file(bob.addr(), &path).await.unwrap();
    bob.wait_offer().await;
    let seen_by_alice = alice.device.contacts().await.unwrap().remove(0);
    assert_eq!(seen_by_alice.device_name.as_deref(), Some("Bob's laptop"));
    assert_eq!(seen_by_alice.nickname.as_deref(), Some("Bobby"), "a Nickname is never overwritten");
    assert_eq!(seen_by_alice.display_name(), Some("Bobby"));
    let seen_by_bob = bob.device.contacts().await.unwrap().remove(0);
    assert_eq!(seen_by_bob.device_name.as_deref(), Some("Alice's desktop"));
    bob.device.decline(first).await.unwrap();
    alice.wait_state(first, "declined").await;

    // A rename shows up at the next connection.
    bob.device.set_device_name("Bob's workstation").await.unwrap();
    let second = alice.device.send_file(bob.addr(), &path).await.unwrap();
    bob.wait_for("the second Offer", |t| t.transfer_id == second).await;
    assert_eq!(
        alice.device.contacts().await.unwrap()[0].device_name.as_deref(),
        Some("Bob's workstation")
    );
    bob.device.decline(second).await.unwrap();
    alice.wait_state(second, "declined").await;
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn an_offer_from_a_non_contact_carries_the_senders_device_name() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    alice.device.set_device_name("Alice's desktop").await.unwrap();
    bob.device.set_device_name("Bob's laptop").await.unwrap();
    let (_src, path) = write_source("a.txt", b"hi");

    let id = alice.device.send_file(bob.addr(), &path).await.unwrap();
    let offer = bob.wait_offer().await;
    assert_eq!(offer.peer, alice.device.device_id());
    assert_eq!(offer.peer_name.as_deref(), Some("Alice's desktop"));
    // Alice only learns Bob's name once she has reached him, so her first event has none.
    bob.device.accept(id).await.unwrap();
    let accepted = alice.wait_state(id, "accepted").await;
    assert_eq!(accepted.peer_name.as_deref(), Some("Bob's laptop"));
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;
    // Nobody became a Contact by connecting.
    assert!(bob.device.contacts().await.unwrap().is_empty());
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn a_sender_that_sends_no_name_or_a_hostile_one_is_shown_cleaned() {
    use bhayanakshare_core::protocol::{self, Hello, Message, read_frame, write_frame};

    let mut bob = TestDevice::start("bob").await;
    let peer = support::raw_peer().await;
    let conn = peer.connect(support::dial_addr(&bob), protocol::ALPN).await.unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    let mut hello = Hello::current();
    hello.device_name = format!(" Boss\u{0}\n{} ", "x".repeat(100));
    write_frame(&mut send, &Message::Hello(hello)).await.unwrap();
    assert!(matches!(read_frame(&mut recv).await.unwrap(), Message::Hello(_)));
    let id = bhayanakshare_core::TransferId::from_bytes([3; 16]);
    write_frame(
        &mut send,
        &Message::Offer(support::one_file_offer(*id.as_bytes(), "a.txt", 1)),
    )
    .await
    .unwrap();

    let offer = bob.wait_offer().await;
    let name = offer.peer_name.unwrap();
    assert!(name.starts_with("Boss") && !name.chars().any(char::is_control), "{name:?}");
    assert_eq!(name.chars().count(), 64);
    bob.shutdown().await;
}
