//! Batches: one send to several Receivers. Each Receiver sees an ordinary Transfer and each
//! Transfer ends on its own; the Sender sees them as a group (one Batch ID, one hashing, at
//! most 3 downloading at a time) and can cancel one or all, or retry a Failed one. Where a
//! test must hold a Receiver at one stage (downloading, and never finishing) a hand-written
//! Receiver stands in for a real Device.

mod support;

use std::{collections::HashSet, time::Duration};

use bhayanakshare_core::{
    BatchId, DeviceAddr, DeviceId, Error, EventKind, Role, TransferId, TransferState,
    protocol::{self, FrameError, Hello, Message, Offer, spawn_reader, write_frame},
};
use iroh::{
    Endpoint,
    endpoint::{Connection, SendStream},
};
use support::{TestDevice, dial_addr, pseudo_random_bytes, raw_peer};
use tempfile::TempDir;
use tokio::sync::mpsc;

const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);
/// Long enough that something that was going to happen has happened.
const QUIET: Duration = Duration::from_millis(400);

/// Writes `bytes` to a new file; keep the returned folder alive for as long as the Sender
/// needs the file.
fn source(name: &str, bytes: &[u8]) -> (TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    (dir, path)
}

/// The most Transfers Alice showed as Transferring at the same moment. A Transfer counts until
/// its next event, which Alice emits before it frees the slot, so this never undercounts.
fn most_downloading_at_once(alice: &TestDevice) -> usize {
    let mut now = HashSet::new();
    let mut most = 0;
    for event in &alice.log {
        let EventKind::Transfer(t) = &event.kind else { continue };
        if t.state == TransferState::Transferring {
            now.insert(t.transfer_id);
        } else {
            now.remove(&t.transfer_id);
        }
        most = most.max(now.len());
    }
    most
}

/// A Receiver written by hand, so a test decides what it says and when. It never fetches
/// anything: once it has said yes and been told to go ahead it just sits there, holding one
/// of the Sender's download slots until the test lets go.
struct RawReceiver {
    endpoint: Endpoint,
    send: Option<SendStream>,
    incoming: Option<mpsc::Receiver<Result<Message, FrameError>>>,
    conn: Option<Connection>,
    /// Set by `read_offer`.
    id: Option<TransferId>,
}

impl RawReceiver {
    async fn start() -> Self {
        let endpoint = raw_peer().await;
        endpoint.set_alpns(vec![protocol::ALPN.to_vec()]);
        Self { endpoint, send: None, incoming: None, conn: None, id: None }
    }

    fn addr(&self) -> DeviceAddr {
        let id = data_encoding::BASE32_NOPAD.encode(self.endpoint.id().as_bytes());
        DeviceAddr { id: id.parse::<DeviceId>().unwrap(), direct: self.endpoint.bound_sockets() }
    }

    /// Takes the Sender's call and reads the Offer it makes.
    async fn read_offer(&mut self) -> Offer {
        let conn = self.endpoint.accept().await.expect("the Sender dials").await.unwrap();
        let (mut send, recv) = conn.accept_bi().await.unwrap();
        let mut incoming = spawn_reader(recv);
        assert!(matches!(next(&mut incoming).await, Message::Hello(_)));
        write_frame(&mut send, &Message::Hello(Hello::current())).await.unwrap();
        let Message::Offer(offer) = next(&mut incoming).await else { panic!("expected an Offer") };
        self.id = Some(TransferId::from_bytes(offer.transfer_id));
        (self.send, self.incoming, self.conn) = (Some(send), Some(incoming), Some(conn));
        offer
    }

    fn id(&self) -> TransferId {
        self.id.expect("read_offer first")
    }

    async fn say(&mut self, msg: Message) {
        write_frame(self.send.as_mut().unwrap(), &msg).await.unwrap();
    }

    async fn hear(&mut self) -> Message {
        next(self.incoming.as_mut().unwrap()).await
    }

    /// Says yes, then waits for the go-ahead to fetch: a slot of its own.
    async fn accept(&mut self) {
        self.say(Message::Accept).await;
        assert!(matches!(self.hear().await, Message::HashReady { .. }));
    }

    /// Says the Transfer is done, then waits for the Sender to hang up.
    async fn complete(&mut self) {
        self.say(Message::Completed).await;
        let incoming = self.incoming.as_mut().unwrap();
        let hung_up =
            tokio::time::timeout(ANSWER_TIMEOUT, async { while incoming.recv().await.is_some() {} }).await;
        assert!(hung_up.is_ok(), "the Sender did not finish the Transfer");
    }

    /// Hangs up the control connection, as a Receiver that lost its network does.
    fn drop_connection(&mut self) {
        self.conn.take().unwrap().close(0u32.into(), b"gone");
        self.send = None;
        self.incoming = None;
    }

    /// Dials the Sender again and asks to resume. Returns what the Sender says in answer.
    async fn resume(&mut self, alice: &TestDevice) -> mpsc::Receiver<Result<Message, FrameError>> {
        let conn = self.endpoint.connect(dial_addr(alice), protocol::ALPN).await.unwrap();
        let (mut send, recv) = conn.open_bi().await.unwrap();
        let mut incoming = spawn_reader(recv);
        write_frame(&mut send, &Message::Hello(Hello::current())).await.unwrap();
        assert!(matches!(next(&mut incoming).await, Message::Hello(_)));
        write_frame(&mut send, &Message::Resume { transfer_id: *self.id().as_bytes() }).await.unwrap();
        (self.send, self.conn) = (Some(send), Some(conn));
        incoming
    }
}

async fn next(incoming: &mut mpsc::Receiver<Result<Message, FrameError>>) -> Message {
    tokio::time::timeout(ANSWER_TIMEOUT, incoming.recv())
        .await
        .expect("timed out waiting for the Sender")
        .expect("the Sender closed the stream")
        .expect("a well-formed frame")
}

#[tokio::test]
async fn each_receiver_gets_an_ordinary_transfer_that_ends_on_its_own() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let mut carol = TestDevice::start("carol").await;
    let mut dave = TestDevice::start("dave").await;
    let (_src, path) = source("report.txt", b"the same for everyone\n");

    let batch = alice
        .device
        .send_batch(&[bob.addr(), carol.addr(), dave.addr()], &[path])
        .await
        .unwrap();

    let [to_bob, to_carol, to_dave] = batch.transfers[..] else { panic!("one Transfer per Receiver") };
    assert_eq!(HashSet::from([to_bob, to_carol, to_dave]).len(), 3, "each has its own Transfer ID");
    // Every Receiver sees an ordinary Offer: no Batch, nobody else.
    for (receiver, id) in [(&mut bob, to_bob), (&mut carol, to_carol), (&mut dave, to_dave)] {
        let offer = receiver.wait_offer().await;
        assert_eq!(offer.transfer_id, id);
        assert_eq!((offer.batch_id, offer.items), (None, vec!["report.txt".to_owned()]));
    }

    // Bob takes it, Carol turns it down, Dave drops out: none of that touches the others.
    bob.device.accept(to_bob).await.unwrap();
    carol.device.decline(to_carol).await.unwrap();
    dave.device.cancel(to_dave).await.unwrap();
    bob.wait_state(to_bob, "completed").await;
    alice.wait_state(to_bob, "completed").await;
    assert_eq!(alice.wait_state(to_carol, "declined").await.state, TransferState::Declined);
    assert_eq!(
        alice.wait_state(to_dave, "cancelled").await.state,
        TransferState::Cancelled { by: Role::Receiver }
    );
    assert_eq!(std::fs::read(bob.save_dir.join("report.txt")).unwrap(), b"the same for everyone\n");
    assert!(!carol.save_dir.join("report.txt").exists() && !dave.save_dir.join("report.txt").exists());

    // Alice's events and records tie the three together; the Receivers' do not.
    let batch_of = |device: &TestDevice, id| {
        device.log.iter().find_map(|e| match &e.kind {
            EventKind::Transfer(t) if t.transfer_id == id => Some(t.batch_id),
            _ => None,
        })
    };
    for id in [to_bob, to_carol, to_dave] {
        assert_eq!(batch_of(&alice, id), Some(Some(batch.id)));
    }
    let records = alice.device.transfers().await.unwrap();
    assert_eq!(records.len(), 3);
    assert!(records.iter().all(|r| r.batch_id == Some(batch.id)));
    assert!(bob.device.transfers().await.unwrap().iter().all(|r| r.batch_id.is_none()));

    for device in [&mut alice, &mut bob, &mut carol, &mut dave] {
        device.shutdown().await;
    }
}

#[tokio::test]
async fn offers_in_a_batch_differ_only_by_their_transfer_id() {
    let mut alice = TestDevice::start("alice").await;
    let mut raws = [RawReceiver::start().await, RawReceiver::start().await, RawReceiver::start().await];
    let (_src, path) = source("a.txt", b"hello");
    let to: Vec<_> = raws.iter().map(RawReceiver::addr).collect();

    alice.device.send_batch(&to, &[path]).await.unwrap();

    let mut offers = Vec::new();
    for raw in &mut raws {
        offers.push(raw.read_offer().await);
    }
    // The Offer has no field for anything else; what it holds is the same for everyone.
    let ids: HashSet<_> = offers.iter().map(|o| o.transfer_id).collect();
    assert_eq!(ids.len(), 3);
    for offer in &offers {
        assert_eq!(Offer { transfer_id: offers[0].transfer_id, ..offer.clone() }, offers[0]);
    }
    alice.shutdown().await;
}

#[tokio::test]
async fn a_batch_needs_at_least_one_receiver_and_each_only_once() {
    let mut alice = TestDevice::start("alice").await;
    let bob = TestDevice::start("bob").await;
    let (_src, path) = source("a.txt", b"hello");

    let none = alice.device.send_batch(&[], &[path.clone()]).await;
    assert!(matches!(none, Err(Error::NoReceivers)));
    let twice = alice.device.send_batch(&[bob.addr(), bob.addr()], &[path]).await;
    assert!(matches!(twice, Err(Error::DuplicateReceiver(id)) if id == bob.device.device_id()));
    assert!(alice.device.transfers().await.unwrap().is_empty(), "nothing was sent");
    alice.shutdown().await;
}

#[tokio::test]
async fn only_three_receivers_download_at_once_and_the_rest_wait_for_a_slot() {
    let mut alice = TestDevice::start("alice").await;
    let mut raws = [RawReceiver::start().await, RawReceiver::start().await, RawReceiver::start().await];
    let mut dave = TestDevice::start("dave").await;
    let bytes = pseudo_random_bytes(300_000, 7);
    let (_src, path) = source("big.bin", &bytes);
    let to = [raws[0].addr(), raws[1].addr(), raws[2].addr(), dave.addr()];

    let batch = alice.device.send_batch(&to, &[path]).await.unwrap();
    let to_dave = batch.transfers[3];
    for raw in &mut raws {
        raw.read_offer().await;
    }
    dave.wait_offer().await;

    // Three Receivers accept and are let go: they hold every slot.
    for raw in &mut raws {
        raw.accept().await;
        alice.wait_state(raw.id(), "transferring").await;
    }
    // The fourth accepts and has to wait. It is not told to go ahead, so it fetches nothing.
    dave.device.accept(to_dave).await.unwrap();
    alice.wait_state(to_dave, "waiting").await;
    alice.quiet_for(QUIET).await;
    dave.quiet_for(QUIET).await;
    assert_eq!(dave.history(to_dave), ["offered", "accepted"]);
    assert!(!alice.history(to_dave).contains(&"transferring"));

    // One finishes, and the one that waited goes ahead and delivers.
    raws[0].complete().await;
    alice.wait_state(to_dave, "transferring").await;
    dave.wait_state(to_dave, "completed").await;
    alice.wait_state(to_dave, "completed").await;
    assert_eq!(std::fs::read(dave.save_dir.join("big.bin")).unwrap(), bytes);
    assert_eq!(alice.history(to_dave), ["offered", "accepted", "waiting", "transferring", "completed"]);
    assert_eq!(most_downloading_at_once(&alice), 3, "the limit was reached and never passed");

    for raw in &mut raws[1..] {
        raw.say(Message::Cancel).await;
    }
    alice.shutdown().await;
    dave.shutdown().await;
}

#[tokio::test]
async fn a_receiver_that_comes_back_waits_for_a_slot_like_any_other() {
    let mut alice = TestDevice::start("alice").await;
    let mut raws: Vec<_> = Vec::new();
    for _ in 0..4 {
        raws.push(RawReceiver::start().await);
    }
    let (_src, path) = source("a.txt", b"hello");
    let to: Vec<_> = raws.iter().map(RawReceiver::addr).collect();
    alice.device.send_batch(&to, &[path]).await.unwrap();
    for raw in &mut raws {
        raw.read_offer().await;
    }
    for raw in &mut raws[..3] {
        raw.accept().await;
        alice.wait_state(raw.id(), "transferring").await;
    }
    raws[3].say(Message::Accept).await;
    let (first, last) = (raws[0].id(), raws[3].id());
    alice.wait_state(last, "waiting").await;

    // The first loses its connection. Nobody is downloading through it, so its slot goes to
    // the one that waited.
    raws[0].drop_connection();
    assert!(matches!(raws[3].hear().await, Message::HashReady { .. }));
    alice.wait_state(last, "transferring").await;

    // It dials back, but every slot is taken again: it is kept waiting, not answered.
    let mut answer = raws[0].resume(&alice).await;
    alice.wait_state(first, "waiting").await;
    assert!(tokio::time::timeout(QUIET, answer.recv()).await.is_err(), "answered with no slot free");

    // A slot frees, and it is let back in.
    raws[3].complete().await;
    assert_eq!(next(&mut answer).await, Message::ResumeOk);
    alice.wait_state(first, "transferring").await;

    for raw in &mut raws[..3] {
        raw.say(Message::Cancel).await;
    }
    alice.shutdown().await;
}

#[tokio::test]
async fn the_sender_can_cancel_one_receiver_without_touching_the_others() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let mut carol = TestDevice::start("carol").await;
    let mut dave = TestDevice::start("dave").await;
    let (_src, path) = source("a.txt", b"hello");
    let batch = alice
        .device
        .send_batch(&[bob.addr(), carol.addr(), dave.addr()], &[path])
        .await
        .unwrap();
    let [to_bob, to_carol, to_dave] = batch.transfers[..] else { panic!("one Transfer per Receiver") };
    for receiver in [&mut bob, &mut carol, &mut dave] {
        receiver.wait_offer().await;
    }

    alice.device.cancel(to_carol).await.unwrap();

    let ended = carol.wait_state(to_carol, "cancelled").await;
    assert_eq!(ended.state, TransferState::Cancelled { by: Role::Sender });
    alice.wait_state(to_carol, "cancelled").await;
    // The others can still be answered, and arrive.
    bob.device.accept(to_bob).await.unwrap();
    dave.device.accept(to_dave).await.unwrap();
    for (receiver, id) in [(&mut bob, to_bob), (&mut dave, to_dave)] {
        receiver.wait_state(id, "completed").await;
        alice.wait_state(id, "completed").await;
        assert_eq!(std::fs::read(receiver.save_dir.join("a.txt")).unwrap(), b"hello");
    }
    for device in [&mut alice, &mut bob, &mut carol, &mut dave] {
        device.shutdown().await;
    }
}

#[tokio::test]
async fn cancelling_a_batch_stops_what_is_running_and_leaves_what_ended() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let mut dave = TestDevice::start("dave").await;
    let mut raws = [RawReceiver::start().await, RawReceiver::start().await, RawReceiver::start().await];
    let (_src, path) = source("a.txt", b"hello");
    let to = [bob.addr(), raws[0].addr(), raws[1].addr(), raws[2].addr(), dave.addr()];
    let batch = alice.device.send_batch(&to, &[path]).await.unwrap();
    let (to_bob, to_dave) = (batch.transfers[0], batch.transfers[4]);

    // Bob is done; three Receivers are downloading and Dave is waiting for a slot.
    bob.wait_offer().await;
    bob.device.accept(to_bob).await.unwrap();
    alice.wait_state(to_bob, "completed").await;
    for raw in &mut raws {
        raw.read_offer().await;
        raw.accept().await;
        alice.wait_state(raw.id(), "transferring").await;
    }
    dave.wait_offer().await;
    dave.device.accept(to_dave).await.unwrap();
    alice.wait_state(to_dave, "waiting").await;

    alice.device.cancel_batch(batch.id).await.unwrap();

    for raw in &mut raws {
        assert_eq!(raw.hear().await, Message::Cancel);
        assert_eq!(
            alice.wait_state(raw.id(), "cancelled").await.state,
            TransferState::Cancelled { by: Role::Sender }
        );
    }
    assert_eq!(
        alice.wait_state(to_dave, "cancelled").await.state,
        TransferState::Cancelled { by: Role::Sender }
    );
    let cancelled = dave.wait_state(to_dave, "cancelled").await;
    assert_eq!(cancelled.state, TransferState::Cancelled { by: Role::Sender });
    assert!(!dave.history(to_dave).contains(&"transferring"));
    // What had finished stays finished.
    assert_eq!(alice.history(to_bob).last(), Some(&"completed"));
    assert_eq!(std::fs::read(bob.save_dir.join("a.txt")).unwrap(), b"hello");

    assert!(matches!(alice.device.cancel_batch(BatchId::random()).await, Err(Error::UnknownBatch(_))));
    for device in [&mut alice, &mut bob, &mut dave] {
        device.shutdown().await;
    }
}

#[tokio::test]
async fn a_failed_transfer_can_be_retried_in_its_batch_and_a_declined_one_cannot() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let mut carol = TestDevice::start("carol").await;
    let mut dave = TestDevice::start("dave").await;
    let (_src, path) = source("report.txt", b"the same for everyone\n");
    let batch = alice
        .device
        .send_batch(&[bob.addr(), carol.addr(), dave.addr()], &[path])
        .await
        .unwrap();
    let [to_bob, to_carol, to_dave] = batch.transfers[..] else { panic!("one Transfer per Receiver") };
    for receiver in [&mut bob, &mut carol, &mut dave] {
        receiver.wait_offer().await;
    }

    // Bob declines, Dave delivers, and Carol's Device goes away before she answers.
    bob.device.decline(to_bob).await.unwrap();
    dave.device.accept(to_dave).await.unwrap();
    dave.wait_state(to_dave, "completed").await;
    carol.shutdown().await;
    alice.wait_state(to_bob, "declined").await;
    alice.wait_state(to_dave, "completed").await;
    let failed = alice.wait_state(to_carol, "failed").await;
    assert!(matches!(failed.state, TransferState::Failed { .. }));

    // Only Carol's can be sent again. Nothing else in the Batch is a candidate.
    let refused = |result: Result<TransferId, Error>| match result {
        Err(Error::NotRetryable(_, why)) => why,
        other => panic!("expected the retry to be refused, got {other:?}"),
    };
    assert!(refused(alice.device.retry(to_bob).await).contains("declined"));
    refused(alice.device.retry(to_dave).await);
    refused(alice.device.retry(TransferId::random()).await);

    // Even after Alice's own restart: what the Batch sent is on disk. Carol is found again by
    // the address she is told to be at.
    alice.restart().await;
    carol.restart().await;
    alice.device.note_address(carol.addr());
    let again = alice.device.retry(to_carol).await.unwrap();
    assert_ne!(again, to_carol, "a new Offer is a new Transfer");

    let offer = carol.wait_offer().await;
    assert_eq!(offer.transfer_id, again);
    let sent = alice.wait_state(again, "offered").await;
    assert_eq!(sent.batch_id, Some(batch.id), "it stays in the Batch");
    carol.device.accept(again).await.unwrap();
    carol.wait_state(again, "completed").await;
    alice.wait_state(again, "completed").await;
    assert_eq!(std::fs::read(carol.save_dir.join("report.txt")).unwrap(), b"the same for everyone\n");

    // Once is enough: the old attempt is history, and the new one is not Failed.
    assert!(refused(alice.device.retry(to_carol).await).contains("already"));
    refused(alice.device.retry(again).await);
    let in_batch = alice.device.transfers().await.unwrap();
    assert_eq!(in_batch.iter().filter(|t| t.batch_id == Some(batch.id)).count(), 4);

    for device in [&mut alice, &mut bob, &mut carol, &mut dave] {
        device.shutdown().await;
    }
}
