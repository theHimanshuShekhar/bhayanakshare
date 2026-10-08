//! Transfer History: every Transfer a Device has sent or received is listed newest first with
//! what it needs to find it again, narrowed by Device, direction and item name, with a Batch as
//! one entry for its Sender. Entries are kept until deleted; deleting and clearing touch only
//! Transfers that have ended, so a running one carries on and resumes as if nothing happened.

mod support;

use std::{
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use bhayanakshare_core::{
    BatchId, Error, HistoryEntry, HistoryQuery, HistoryTransfer, Role, TransferId, TransferKind,
    TransferState,
};
use support::TestDevice;
use tempfile::TempDir;

/// Writes `bytes` to a new file called `name`; keep the returned folder alive for as long as
/// the Sender needs the file.
fn source(name: &str, bytes: &[u8]) -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    (dir, path)
}

async fn history(device: &TestDevice, query: HistoryQuery) -> Vec<HistoryEntry> {
    device.device.history(&query).await.unwrap()
}

async fn everything(device: &TestDevice) -> Vec<HistoryEntry> {
    history(device, HistoryQuery::default()).await
}

/// The Transfer an entry is, which it must be.
fn one(entry: &HistoryEntry) -> &HistoryTransfer {
    match entry {
        HistoryEntry::Transfer { transfer } => transfer,
        HistoryEntry::Batch { .. } => panic!("expected a Transfer, got a Batch: {entry:?}"),
    }
}

/// The Batch an entry is, which it must be.
fn batch_of(entry: &HistoryEntry) -> (BatchId, &[HistoryTransfer]) {
    match entry {
        HistoryEntry::Batch { batch_id, transfers } => (*batch_id, transfers),
        HistoryEntry::Transfer { .. } => panic!("expected a Batch, got a Transfer: {entry:?}"),
    }
}

/// The Transfers an entry stands for, oldest first.
fn transfers_of(entry: &HistoryEntry) -> Vec<TransferId> {
    match entry {
        HistoryEntry::Transfer { transfer } => vec![transfer.record.id],
        HistoryEntry::Batch { transfers, .. } => transfers.iter().map(|t| t.record.id).collect(),
    }
}

/// The Transfers of all `entries`, in the order listed.
fn ids(entries: &[HistoryEntry]) -> Vec<TransferId> {
    entries.iter().flat_map(transfers_of).collect()
}

/// Alice sends `path` to Bob, who accepts; both see it complete.
async fn delivered(alice: &mut TestDevice, bob: &mut TestDevice, path: &Path) -> TransferId {
    let id = alice.device.send_file(bob.addr(), path).await.unwrap();
    bob.wait_offer().await;
    bob.device.accept(id).await.unwrap();
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;
    id
}

/// A sparse file of `len` bytes with a mark in it, big enough to still be running when a test
/// stops a Device part-way.
fn big_file(name: &str, len: u64) -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    let mut file = std::fs::File::create(&path).unwrap();
    file.set_len(len).unwrap();
    file.seek(SeekFrom::Start(len / 2)).unwrap();
    file.write_all(b"the middle").unwrap();
    (dir, path)
}

// ---- What it records ----------------------------------------------------------------

#[tokio::test]
async fn history_lists_every_transfer_newest_first_with_what_each_side_knows_of_it() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    alice.device.set_device_name("Alice's desktop").await.unwrap();
    bob.device.set_device_name("Bob's laptop").await.unwrap();
    assert!(everything(&alice).await.is_empty());

    // A file whose name Windows refuses, so that Bob adjusts it; Bob answers after a while.
    let (_src, path) = source("CON.txt", b"hello");
    let file = alice.device.send_file(bob.addr(), &path).await.unwrap();
    bob.wait_offer().await;
    bob.clock.advance(5_000);
    bob.device.accept(file).await.unwrap();
    bob.wait_state(file, "completed").await;
    alice.wait_state(file, "completed").await;

    // Then a text, and then Bob offers Alice a file, which she declines.
    for device in [&alice, &bob] {
        device.clock.advance(60_000);
    }
    let text = alice.device.send_text(bob.addr(), "see you at <b>noon</b>").await.unwrap();
    bob.wait_offer().await;
    bob.device.accept(text).await.unwrap();
    bob.wait_state(text, "completed").await;
    alice.wait_state(text, "completed").await;

    for device in [&alice, &bob] {
        device.clock.advance(60_000);
    }
    let back = bob.device.send_file(alice.addr(), &path).await.unwrap();
    alice.wait_offer().await;
    alice.device.decline(back).await.unwrap();
    bob.wait_state(back, "declined").await;

    // Bob: the Offer he made, then the two he received.
    let seen = everything(&bob).await;
    assert_eq!(ids(&seen), [back, text, file]);
    let [declined, got_text, got_file] = [0, 1, 2].map(|i| &one(&seen[i]).record);
    assert_eq!((declined.role, declined.peer.as_str()), (Role::Sender, alice.device.device_id().to_string().as_str()));
    assert_eq!(declined.state, TransferState::Declined);
    assert_eq!(declined.accepted_at, None);
    assert_eq!(declined.peer_name.as_deref(), Some("Alice's desktop"));

    assert_eq!((got_text.role, got_text.kind), (Role::Receiver, TransferKind::Text));
    assert_eq!(got_text.text.as_deref(), Some("see you at <b>noon</b>"));
    assert_eq!((got_text.name.as_str(), got_text.items.len(), got_text.file_count), ("", 0, 0));
    assert_eq!(got_text.size, "see you at <b>noon</b>".len() as u64);
    assert_eq!(got_text.state, TransferState::Completed { saved_to: None });
    assert_eq!(one(&seen[1]).saved_present, None, "text is not saved anywhere");

    // Everything about the file: who, when, what, how much, and where it went.
    assert_eq!((got_file.role, got_file.kind), (Role::Receiver, TransferKind::Files));
    assert_eq!(got_file.peer, alice.device.device_id().to_string());
    assert_eq!(got_file.peer_name.as_deref(), Some("Alice's desktop"));
    assert_eq!((got_file.items.as_slice(), got_file.file_count, got_file.size), (["CON.txt".to_owned()].as_slice(), 1, 5));
    assert_eq!(got_file.adjusted_names, 1);
    assert_eq!(got_file.text, None, "the contents of files are never kept");
    assert_eq!(got_file.accepted_at, Some(got_file.created_at + 5_000));
    assert!(got_file.updated_at >= got_file.accepted_at.unwrap(), "ended after it was accepted");
    assert_eq!(got_file.batch_id, None);
    let saved = bob.save_dir.join("CON_.txt");
    assert_eq!(got_file.state, TransferState::Completed { saved_to: Some(saved.to_string_lossy().into_owned()) });
    assert_eq!(one(&seen[2]).saved_present, Some(true));
    assert!(got_text.created_at > got_file.created_at && declined.created_at > got_text.created_at);

    // The files can be moved since: History then says they are not where they were put.
    std::fs::remove_file(&saved).unwrap();
    assert_eq!(one(&everything(&bob).await[2]).saved_present, Some(false));

    // Alice: the Offer she declined, then what she sent, to a Device that called itself Bob's laptop.
    let seen = everything(&alice).await;
    assert_eq!(ids(&seen), [back, text, file]);
    let [declined, sent_text, sent_file] = [0, 1, 2].map(|i| one(&seen[i]));
    assert_eq!(declined.record.role, Role::Receiver);
    assert_eq!(declined.record.state, TransferState::Declined);
    assert_eq!(declined.record.accepted_at, None);
    assert_eq!(sent_text.record.role, Role::Sender);
    assert_eq!(sent_text.record.text.as_deref(), Some("see you at <b>noon</b>"));
    assert_eq!(sent_file.record.peer_name.as_deref(), Some("Bob's laptop"));
    assert_eq!(sent_file.record.adjusted_names, 0, "a Sender adjusts nothing");
    assert_eq!(sent_file.record.state, TransferState::Completed { saved_to: None });
    assert_eq!(sent_file.saved_present, None, "a Sender has nothing saved");
    assert!(sent_file.record.accepted_at.is_some());

    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn how_a_transfer_ended_is_in_history_with_its_reason_and_who_cancelled() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let (_src, path) = source("a.txt", b"hello");

    // Cancelled by the Sender before the Receiver answered.
    let by_alice = alice.device.send_file(bob.addr(), &path).await.unwrap();
    bob.wait_offer().await;
    alice.device.cancel(by_alice).await.unwrap();
    bob.wait_state(by_alice, "cancelled").await;
    // Failed: the Receiver's Device goes away before it answers.
    let failed = alice.device.send_file(bob.addr(), &path).await.unwrap();
    bob.wait_offer().await;
    bob.shutdown().await;
    alice.wait_state(failed, "failed").await;

    let seen = everything(&alice).await;
    assert_eq!(ids(&seen), [failed, by_alice]);
    assert!(matches!(&one(&seen[0]).record.state, TransferState::Failed { reason } if !reason.is_empty()));
    assert_eq!(one(&seen[1]).record.state, TransferState::Cancelled { by: Role::Sender });
    // Bob was told of the cancelling, so it is the Sender's; the Offer that was never answered
    // lapsed when he started again.
    bob.restart().await;
    let seen = everything(&bob).await;
    assert_eq!(ids(&seen), [failed, by_alice]);
    assert_eq!(one(&seen[0]).record.state, TransferState::Expired);
    assert_eq!(one(&seen[1]).record.state, TransferState::Cancelled { by: Role::Sender });
    alice.shutdown().await;
    bob.shutdown().await;
}

// ---- Batches and Devices ------------------------------------------------------------

#[tokio::test]
async fn a_batch_is_one_entry_for_its_sender_but_each_receivers_own_transfer_for_a_device() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let mut carol = TestDevice::start("carol").await;
    let (_src, path) = source("report.txt", b"the same for everyone\n");
    let (_lone_src, lone_path) = source("lone.txt", b"just for bob");

    let batch = alice.device.send_batch(&[bob.addr(), carol.addr()], &[path]).await.unwrap();
    let [to_bob, to_carol] = batch.transfers[..] else { panic!("one Transfer per Receiver") };
    for receiver in [&mut bob, &mut carol] {
        receiver.wait_offer().await;
    }
    bob.device.accept(to_bob).await.unwrap();
    carol.device.decline(to_carol).await.unwrap();
    alice.wait_state(to_bob, "completed").await;
    alice.wait_state(to_carol, "declined").await;
    alice.clock.advance(1_000);
    let lone = delivered(&mut alice, &mut bob, &lone_path).await;

    // One entry for the Batch, in the order the Receivers were given, where the Batch began.
    let seen = everything(&alice).await;
    assert_eq!(seen.len(), 2);
    assert_eq!(one(&seen[0]).record.id, lone);
    let (id, members) = batch_of(&seen[1]);
    assert_eq!(id, batch.id);
    let states: Vec<_> = members.iter().map(|m| (m.record.id, m.record.state.label(), m.record.batch_id)).collect();
    assert_eq!(
        states,
        [(to_bob, "completed", Some(batch.id)), (to_carol, "declined", Some(batch.id))]
    );

    // Opened on one Device, it is that Receiver's Transfer, as a Contact's page shows it.
    let with_bob = history(&alice, HistoryQuery { device: Some(bob.device.device_id()), ..Default::default() }).await;
    assert_eq!(ids(&with_bob), [lone, to_bob]);
    assert!(with_bob.iter().all(|entry| matches!(entry, HistoryEntry::Transfer { .. })));
    assert_eq!(one(&with_bob[1]).record.batch_id, Some(batch.id), "still the Transfer of the Batch");
    let with_carol = history(&alice, HistoryQuery { device: Some(carol.device.device_id()), ..Default::default() }).await;
    assert_eq!(ids(&with_carol), [to_carol]);
    assert_eq!(one(&with_carol[0]).record.state, TransferState::Declined);

    // A Receiver never learns of the Batch: for Bob it is two Transfers from Alice.
    let seen = everything(&bob).await;
    assert_eq!(ids(&seen), [lone, to_bob]);
    assert!(seen.iter().all(|entry| one(entry).record.batch_id.is_none()));

    for device in [&mut alice, &mut bob, &mut carol] {
        device.shutdown().await;
    }
}

#[tokio::test]
async fn history_is_narrowed_by_device_direction_and_item_name_and_outlives_a_contact() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let mut carol = TestDevice::start("carol").await;
    let (_a, holiday) = source("Holiday Photos.zip", b"zip");
    let (_b, report) = source("Report.PDF", b"pdf");
    let (_c, notes) = source("notes.txt", b"notes");

    // To Bob a holiday and a report, from Carol the same report, and a text from Bob.
    let to_bob_1 = delivered(&mut alice, &mut bob, &holiday).await;
    let to_bob_2 = delivered(&mut alice, &mut bob, &report).await;
    let from_carol = delivered(&mut carol, &mut alice, &report).await;
    let text = bob.device.send_text(alice.addr(), "meet at the PDF stand").await.unwrap();
    alice.wait_offer().await;
    alice.device.accept(text).await.unwrap();
    alice.wait_state(text, "completed").await;
    let notes_from_bob = delivered(&mut bob, &mut alice, &notes).await;
    let [bob_id, carol_id] = [&bob, &carol].map(|d| d.device.device_id());

    let query = |device, direction, search: Option<&str>| HistoryQuery {
        device,
        direction,
        search: search.map(str::to_owned),
    };
    let found = |entries: Vec<HistoryEntry>| ids(&entries);
    // Every Transfer, newest first.
    assert_eq!(
        found(everything(&alice).await),
        [notes_from_bob, text, from_carol, to_bob_2, to_bob_1]
    );
    // By Device.
    assert_eq!(
        found(history(&alice, query(Some(bob_id), None, None)).await),
        [notes_from_bob, text, to_bob_2, to_bob_1]
    );
    assert_eq!(found(history(&alice, query(Some(carol_id), None, None)).await), [from_carol]);
    // By direction.
    assert_eq!(
        found(history(&alice, query(None, Some(Role::Sender), None)).await),
        [to_bob_2, to_bob_1]
    );
    assert_eq!(
        found(history(&alice, query(None, Some(Role::Receiver), None)).await),
        [notes_from_bob, text, from_carol]
    );
    // By item name, in any case, anywhere in the name; text has no item names.
    assert_eq!(found(history(&alice, query(None, None, Some("report"))).await), [from_carol, to_bob_2]);
    assert_eq!(found(history(&alice, query(None, None, Some("OTO"))).await), [to_bob_1]);
    assert_eq!(found(history(&alice, query(None, None, Some(".txt"))).await), [notes_from_bob]);
    assert!(history(&alice, query(None, None, Some("nothing like it"))).await.is_empty());
    // And together.
    assert_eq!(
        found(history(&alice, query(Some(bob_id), Some(Role::Sender), Some("report"))).await),
        [to_bob_2]
    );
    assert!(history(&alice, query(Some(carol_id), Some(Role::Sender), None)).await.is_empty());

    // Removing a Contact keeps History, with the Device's name as it called itself then.
    alice.device.add_contact(bob_id, Some("Bob")).await.unwrap();
    alice.device.remove_contact(bob_id).await.unwrap();
    assert_eq!(found(history(&alice, query(Some(bob_id), None, None)).await).len(), 4);

    for device in [&mut alice, &mut bob, &mut carol] {
        device.shutdown().await;
    }
}

// ---- Keeping and deleting -----------------------------------------------------------

#[tokio::test]
async fn entries_are_kept_across_restarts_until_deleted_one_at_a_time() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let (_src, path) = source("a.txt", b"hello");
    let first = delivered(&mut alice, &mut bob, &path).await;
    let second = delivered(&mut alice, &mut bob, &path).await;
    let text = alice.device.send_text(bob.addr(), "kept").await.unwrap();
    bob.wait_offer().await;
    bob.device.accept(text).await.unwrap();
    bob.wait_state(text, "completed").await;

    // Nothing expires: a long time and a restart later, it is all there, text included.
    for device in [&mut alice, &mut bob] {
        device.clock.advance(400 * 24 * 60 * 60 * 1_000);
        device.restart().await;
    }
    assert_eq!(ids(&everything(&bob).await), [text, second, first]);
    assert_eq!(one(&everything(&bob).await[0]).record.text.as_deref(), Some("kept"));

    // Deleting one removes that one and nothing else, on that Device only.
    bob.device.delete_history_transfer(second).await.unwrap();
    assert_eq!(ids(&everything(&bob).await), [text, first]);
    assert_eq!(ids(&everything(&alice).await), [text, second, first]);
    bob.restart().await;
    assert_eq!(ids(&everything(&bob).await), [text, first], "and it stays deleted");

    // A Transfer that is not there cannot be deleted, nor can one that has not ended.
    assert!(matches!(bob.device.delete_history_transfer(second).await, Err(Error::UnknownTransfer(_))));
    let waiting = alice.device.send_text(bob.addr(), "unanswered").await.unwrap();
    bob.wait_offer().await;
    assert!(matches!(alice.device.delete_history_transfer(waiting).await, Err(Error::NotFinished(_))));
    assert!(matches!(bob.device.delete_history_transfer(waiting).await, Err(Error::NotFinished(_))));
    assert_eq!(ids(&everything(&bob).await), [waiting, text, first]);
    // Which still gets its answer.
    bob.device.accept(waiting).await.unwrap();
    bob.wait_state(waiting, "completed").await;
    alice.wait_state(waiting, "completed").await;

    // Everything can be cleared; the count is how many entries went.
    assert_eq!(bob.device.clear_history().await.unwrap(), 3);
    assert!(everything(&bob).await.is_empty());
    assert_eq!(bob.device.clear_history().await.unwrap(), 0);
    assert_eq!(everything(&alice).await.len(), 4, "Alice's own History is hers");

    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn deleting_part_of_a_batch_leaves_the_rest_and_a_failed_transfer_can_still_be_retried() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let mut carol = TestDevice::start("carol").await;
    let mut dave = TestDevice::start("dave").await;
    let (_src, path) = source("report.txt", b"the same for everyone\n");
    let batch = alice.device.send_batch(&[bob.addr(), carol.addr(), dave.addr()], &[path]).await.unwrap();
    let [to_bob, to_carol, to_dave] = batch.transfers[..] else { panic!("one Transfer per Receiver") };
    for receiver in [&mut bob, &mut carol, &mut dave] {
        receiver.wait_offer().await;
    }
    // Bob declines, Dave delivers, and Carol's Device goes away before she answers.
    bob.device.decline(to_bob).await.unwrap();
    dave.device.accept(to_dave).await.unwrap();
    carol.shutdown().await;
    alice.wait_state(to_bob, "declined").await;
    alice.wait_state(to_dave, "completed").await;
    alice.wait_state(to_carol, "failed").await;

    // Deleting one Receiver's Transfer takes it out of the Batch's entry and no other.
    alice.device.delete_history_transfer(to_bob).await.unwrap();
    let seen = everything(&alice).await;
    assert_eq!(transfers_of(&seen[0]), [to_carol, to_dave]);
    assert_eq!(seen.len(), 1);

    // The Failed one can still be sent again, with what the Batch was made of.
    carol.restart().await;
    alice.device.note_address(carol.addr());
    let again = alice.device.retry(to_carol).await.unwrap();
    carol.wait_offer().await;
    carol.device.accept(again).await.unwrap();
    carol.wait_state(again, "completed").await;
    alice.wait_state(again, "completed").await;
    assert_eq!(transfers_of(&everything(&alice).await[0]), [to_carol, to_dave, again]);

    // Deleting the Batch takes what has ended with it. After that nothing is left to retry.
    assert_eq!(alice.device.delete_history_batch(batch.id).await.unwrap(), 3);
    assert!(everything(&alice).await.is_empty());
    assert!(matches!(alice.device.delete_history_batch(batch.id).await, Err(Error::UnknownBatch(_))));
    assert!(matches!(alice.device.retry(to_carol).await, Err(Error::NotRetryable(..))));
    // The Receivers' own Histories are theirs.
    assert_eq!(ids(&everything(&dave).await), [to_dave]);

    for device in [&mut alice, &mut bob, &mut carol, &mut dave] {
        device.shutdown().await;
    }
}

// ---- Clearing and what is running ---------------------------------------------------

#[tokio::test]
async fn clearing_history_leaves_a_transfer_that_is_running_to_finish_and_stay() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let (_src, small) = source("small.txt", b"done already");
    let ended = delivered(&mut alice, &mut bob, &small).await;
    // One Offer nobody has answered, and one the Receiver has accepted and is downloading.
    let (_big_src, big) = big_file("big.bin", 256 << 20);
    let unanswered = alice.device.send_text(bob.addr(), "still waiting").await.unwrap();
    bob.wait_offer().await;
    let moving = alice.device.send_file(bob.addr(), &big).await.unwrap();
    bob.wait_offer().await;
    bob.device.accept(moving).await.unwrap();
    bob.wait_progress_big(moving, 1 << 20).await;

    for device in [&alice, &bob] {
        assert_eq!(device.device.clear_history().await.unwrap(), 1, "only what ended");
        let left = ids(&everything(device).await);
        assert_eq!(left, [moving, unanswered]);
    }
    // They carry on as if nothing was cleared: the Offer is still there to answer, and the
    // download is not interrupted.
    bob.device.accept(unanswered).await.unwrap();
    bob.wait_state(unanswered, "completed").await;
    bob.wait_state_big(moving, "completed").await;
    alice.wait_state_big(moving, "completed").await;
    assert!(bob.save_dir.join("big.bin").is_file());
    for device in [&alice, &bob] {
        assert_eq!(ids(&everything(device).await), [moving, unanswered], "and stay, now that they ended");
        assert!(!ids(&everything(device).await).contains(&ended));
    }
    assert_eq!(bob.device.clear_history().await.unwrap(), 2);

    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn a_transfer_resumes_after_a_restart_though_history_was_cleared_meanwhile() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let (_src, small) = source("small.txt", b"done already");
    delivered(&mut alice, &mut bob, &small).await;
    let (_big_src, big) = big_file("big.bin", 256 << 20);
    let id = alice.device.send_file(bob.addr(), &big).await.unwrap();
    bob.wait_offer().await;
    bob.device.accept(id).await.unwrap();
    bob.wait_progress_big(id, 16 << 20).await;

    // Both sides clear, then both are restarted: each takes the Transfer up from its record.
    assert_eq!(alice.device.clear_history().await.unwrap(), 1);
    assert_eq!(bob.device.clear_history().await.unwrap(), 1);
    bob.restart().await;
    alice.restart().await;
    bob.device.note_address(alice.addr());
    alice.device.note_address(bob.addr());

    bob.wait_state_big(id, "completed").await;
    alice.wait_state_big(id, "completed").await;
    assert_eq!(std::fs::metadata(bob.save_dir.join("big.bin")).unwrap().len(), 256 << 20);
    for device in [&alice, &bob] {
        assert_eq!(ids(&everything(device).await), [id]);
        assert!(matches!(one(&everything(device).await[0]).record.state, TransferState::Completed { .. }));
    }
    alice.shutdown().await;
    bob.shutdown().await;
}
