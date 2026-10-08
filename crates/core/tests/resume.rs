//! Interrupted Transfers carry on by themselves: after either Device restarts cleanly, and
//! until nobody has made progress for 24 hours. A Device that stops part-way is shut down and
//! started again on the same folders (`TestDevice::restart`); a Device killed outright is in
//! `tests/crash.rs`. The Receiver is told where to find a restarted Sender with
//! `Device::note_address`, as discovery would on a real network.

mod support;

use std::{
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use bhayanakshare_core::{EventKind, Role, STALL_TTL_MS, TransferId, TransferState};
use support::{TestDevice, list_dir};
use tempfile::TempDir;

const INCOMING: &str = ".bhayanakshare-incoming";
/// Big enough that a Transfer is still running when a test stops a Device part-way.
const BIG: u64 = 256 << 20;
/// How much a Receiver has when the test stops a Device.
const PART: u64 = 16 << 20;
/// Long enough for several of the Devices' checks of their clocks, which run on real time.
const SEVERAL_CHECKS: Duration = Duration::from_millis(600);

/// A sparse file of `len` bytes with a few marks in it, so that a copy that lost or
/// misplaced a part of it differs from it. Returns the folder to keep alive and the path.
fn big_file(name: &str, len: u64) -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    write_big(&path, len);
    (dir, path)
}

fn write_big(path: &Path, len: u64) {
    let mut file = std::fs::File::create(path).unwrap();
    file.set_len(len).unwrap();
    for (i, at) in [0, len / 7, len / 3, len / 2, len - 100].into_iter().enumerate() {
        file.seek(SeekFrom::Start(at)).unwrap();
        file.write_all(format!("mark {i} at {at}").as_bytes()).unwrap();
    }
}

/// `album/`: the big file, a small one in a folder with a time of its own, an empty folder
/// and an executable, so that what the manifest carries can be checked after a Transfer
/// that was interrupted. Returns the folder to keep alive and the path of `album`.
fn big_album(len: u64) -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let album = dir.path().join("album");
    std::fs::create_dir_all(album.join("notes")).unwrap();
    std::fs::create_dir_all(album.join("empty")).unwrap();
    write_big(&album.join("movie.bin"), len);
    std::fs::write(album.join("notes/a.txt"), b"alpha").unwrap();
    std::fs::write(album.join("run.sh"), b"#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(album.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let at = std::time::UNIX_EPOCH + Duration::new(1_600_000_000, 123_456_700);
    let notes = std::fs::OpenOptions::new().write(true).open(album.join("notes/a.txt")).unwrap();
    notes.set_modified(at).unwrap();
    (dir, album)
}

/// Checks that what `big_album` made arrived whole in `got`, with what the manifest carries.
fn assert_album_arrived(sent: &Path, got: &Path) {
    assert!(same_content(&sent.join("movie.bin"), &got.join("movie.bin")));
    assert_eq!(std::fs::read(got.join("notes/a.txt")).unwrap(), b"alpha");
    let at = std::time::UNIX_EPOCH + Duration::new(1_600_000_000, 123_456_700);
    assert_eq!(std::fs::metadata(got.join("notes/a.txt")).unwrap().modified().unwrap(), at);
    assert!(got.join("empty").is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(std::fs::metadata(got.join("run.sh")).unwrap().permissions().mode() & 0o111, 0);
    }
}

fn same_content(a: &Path, b: &Path) -> bool {
    let (mut a, mut b) = (std::fs::File::open(a).unwrap(), std::fs::File::open(b).unwrap());
    if a.metadata().unwrap().len() != b.metadata().unwrap().len() {
        return false;
    }
    let (mut left, mut right) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    loop {
        let n = a.read(&mut left).unwrap();
        if n == 0 {
            return true;
        }
        b.read_exact(&mut right[..n]).unwrap();
        if left[..n] != right[..n] {
            return false;
        }
    }
}

/// Alice offers `path` to Bob, Bob accepts, and has received at least `PART` bytes of it.
/// Returns the Transfer and how many bytes Bob has reported so far.
async fn part_way(alice: &mut TestDevice, bob: &mut TestDevice, path: &Path) -> (TransferId, u64) {
    let id = alice.device.send_file(bob.addr(), path).await.unwrap();
    bob.wait_offer().await;
    bob.device.accept(id).await.unwrap();
    bob.wait_progress(id, PART).await;
    let received = bob.progress(id).last().unwrap().bytes;
    (id, received)
}

/// The Transfer's progress reports on `device` from `from` events on.
fn progress_after(device: &TestDevice, from: usize, id: TransferId) -> Vec<u64> {
    device.log[from..]
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::Progress(p) if p.transfer_id == id => Some(p.bytes),
            _ => None,
        })
        .collect()
}

/// What Bob reports after he is back: the first report is how much he already had, which
/// is at least what he had reported before, and not everything.
fn assert_picked_up(resumed: &[u64], before: u64, total: u64) {
    let first = *resumed.first().expect("a report when the fetch begins again");
    assert!(first >= before, "started again from {first}, had {before}");
    assert!(first < total, "had everything already: {resumed:?}");
    assert_eq!(*resumed.last().unwrap(), total, "and finished");
}

async fn assert_incoming_empty(device: &TestDevice) {
    let dir = device.save_dir.join(INCOMING);
    tokio::time::timeout(Duration::from_secs(10), async {
        while dir.exists() && !list_dir(&dir).is_empty() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{} still holds {:?}", dir.display(), list_dir(&dir)));
}

// ---- Clean restarts -----------------------------------------------------------------

#[tokio::test]
async fn the_receiver_restarts_cleanly_and_the_transfer_carries_on() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let (_src, path) = big_file("movie.bin", BIG);
    let (id, before) = part_way(&mut alice, &mut bob, &path).await;

    bob.restart().await;
    bob.device.note_address(alice.addr());
    let seen = bob.log.len();

    // The restarted Receiver shows Reconnecting, then fetches the rest.
    bob.wait_state(id, "reconnecting").await;
    bob.wait_state(id, "transferring").await;
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;
    assert!(same_content(&path, &bob.save_dir.join("movie.bin")));
    assert_picked_up(&progress_after(&bob, seen, id), before, BIG);
    alice.shutdown().await;
    bob.shutdown().await;
    assert_incoming_empty(&bob).await;
    assert_eq!(alice.history(id), ["offered", "accepted", "transferring", "completed"]);
}

#[tokio::test]
async fn the_sender_restarts_cleanly_and_the_transfer_carries_on() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let (_src, path) = big_file("movie.bin", BIG);
    let (id, before) = part_way(&mut alice, &mut bob, &path).await;

    alice.restart().await;
    bob.device.note_address(alice.addr());
    let seen = bob.log.len();

    // Bob lost Alice mid-fetch, redials, and Alice, from her database, takes him back.
    bob.wait_state(id, "reconnecting").await;
    bob.wait_state(id, "transferring").await;
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;
    assert!(same_content(&path, &bob.save_dir.join("movie.bin")));
    assert_picked_up(&progress_after(&bob, seen, id), before, BIG);
    alice.shutdown().await;
    bob.shutdown().await;
    assert_incoming_empty(&bob).await;
    // The restarted Alice announces the Transfer again once Bob is back.
    assert_eq!(alice.history(id), ["offered", "accepted", "transferring", "transferring", "completed"]);
    let record = &alice.device.transfers().await.unwrap()[0];
    assert!(matches!(record.state, TransferState::Completed { .. }));
}

#[tokio::test]
async fn a_folder_carries_on_after_the_receiver_restarts_and_is_built_from_the_saved_manifest() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let (_src, album) = big_album(BIG);
    let (id, before) = part_way(&mut alice, &mut bob, &album).await;

    bob.restart().await;
    bob.device.note_address(alice.addr());
    let seen = bob.log.len();

    bob.wait_state(id, "reconnecting").await;
    bob.wait_state(id, "transferring").await;
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;
    assert_album_arrived(&album, &bob.save_dir.join("album"));
    assert_picked_up(&progress_after(&bob, seen, id), before, BIG + 5 + 10);
    // The events of a resumed Transfer still say what it holds.
    let record = &bob.device.transfers().await.unwrap()[0];
    assert_eq!((record.items.as_slice(), record.file_count), (["album".to_owned()].as_slice(), 3));
    let resumed = bob.log[seen..].iter().find_map(|e| match &e.kind {
        EventKind::Transfer(t) if t.state.label() == "reconnecting" => Some(t.clone()),
        _ => None,
    });
    assert_eq!(resumed.unwrap().items, ["album"]);
    alice.shutdown().await;
    bob.shutdown().await;
    assert_incoming_empty(&bob).await;
}

#[tokio::test]
async fn a_folder_carries_on_after_the_sender_restarts() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let (_src, album) = big_album(BIG);
    let (id, _) = part_way(&mut alice, &mut bob, &album).await;

    alice.restart().await;
    bob.device.note_address(alice.addr());

    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;
    assert_album_arrived(&album, &bob.save_dir.join("album"));
    alice.shutdown().await;
    bob.shutdown().await;
    assert_incoming_empty(&bob).await;
}

#[tokio::test]
async fn shutdown_does_not_wait_past_its_deadline() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let (_src, path) = big_file("movie.bin", BIG);
    part_way(&mut alice, &mut bob, &path).await;

    // With no time to spare it returns at once, leaving Bob's store to close in the
    // background; a Device that would not be left behind like this is `restart`ed instead.
    let asked = std::time::Instant::now();
    bob.device.shutdown(Duration::ZERO).await;
    assert!(asked.elapsed() < Duration::from_secs(2), "waited {:?}", asked.elapsed());
    alice.shutdown().await;
}

// ---- Giving up ----------------------------------------------------------------------

#[tokio::test]
async fn a_transfer_with_no_progress_for_24_hours_expires_on_both_sides() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let (_src, path) = big_file("movie.bin", BIG);
    let (id, _) = part_way(&mut alice, &mut bob, &path).await;
    // Alice restarts and Bob is not told where she is, so he cannot reach her.
    alice.restart().await;
    bob.wait_state(id, "reconnecting").await;

    // Almost a day later nobody has given up yet.
    let day = STALL_TTL_MS;
    alice.clock.advance(day - day / 24);
    bob.clock.advance(day - day / 24);
    alice.quiet_for(SEVERAL_CHECKS).await;
    bob.quiet_for(SEVERAL_CHECKS).await;
    assert_eq!(bob.history(id).last(), Some(&"reconnecting"));
    assert!(matches!(alice.device.transfers().await.unwrap()[0].state, TransferState::Transferring));

    // Past 24 hours without progress, both are done with it.
    alice.clock.advance(day / 12);
    bob.clock.advance(day / 12);
    let failed = bob.wait_state(id, "failed").await;
    let TransferState::Failed { reason } = failed.state else { unreachable!() };
    assert!(reason.contains("24 hours"), "{reason}");
    let failed = alice.wait_state(id, "failed").await;
    let TransferState::Failed { reason } = failed.state else { unreachable!() };
    assert!(reason.contains("24 hours"), "{reason}");

    // Bob's partial data is gone and nothing reached his save folder.
    assert_incoming_empty(&bob).await;
    assert!(!bob.save_dir.join("movie.bin").exists());
    // Neither will take the Transfer up again after another restart.
    alice.restart().await;
    bob.restart().await;
    bob.quiet_for(SEVERAL_CHECKS).await;
    assert!(matches!(bob.device.transfers().await.unwrap()[0].state, TransferState::Failed { .. }));
    assert!(matches!(alice.device.transfers().await.unwrap()[0].state, TransferState::Failed { .. }));
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn the_24_hours_count_from_the_last_progress_not_from_the_restart() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let (_src, path) = big_file("movie.bin", BIG);
    let (id, _) = part_way(&mut alice, &mut bob, &path).await;
    alice.restart().await;
    bob.wait_state(id, "reconnecting").await;

    // Twenty hours go by, then Bob restarts as well: that is not progress.
    bob.clock.advance(STALL_TTL_MS - 4 * 3_600_000);
    bob.restart().await;
    bob.wait_state(id, "reconnecting").await;
    bob.clock.advance(5 * 3_600_000);

    let failed = bob.wait_state(id, "failed").await;
    assert!(matches!(failed.state, TransferState::Failed { .. }));
    alice.shutdown().await;
    bob.shutdown().await;
}

// ---- A Transfer the Sender can no longer serve ---------------------------------------

#[tokio::test]
async fn a_source_file_that_changed_while_the_receiver_was_away_fails_the_transfer() {
    for delete in [false, true] {
        let mut alice = TestDevice::start("alice").await;
        let mut bob = TestDevice::start("bob").await;
        let (_src, path) = big_file("movie.bin", BIG);
        let (id, _) = part_way(&mut alice, &mut bob, &path).await;
        bob.shutdown().await;

        if delete {
            std::fs::remove_file(&path).unwrap();
        } else {
            let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            file.write_all(b"one more byte").unwrap();
        }
        bob.restart().await;
        bob.device.note_address(alice.addr());

        let want = "A file changed on the sending Device: movie.bin";
        for device in [&mut bob, &mut alice] {
            let failed = device.wait_state(id, "failed").await;
            assert_eq!(failed.state, TransferState::Failed { reason: want.into() }, "{}", device.name);
        }
        assert_incoming_empty(&bob).await;
        assert!(!bob.save_dir.join("movie.bin").exists());
        alice.shutdown().await;
        bob.shutdown().await;
    }
}

#[tokio::test]
async fn a_file_in_a_folder_that_changed_while_the_receiver_was_away_fails_the_transfer() {
    for delete in [false, true] {
        let mut alice = TestDevice::start("alice").await;
        let mut bob = TestDevice::start("bob").await;
        let (_src, album) = big_album(BIG);
        let (id, _) = part_way(&mut alice, &mut bob, &album).await;
        bob.shutdown().await;

        // Not the big file that is being served, but one of the small ones around it.
        let small = album.join("notes/a.txt");
        if delete {
            std::fs::remove_file(&small).unwrap();
        } else {
            let mut file = std::fs::OpenOptions::new().append(true).open(&small).unwrap();
            file.write_all(b"more").unwrap();
        }
        bob.restart().await;
        bob.device.note_address(alice.addr());

        let want = "A file changed on the sending Device: album/notes/a.txt";
        for device in [&mut bob, &mut alice] {
            let failed = device.wait_state(id, "failed").await;
            assert_eq!(failed.state, TransferState::Failed { reason: want.into() }, "{}", device.name);
        }
        assert_incoming_empty(&bob).await;
        assert!(!bob.save_dir.join("album").exists());
        alice.shutdown().await;
        bob.shutdown().await;
    }
}

#[tokio::test]
async fn a_sender_that_cancelled_while_the_receiver_was_away_tells_it_when_it_returns() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let (_src, path) = big_file("movie.bin", BIG);
    let (id, _) = part_way(&mut alice, &mut bob, &path).await;
    bob.shutdown().await;

    alice.device.cancel(id).await.unwrap();
    alice.wait_state(id, "cancelled").await;
    bob.restart().await;
    bob.device.note_address(alice.addr());

    let cancelled = bob.wait_state(id, "cancelled").await;
    assert_eq!(cancelled.state, TransferState::Cancelled { by: Role::Sender });
    assert_incoming_empty(&bob).await;
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn a_sender_that_forgot_the_transfer_says_so_and_the_receiver_gives_up() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let (_src, path) = big_file("movie.bin", BIG);
    let (id, _) = part_way(&mut alice, &mut bob, &path).await;
    bob.shutdown().await;
    alice.shutdown().await;
    // Alice comes back with an empty data folder: same key, but no memory of the Transfer. The
    // folder is a new one rather than the old one emptied, as Windows will not delete files
    // that the stopped Device still has open.
    let key = std::fs::read(alice.data_dir.join("secret.key")).unwrap();
    let fresh = tempfile::tempdir().unwrap();
    alice.data_dir = fresh.path().join("data");
    std::fs::create_dir_all(&alice.data_dir).unwrap();
    std::fs::write(alice.data_dir.join("secret.key"), key).unwrap();
    alice.restart().await;
    bob.restart().await;
    bob.device.note_address(alice.addr());

    let failed = bob.wait_state(id, "failed").await;
    let TransferState::Failed { reason } = failed.state else { unreachable!() };
    assert!(reason.contains("no longer has this Transfer"), "{reason}");
    assert_incoming_empty(&bob).await;
    alice.shutdown().await;
    bob.shutdown().await;
}

// ---- The user stops it ----------------------------------------------------------------

#[tokio::test]
async fn the_receiver_can_cancel_while_it_is_reconnecting_and_its_partial_data_is_deleted() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let (_src, path) = big_file("movie.bin", BIG);
    let (id, _) = part_way(&mut alice, &mut bob, &path).await;
    alice.restart().await;
    bob.wait_state(id, "reconnecting").await;

    bob.device.cancel(id).await.unwrap();

    let cancelled = bob.wait_state(id, "cancelled").await;
    assert_eq!(cancelled.state, TransferState::Cancelled { by: Role::Receiver });
    assert_incoming_empty(&bob).await;
    assert_eq!(list_dir(&bob.save_dir), [INCOMING]);
    alice.shutdown().await;
    bob.shutdown().await;
}

// ---- Offers ---------------------------------------------------------------------------

#[tokio::test]
async fn a_pending_offer_does_not_survive_a_sender_restart() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let (_src, path) = big_file("note.bin", 1 << 20);
    let id = alice.device.send_file(bob.addr(), &path).await.unwrap();
    bob.wait_offer().await;

    alice.restart().await;

    // Alice's record says Expired, and nothing is waiting for Bob's answer to it any more.
    let records = alice.device.transfers().await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!((records[0].id, &records[0].state), (id, &TransferState::Expired));
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn a_transfer_that_cannot_be_resumed_fails_instead_of_staying_open_forever() {
    let mut alice = TestDevice::start("alice").await;
    // Bob is told there is room for the sparse file below, which he never actually receives.
    let mut bob = TestDevice::start_with_free_space("bob", |_: &Path| Ok(u64::MAX)).await;
    // A file this big takes seconds to hash, so Alice restarts while still hashing.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("huge.bin");
    std::fs::File::create(&path).unwrap().set_len(8 << 30).unwrap();
    let id = alice.device.send_file(bob.addr(), &path).await.unwrap();
    bob.wait_offer().await;
    bob.device.accept(id).await.unwrap();
    alice.wait_state(id, "accepted").await;

    alice.restart().await;

    let records = alice.device.transfers().await.unwrap();
    assert!(
        matches!(&records[0].state, TransferState::Failed { reason } if reason.contains("restarted")),
        "{:?}",
        records[0].state
    );
    alice.shutdown().await;
    bob.shutdown().await;
}
