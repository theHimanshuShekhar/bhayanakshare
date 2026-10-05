//! Every way an Offer or a Transfer can end short of Completed: cancelled by either side at
//! each stage, expired, or refused as Busy. Expiry is driven by the Devices' injected
//! clocks; a hand-written Sender stands in where a test must hold a real Device at one
//! stage (waiting for the content, or part-way through downloading it).

mod support;

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use bhayanakshare_core::{
    Error, OFFER_TTL_MS, Role, TransferId, TransferState,
    protocol::{self, FrameError, Hello, Message, spawn_reader, write_frame},
};
use iroh::endpoint::{Connection, SendStream};
use support::{TestDevice, dial_addr, list_dir, raw_peer};
use tempfile::TempDir;
use tokio::sync::mpsc;

const INCOMING: &str = ".bhayanakshare-incoming";
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);
/// Long enough for several of the Devices' expiry checks, which run on real time.
const SEVERAL_CHECKS: Duration = Duration::from_millis(600);

/// Writes `bytes` to a new file; keep the returned folder alive for as long as the Sender
/// needs the file.
fn source(name: &str, bytes: &[u8]) -> (TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    (dir, path)
}

/// Alice offers a small file to Bob and Bob has seen the Offer.
async fn offered(alice: &mut TestDevice, bob: &mut TestDevice) -> (TransferId, TempDir) {
    let (dir, path) = source("note.txt", b"hello, bob\n");
    let id = alice.device.send_file(bob.addr(), &path).await.unwrap();
    assert_eq!(bob.wait_offer().await.transfer_id, id);
    (id, dir)
}

/// Waits for the Transfer to be Cancelled and checks by whom.
async fn assert_cancelled(device: &mut TestDevice, id: TransferId, by: Role) {
    let ended = device.wait_state(id, "cancelled").await;
    assert_eq!(ended.state, TransferState::Cancelled { by }, "{}", device.name);
}

/// Waits until `dir` is empty or gone: the incoming store is deleted by a background task.
async fn assert_becomes_empty(dir: &std::path::Path) {
    let gone = tokio::time::timeout(ANSWER_TIMEOUT, async {
        while dir.exists() && !list_dir(dir).is_empty() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(gone.is_ok(), "{} still holds {:?}", dir.display(), list_dir(dir));
}

/// A Sender written by hand, so a test decides what it says and when. It also accepts
/// iroh-blobs connections and never answers them: a Receiver that fetches from it waits
/// for content that does not come.
struct RawSender {
    id: TransferId,
    send: SendStream,
    incoming: mpsc::Receiver<Result<Message, FrameError>>,
    _conn: Connection,
    _holding: Arc<Mutex<Vec<Connection>>>,
}

impl RawSender {
    /// Dials Bob and sends an Offer for a file of `size` bytes. Bob has not answered.
    async fn offer(bob: &TestDevice, name: &str, size: u64) -> Self {
        Self::offer_with_id(bob, TransferId::random(), name, size).await
    }

    async fn offer_with_id(bob: &TestDevice, id: TransferId, name: &str, size: u64) -> Self {
        let endpoint = raw_peer().await;
        endpoint.set_alpns(vec![iroh_blobs::ALPN.to_vec()]);
        let holding = Arc::new(Mutex::new(Vec::new()));
        tokio::spawn({
            let (endpoint, holding) = (endpoint.clone(), holding.clone());
            async move {
                while let Some(incoming) = endpoint.accept().await {
                    if let Ok(conn) = incoming.await {
                        holding.lock().unwrap().push(conn);
                    }
                }
            }
        });
        let conn = endpoint.connect(dial_addr(bob), protocol::ALPN).await.unwrap();
        let (mut send, recv) = conn.open_bi().await.unwrap();
        let mut incoming = spawn_reader(recv);
        write_frame(&mut send, &Message::Hello(Hello::current())).await.unwrap();
        assert!(matches!(next(&mut incoming).await, Message::Hello(_)));
        let offer = support::one_file_offer(*id.as_bytes(), name, size);
        write_frame(&mut send, &Message::Offer(offer)).await.unwrap();
        Self { id, send, incoming, _conn: conn, _holding: holding }
    }

    async fn say(&mut self, msg: Message) {
        write_frame(&mut self.send, &msg).await.unwrap();
    }

    async fn expect(&mut self, want: Message) {
        assert_eq!(next(&mut self.incoming).await, want);
    }

    /// Reads Bob's `Accept` and gives the go-ahead to fetch. Bob then downloads from a
    /// provider that never replies.
    async fn accepted_and_ready(&mut self) {
        self.expect(Message::Accept).await;
        self.say(Message::HashReady { collection_hash: [7; 32] }).await;
    }
}

async fn next(incoming: &mut mpsc::Receiver<Result<Message, FrameError>>) -> Message {
    tokio::time::timeout(ANSWER_TIMEOUT, incoming.recv())
        .await
        .expect("timed out waiting for the other side")
        .expect("the other side closed the stream")
        .expect("a well-formed frame")
}

// ---- Cancel -------------------------------------------------------------------------

#[tokio::test]
async fn the_sender_can_cancel_an_unanswered_offer() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let (id, _src) = offered(&mut alice, &mut bob).await;

    alice.device.cancel(id).await.unwrap();

    assert_cancelled(&mut alice, id, Role::Sender).await;
    assert_cancelled(&mut bob, id, Role::Sender).await;
    // The Offer is no longer waiting for an answer.
    assert!(matches!(bob.device.accept(id).await, Err(Error::UnknownTransfer(_))));
    alice.shutdown().await;
    bob.shutdown().await;
    assert_eq!(alice.history(id), ["offered", "cancelled"]);
    assert_eq!(bob.history(id), ["offered", "cancelled"]);
    assert_eq!(list_dir(&bob.save_dir), Vec::<String>::new());
    let record = &bob.device.transfers().await.unwrap()[0];
    assert_eq!(record.state, TransferState::Cancelled { by: Role::Sender });
}

#[tokio::test]
async fn the_receiver_can_cancel_an_offer_it_has_not_answered() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let (id, _src) = offered(&mut alice, &mut bob).await;

    bob.device.cancel(id).await.unwrap();

    assert_cancelled(&mut bob, id, Role::Receiver).await;
    assert_cancelled(&mut alice, id, Role::Receiver).await;
    assert!(matches!(bob.device.accept(id).await, Err(Error::UnknownTransfer(_))));
    alice.shutdown().await;
    bob.shutdown().await;
    assert_eq!(list_dir(&bob.save_dir), Vec::<String>::new());
}

#[tokio::test]
async fn cancelling_a_transfer_that_is_not_running_is_an_error() {
    let mut alice = TestDevice::start("alice").await;
    assert!(matches!(alice.device.cancel(TransferId::random()).await, Err(Error::NotRunning(_))));
    alice.shutdown().await;
}

#[tokio::test]
async fn the_receiver_can_cancel_after_accepting_before_the_content_is_ready() {
    let mut bob = TestDevice::start("bob").await;
    let mut alice = RawSender::offer(&bob, "plans.bin", 1000).await;
    let id = alice.id;
    bob.wait_offer().await;
    bob.device.accept(id).await.unwrap();
    bob.wait_state(id, "accepted").await;
    alice.expect(Message::Accept).await;

    bob.device.cancel(id).await.unwrap();

    alice.expect(Message::Cancel).await;
    assert_cancelled(&mut bob, id, Role::Receiver).await;
    bob.shutdown().await;
    assert_eq!(bob.history(id), ["offered", "accepted", "cancelled"]);
    assert_eq!(list_dir(&bob.save_dir), Vec::<String>::new());
}

#[tokio::test]
async fn the_sender_can_cancel_while_the_receiver_waits_for_the_content() {
    let mut bob = TestDevice::start("bob").await;
    let mut alice = RawSender::offer(&bob, "plans.bin", 1000).await;
    let id = alice.id;
    bob.wait_offer().await;
    bob.device.accept(id).await.unwrap();
    alice.expect(Message::Accept).await;

    alice.say(Message::Cancel).await;

    assert_cancelled(&mut bob, id, Role::Sender).await;
    bob.shutdown().await;
    assert_eq!(list_dir(&bob.save_dir), Vec::<String>::new());
}

#[tokio::test]
async fn the_receiver_can_cancel_a_download_and_its_partial_data_is_deleted() {
    let mut bob = TestDevice::start("bob").await;
    let mut alice = RawSender::offer(&bob, "plans.bin", 1 << 20).await;
    let id = alice.id;
    bob.wait_offer().await;
    bob.device.accept(id).await.unwrap();
    alice.accepted_and_ready().await;
    bob.wait_state(id, "transferring").await;
    // The incoming store exists while the download runs.
    let incoming = bob.save_dir.join(INCOMING);
    tokio::time::timeout(ANSWER_TIMEOUT, async {
        while !incoming.join(id.to_string()).exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the incoming store is created");

    bob.device.cancel(id).await.unwrap();

    alice.expect(Message::Cancel).await;
    assert_cancelled(&mut bob, id, Role::Receiver).await;
    assert_becomes_empty(&incoming).await;
    bob.shutdown().await;
    assert_eq!(bob.history(id), ["offered", "accepted", "transferring", "cancelled"]);
    assert_eq!(list_dir(&bob.save_dir), [INCOMING]);
}

#[tokio::test]
async fn the_sender_can_cancel_a_download_and_the_receiver_deletes_its_partial_data() {
    let mut bob = TestDevice::start("bob").await;
    let mut alice = RawSender::offer(&bob, "plans.bin", 1 << 20).await;
    let id = alice.id;
    bob.wait_offer().await;
    bob.device.accept(id).await.unwrap();
    alice.accepted_and_ready().await;
    bob.wait_state(id, "transferring").await;

    alice.say(Message::Cancel).await;

    assert_cancelled(&mut bob, id, Role::Sender).await;
    assert_becomes_empty(&bob.save_dir.join(INCOMING)).await;
    bob.shutdown().await;
    assert_eq!(list_dir(&bob.save_dir), [INCOMING]);
}

#[tokio::test]
async fn the_sender_can_cancel_while_it_is_still_hashing() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    // A sparse file: nothing on disk, but hashing 2 GiB of it takes seconds, so Bob's Accept
    // arrives, and Alice is cancelled, long before the content is ready.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("huge.bin");
    std::fs::File::create(&path).unwrap().set_len(2 << 30).unwrap();
    let id = alice.device.send_file(bob.addr(), &path).await.unwrap();
    bob.wait_offer().await;
    bob.device.accept(id).await.unwrap();
    alice.wait_state(id, "accepted").await;

    alice.device.cancel(id).await.unwrap();

    assert_cancelled(&mut alice, id, Role::Sender).await;
    assert_cancelled(&mut bob, id, Role::Sender).await;
    alice.shutdown().await;
    bob.shutdown().await;
    assert_eq!(alice.history(id), ["offered", "accepted", "cancelled"]);
    assert_eq!(list_dir(&bob.save_dir), Vec::<String>::new());
}

#[tokio::test]
async fn a_transfer_cannot_be_cancelled_once_it_has_completed() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let (id, _src) = offered(&mut alice, &mut bob).await;
    bob.device.accept(id).await.unwrap();
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;

    // Give the tasks a moment to finish and forget the Transfer.
    tokio::time::timeout(ANSWER_TIMEOUT, async {
        while alice.device.cancel(id).await.is_ok() || bob.device.cancel(id).await.is_ok() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("a completed Transfer stops being cancellable");
    alice.shutdown().await;
    bob.shutdown().await;
    assert_eq!(std::fs::read(bob.save_dir.join("note.txt")).unwrap(), b"hello, bob\n");
    assert_eq!(alice.history(id), ["offered", "accepted", "transferring", "completed"]);
}

// ---- Expiry -------------------------------------------------------------------------

#[tokio::test]
async fn an_unanswered_offer_expires_on_both_sides_and_the_sender_can_send_it_again() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let (id, _src) = offered(&mut alice, &mut bob).await;
    // Both sides know when it lapses: 10 minutes after the Offer, by their own clock.
    let sent = alice.wait_state(id, "offered").await;
    assert_eq!(sent.expires_at, 1_000_000 + OFFER_TTL_MS);
    assert_eq!(OFFER_TTL_MS, 10 * 60 * 1000);

    alice.clock.advance(OFFER_TTL_MS);
    bob.clock.advance(OFFER_TTL_MS);

    assert_eq!(alice.wait_state(id, "expired").await.state, TransferState::Expired);
    assert_eq!(bob.wait_state(id, "expired").await.state, TransferState::Expired);
    assert!(matches!(bob.device.accept(id).await, Err(Error::UnknownTransfer(_))));
    assert!(matches!(bob.device.decline(id).await, Err(Error::UnknownTransfer(_))));

    // One click on the Sender: a fresh Offer of the same file to the same Device.
    let again = alice.device.resend(id).await.unwrap();
    assert_ne!(again, id);
    let offer = bob.wait_offer().await;
    assert_eq!((offer.transfer_id, offer.name.as_str()), (again, "note.txt"));
    assert_eq!(offer.expires_at, 1_000_000 + 2 * OFFER_TTL_MS, "its own 10 minutes");
    bob.device.accept(again).await.unwrap();
    bob.wait_state(again, "completed").await;
    alice.wait_state(again, "completed").await;
    assert_eq!(std::fs::read(bob.save_dir.join("note.txt")).unwrap(), b"hello, bob\n");
    // The expired one is spent.
    assert!(matches!(alice.device.resend(id).await, Err(Error::NothingToResend(_))));
    alice.shutdown().await;
    bob.shutdown().await;
    assert_eq!(alice.history(id), ["offered", "expired"]);
    assert_eq!(bob.history(id), ["offered", "expired"]);
}

#[tokio::test]
async fn whichever_side_expires_the_offer_first_the_other_shows_it_expired_too() {
    for sender_first in [true, false] {
        let mut alice = TestDevice::start("alice").await;
        let mut bob = TestDevice::start("bob").await;
        let (id, _src) = offered(&mut alice, &mut bob).await;

        // Only one Device's clock reaches the deadline; the other learns of it from the wire.
        if sender_first { &alice } else { &bob }.clock.advance(OFFER_TTL_MS);

        assert_eq!(alice.wait_state(id, "expired").await.state, TransferState::Expired);
        assert_eq!(bob.wait_state(id, "expired").await.state, TransferState::Expired);
        assert!(matches!(bob.device.accept(id).await, Err(Error::UnknownTransfer(_))));
        alice.shutdown().await;
        bob.shutdown().await;
        assert_eq!(alice.history(id), ["offered", "expired"], "sender_first={sender_first}");
        assert_eq!(bob.history(id), ["offered", "expired"], "sender_first={sender_first}");
        assert_eq!(list_dir(&bob.save_dir), Vec::<String>::new());
    }
}

#[tokio::test]
async fn an_offer_still_inside_its_ten_minutes_can_be_accepted() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let (id, _src) = offered(&mut alice, &mut bob).await;

    alice.clock.advance(OFFER_TTL_MS - 1);
    bob.clock.advance(OFFER_TTL_MS - 1);
    alice.quiet_for(SEVERAL_CHECKS).await;
    bob.quiet_for(SEVERAL_CHECKS).await;
    assert_eq!(alice.history(id), ["offered"]);
    assert_eq!(bob.history(id), ["offered"]);

    bob.device.accept(id).await.unwrap();
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn an_answered_offer_does_not_expire() {
    let mut bob = TestDevice::start("bob").await;
    let mut alice = RawSender::offer(&bob, "plans.bin", 1000).await;
    let id = alice.id;
    bob.wait_offer().await;
    bob.device.accept(id).await.unwrap();
    alice.expect(Message::Accept).await;

    bob.clock.advance(2 * OFFER_TTL_MS);
    bob.quiet_for(SEVERAL_CHECKS).await;

    // Still waiting for the content, and nothing was said to the Sender.
    assert_eq!(bob.history(id), ["offered", "accepted"]);
    let said = tokio::time::timeout(Duration::from_millis(100), alice.incoming.recv()).await;
    assert!(said.is_err(), "the Sender heard {said:?}");
    bob.shutdown().await;
}

#[tokio::test]
async fn a_receiver_that_expires_an_offer_tells_the_sender() {
    let mut bob = TestDevice::start("bob").await;
    let mut alice = RawSender::offer(&bob, "plans.bin", 1000).await;
    let id = alice.id;
    bob.wait_offer().await;

    bob.clock.advance(OFFER_TTL_MS);

    bob.wait_state(id, "expired").await;
    alice.expect(Message::Expired).await;
    bob.shutdown().await;
}

#[tokio::test]
async fn an_expired_offer_cannot_be_sent_again_once_its_file_is_gone() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let (id, src) = offered(&mut alice, &mut bob).await;
    alice.clock.advance(OFFER_TTL_MS);
    alice.wait_state(id, "expired").await;

    drop(src);

    assert!(matches!(alice.device.resend(id).await, Err(Error::NotAFile(_))));
    alice.shutdown().await;
    bob.shutdown().await;
}

// ---- Busy ---------------------------------------------------------------------------

#[tokio::test]
async fn a_sixth_waiting_offer_from_one_sender_gets_busy_and_the_sender_sees_it() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let mut carol = TestDevice::start("carol").await;
    let (_src, path) = source("note.txt", b"hello, bob\n");
    let mut waiting = Vec::new();
    for _ in 0..5 {
        waiting.push(alice.device.send_file(bob.addr(), &path).await.unwrap());
        bob.wait_offer().await;
    }

    let sixth = alice.device.send_file(bob.addr(), &path).await.unwrap();
    let busy = alice.wait_state(sixth, "failed").await;
    let TransferState::Failed { reason } = busy.state else { unreachable!() };
    assert!(reason.contains("too many Offers"), "{reason}");
    assert_eq!(alice.history(sixth), ["offered", "failed"]);

    // Bob never saw it: no prompt, no record.
    bob.quiet_for(SEVERAL_CHECKS).await;
    assert_eq!(bob.history(sixth), Vec::<&str>::new());
    assert_eq!(bob.device.transfers().await.unwrap().len(), 5);

    // The limit is per Sender.
    let (_src2, path2) = source("other.txt", b"from carol");
    let from_carol = carol.device.send_file(bob.addr(), &path2).await.unwrap();
    assert_eq!(bob.wait_offer().await.transfer_id, from_carol);

    // Answering one of Alice's makes room for another.
    bob.device.decline(waiting[0]).await.unwrap();
    alice.wait_state(waiting[0], "declined").await;
    let seventh = alice.device.send_file(bob.addr(), &path).await.unwrap();
    assert_eq!(bob.wait_offer().await.transfer_id, seventh);
    alice.shutdown().await;
    bob.shutdown().await;
    carol.shutdown().await;
}

#[tokio::test]
async fn offers_that_expired_or_were_cancelled_stop_counting_towards_busy() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let (_src, path) = source("note.txt", b"hello, bob\n");
    let mut first = Vec::new();
    for _ in 0..5 {
        first.push(alice.device.send_file(bob.addr(), &path).await.unwrap());
        bob.wait_offer().await;
    }

    // Two are cancelled, one by each side; the other three lapse.
    alice.device.cancel(first[0]).await.unwrap();
    bob.device.cancel(first[1]).await.unwrap();
    bob.wait_state(first[0], "cancelled").await;
    alice.wait_state(first[1], "cancelled").await;
    alice.clock.advance(OFFER_TTL_MS);
    for id in &first[2..] {
        bob.wait_state(*id, "expired").await;
    }

    let next = alice.device.send_file(bob.addr(), &path).await.unwrap();
    assert_eq!(bob.wait_offer().await.transfer_id, next);
    alice.shutdown().await;
    bob.shutdown().await;
}
