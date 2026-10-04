//! The Receiver reports how much of an accepted file has arrived, so the UI can show a
//! progress bar and a rate. Driven through the Device API like every other integration test.

mod support;

use bhayanakshare_core::{EventKind, ProgressEvent, TransferId};
use support::{TestDevice, pseudo_random_bytes};

#[tokio::test]
async fn the_receiver_reports_progress_between_transferring_and_saving() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let bytes = pseudo_random_bytes(3 * 1024 * 1024 + 123, 5);
    let src = tempfile::tempdir().unwrap();
    let path = src.path().join("big.bin");
    std::fs::write(&path, &bytes).unwrap();

    let id = alice.device.send_file(bob.addr(), &path).await.unwrap();
    bob.wait_offer().await;
    bob.device.accept(id).await.unwrap();
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;
    alice.shutdown().await;
    bob.shutdown().await;

    let total = bytes.len() as u64;
    let received = progress_between(&bob, id, "transferring", "saving");
    assert!(!received.is_empty(), "no progress events");
    assert!(received.iter().all(|p| p.total == total));
    let done: Vec<u64> = received.iter().map(|p| p.bytes).collect();
    assert!(done.windows(2).all(|w| w[0] <= w[1]), "progress went backwards: {done:?}");
    assert_eq!(*done.last().unwrap(), total, "the last report is the full size");

    // The Receiver tells the Sender, which reports the same progress while it waits for
    // `Completed`.
    let sent = progress_between(&alice, id, "accepted", "completed");
    assert_eq!(sent, received);
}

#[tokio::test]
async fn progress_is_throttled_by_the_injected_clock() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let src = tempfile::tempdir().unwrap();
    let path = src.path().join("big.bin");
    std::fs::write(&path, pseudo_random_bytes(8 * 1024 * 1024, 9)).unwrap();

    let id = alice.device.send_file(bob.addr(), &path).await.unwrap();
    bob.wait_offer().await;
    bob.device.accept(id).await.unwrap();
    bob.wait_state(id, "completed").await;
    alice.shutdown().await;
    bob.shutdown().await;

    // The test clock never moves, so only the first report and the final one get through.
    let count = bob.log.iter().filter(|e| matches!(e.kind, EventKind::Progress(_))).count();
    assert!((1..=2).contains(&count), "{count} progress events with a stopped clock");
}

/// The Transfer's progress events on `device` that lie between its `from` and `to` states.
fn progress_between(device: &TestDevice, id: TransferId, from: &str, to: &str) -> Vec<ProgressEvent> {
    let state = |label: &str| {
        device
            .log
            .iter()
            .position(|e| matches!(&e.kind, EventKind::Transfer(t) if t.transfer_id == id && t.state.label() == label))
            .unwrap_or_else(|| panic!("{}: never reached {label}", device.name))
    };
    let (from, to) = (state(from), state(to));
    let all: Vec<(usize, &ProgressEvent)> = device
        .log
        .iter()
        .enumerate()
        .filter_map(|(i, e)| match &e.kind {
            EventKind::Progress(p) if p.transfer_id == id => Some((i, p)),
            _ => None,
        })
        .collect();
    assert!(
        all.iter().all(|(i, _)| from < *i && *i < to),
        "{}: progress outside {from}..{to}: {all:?}",
        device.name
    );
    all.into_iter().map(|(_, p)| p.clone()).collect()
}
