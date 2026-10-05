//! The Offer goes out before the files are hashed, and files that change on the Sender before
//! they are served fail the Transfer instead of delivering something else. Where a test must
//! hold a Receiver at one stage (holding a download slot), or a Sender at another (telling the
//! Receiver the content is ready before the answer), a hand-written peer stands in for a real
//! Device.

mod support;

use std::{
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use bhayanakshare_core::{
    DeviceAddr, DeviceId, EventKind, TransferId, TransferState,
    protocol::{self, FrameError, Hello, Message, Offer, read_frame, spawn_reader, write_frame},
};
use iroh::{
    Endpoint,
    endpoint::{Connection, SendStream},
    protocol::Router,
};
use iroh_blobs::{BlobsProtocol, format::collection::Collection, store::mem::MemStore};
use support::{TestDevice, dial_addr, pseudo_random_bytes, raw_peer};
use tempfile::TempDir;
use tokio::sync::mpsc;

const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);
/// Over iroh-blobs' 16 KiB inline limit, so the Sender's store refers to the file.
const SIZE: usize = 1 << 20;
/// Big enough that hashing it takes the Sender far longer than the Receiver takes to say yes,
/// and that a Transfer is still running when a test changes the file.
const BIG: u64 = 256 << 20;
/// How much a Receiver has when the test changes the file under it.
const PART: u64 = 16 << 20;

fn changed(name: &str) -> TransferState {
    TransferState::Failed { reason: format!("A file changed on the sending Device: {name}") }
}

/// `<tmp>/<dir>/<name>` holding `bytes`.
fn file_in(tmp: &TempDir, dir: &str, name: &str, bytes: &[u8]) -> PathBuf {
    std::fs::create_dir_all(tmp.path().join(dir)).unwrap();
    let path = tmp.path().join(dir).join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

/// A sparse file of `len` bytes with a few marks in it. Returns the folder to keep alive and
/// the path.
fn big_file(name: &str, len: u64) -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    let mut file = std::fs::File::create(&path).unwrap();
    file.set_len(len).unwrap();
    for (i, at) in [0, len / 7, len / 3, len / 2].into_iter().enumerate() {
        file.seek(SeekFrom::Start(at)).unwrap();
        file.write_all(format!("mark {i} at {at}").as_bytes()).unwrap();
    }
    (dir, path)
}

/// Overwrites the last bytes of the file in place, leaving its size as it was.
fn overwrite_the_end(path: &Path) {
    let len = std::fs::metadata(path).unwrap().len();
    let mut file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    file.seek(SeekFrom::Start(len - 4096)).unwrap();
    file.write_all(&[0xAB; 4096]).unwrap();
}

/// Alice sends `path` to Bob and Bob accepts it. Returns the Transfer.
async fn sent_and_accepted(alice: &mut TestDevice, bob: &mut TestDevice, path: &Path) -> TransferId {
    let id = alice.device.send_file(bob.addr(), path).await.unwrap();
    bob.wait_offer().await;
    bob.device.accept(id).await.unwrap();
    id
}

fn position(device: &TestDevice, wanted: impl Fn(&EventKind) -> bool) -> usize {
    device.log.iter().position(|e| wanted(&e.kind)).expect("the event is in the log")
}

fn reached(id: TransferId, label: &'static str) -> impl Fn(&EventKind) -> bool {
    move |kind| matches!(kind, EventKind::Transfer(t) if t.transfer_id == id && t.state.label() == label)
}

fn stopped_preparing(id: TransferId) -> impl Fn(&EventKind) -> bool {
    move |kind| matches!(kind, EventKind::Preparing(p) if p.transfer_id == id && !p.preparing)
}

// ---- Offer while hashing --------------------------------------------------------------

#[tokio::test]
async fn the_offer_goes_out_first_and_both_sides_prepare_until_the_files_are_hashed() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let (_src, path) = big_file("movie.bin", BIG);

    let id = alice.device.send_file(bob.addr(), &path).await.unwrap();
    // Bob is shown the Offer, with the names and sizes, while Alice is still hashing.
    let offer = bob.wait_offer().await;
    assert_eq!((offer.name.as_str(), offer.size, offer.file_count), ("movie.bin", BIG, 1));
    bob.device.accept(id).await.unwrap();
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;

    // Both showed Preparing from the Offer on, and stopped when the Sender was done.
    assert_eq!(alice.preparing(id), [true, false]);
    assert_eq!(bob.preparing(id), [true, false]);
    // The answer came first: `HashReady` followed `Accept`, which waited for no one.
    assert!(position(&alice, reached(id, "accepted")) < position(&alice, stopped_preparing(id)));
    assert!(position(&bob, reached(id, "accepted")) < position(&bob, stopped_preparing(id)));
    assert_eq!(alice.history(id), ["offered", "accepted", "transferring", "completed"]);
    assert_eq!(bob.history(id), ["offered", "accepted", "transferring", "saving", "completed"]);
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn hash_ready_after_the_answer_and_hashing_done_before_it_both_complete() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let tmp = tempfile::tempdir().unwrap();
    let bytes = pseudo_random_bytes(SIZE, 5);
    let path = file_in(&tmp, "src", "small.bin", &bytes);

    // Small enough to be hashed long before Bob decides: the content is ready when he says yes.
    let id = alice.device.send_file(bob.addr(), &path).await.unwrap();
    bob.wait_offer().await;
    alice.wait_preparing(id, false).await;
    bob.device.accept(id).await.unwrap();
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;

    assert_eq!(std::fs::read(bob.save_dir.join("small.bin")).unwrap(), bytes);
    assert!(position(&alice, stopped_preparing(id)) < position(&alice, reached(id, "accepted")));
    // The Receiver is told only once it has accepted (spec section 4), whenever hashing ended.
    assert!(position(&bob, reached(id, "accepted")) < position(&bob, stopped_preparing(id)));
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn a_receiver_told_the_content_is_ready_before_it_answers_still_fetches_after_it_does() {
    let mut bob = TestDevice::start("bob").await;
    // A Sender written by hand that sends `HashReady` straight after the Offer.
    let bytes = pseudo_random_bytes(300_000, 3);
    let store = MemStore::new();
    let file = store.blobs().add_bytes(bytes.clone()).temp_tag().await.unwrap();
    let root = Collection::from_iter([("early.bin".to_owned(), file.hash())]).store(&store).await.unwrap();
    let endpoint = raw_peer().await;
    let _provider = Router::builder(endpoint.clone())
        .accept(iroh_blobs::ALPN, BlobsProtocol::new(&store, None))
        .spawn();
    let conn = endpoint.connect(dial_addr(&bob), protocol::ALPN).await.unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, &Message::Hello(Hello::current())).await.unwrap();
    assert!(matches!(read_frame(&mut recv).await.unwrap(), Message::Hello(_)));
    let id = TransferId::from_bytes([4; 16]);
    let offer = support::one_file_offer(*id.as_bytes(), "early.bin", bytes.len() as u64);
    write_frame(&mut send, &Message::Offer(offer)).await.unwrap();
    write_frame(&mut send, &Message::HashReady { collection_hash: *root.hash().as_bytes() })
        .await
        .unwrap();

    bob.wait_offer().await;
    bob.wait_preparing(id, false).await;
    bob.device.accept(id).await.unwrap();
    assert!(matches!(read_frame(&mut recv).await.unwrap(), Message::Accept));
    bob.wait_state(id, "completed").await;

    assert_eq!(std::fs::read(bob.save_dir.join("early.bin")).unwrap(), bytes);
    assert!(position(&bob, stopped_preparing(id)) < position(&bob, reached(id, "accepted")));
    assert_eq!(bob.history(id), ["offered", "accepted", "transferring", "saving", "completed"]);
    // And it told the Sender it had everything.
    let told = tokio::time::timeout(ANSWER_TIMEOUT, async {
        while !matches!(read_frame(&mut recv).await, Ok(Message::Completed)) {}
    })
    .await;
    assert!(told.is_ok(), "the Sender was not told");
    bob.shutdown().await;
}

// ---- Files that change before they are served -----------------------------------------

#[tokio::test]
async fn a_file_that_changed_or_went_after_hashing_fails_the_transfer_before_it_is_served() {
    for delete in [false, true] {
        let mut alice = TestDevice::start("alice").await;
        let mut bob = TestDevice::start("bob").await;
        let tmp = tempfile::tempdir().unwrap();
        let path = file_in(&tmp, "src", "report.txt", b"the numbers as they were");

        let id = alice.device.send_file(bob.addr(), &path).await.unwrap();
        bob.wait_offer().await;
        alice.wait_preparing(id, false).await;
        if delete {
            std::fs::remove_file(&path).unwrap();
        } else {
            std::fs::write(&path, b"the numbers, corrected").unwrap();
        }
        bob.device.accept(id).await.unwrap();

        for device in [&mut alice, &mut bob] {
            let failed = device.wait_state(id, "failed").await;
            assert_eq!(failed.state, changed("report.txt"), "{}, delete: {delete}", device.name);
        }
        // Nothing was served: the Receiver never started to fetch, and saved nothing.
        assert_eq!(alice.history(id), ["offered", "accepted", "failed"]);
        assert_eq!(bob.history(id), ["offered", "accepted", "failed"]);
        assert!(support::list_dir(&bob.save_dir).iter().all(|name| name.starts_with('.')));
        alice.shutdown().await;
        bob.shutdown().await;
    }
}

#[tokio::test]
async fn a_file_in_a_folder_is_named_by_its_path_in_the_offer() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let tmp = tempfile::tempdir().unwrap();
    file_in(&tmp, "album", "a.txt", b"alpha");
    let inner = file_in(&tmp, "album/notes", "b.txt", b"beta");

    let id = alice.device.send(bob.addr(), &[tmp.path().join("album")]).await.unwrap();
    bob.wait_offer().await;
    alice.wait_preparing(id, false).await;
    std::fs::write(inner, b"beta, revised").unwrap();
    bob.device.accept(id).await.unwrap();

    let failed = bob.wait_state(id, "failed").await;
    assert_eq!(failed.state, changed("album/notes/b.txt"));
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn a_file_that_changes_while_the_receiver_waits_for_a_slot_fails_that_transfer_only() {
    for delete in [false, true] {
        let mut alice = TestDevice::start("alice").await;
        let mut raws = [RawReceiver::start().await, RawReceiver::start().await, RawReceiver::start().await];
        let mut dave = TestDevice::start("dave").await;
        let tmp = tempfile::tempdir().unwrap();
        let path = file_in(&tmp, "src", "big.bin", &pseudo_random_bytes(SIZE, 7));
        let to = [raws[0].addr(), raws[1].addr(), raws[2].addr(), dave.addr()];

        let batch = alice.device.send_batch(&to, &[path.clone()]).await.unwrap();
        let to_dave = batch.transfers[3];
        for raw in &mut raws {
            raw.read_offer().await;
        }
        dave.wait_offer().await;
        // Three Receivers hold every slot, so Dave, who has accepted, waits.
        for raw in &mut raws {
            raw.accept().await;
            alice.wait_state(raw.id(), "transferring").await;
        }
        dave.device.accept(to_dave).await.unwrap();
        alice.wait_state(to_dave, "waiting").await;

        if delete {
            std::fs::remove_file(&path).unwrap();
        } else {
            std::fs::write(&path, pseudo_random_bytes(SIZE, 8)).unwrap();
        }
        // A slot frees, and Dave's turn comes: the file is looked at before he is let fetch it.
        raws[0].complete().await;

        for device in [&mut alice, &mut dave] {
            let failed = device.wait_state(to_dave, "failed").await;
            assert_eq!(failed.state, changed("big.bin"), "{}, delete: {delete}", device.name);
        }
        assert_eq!(alice.history(to_dave), ["offered", "accepted", "waiting", "failed"]);
        assert_eq!(dave.history(to_dave), ["offered", "accepted", "failed"]);
        // The ones that were let go earlier carry on; the Transfers are each their own.
        assert!(!alice.history(raws[1].id()).contains(&"failed"));

        for raw in &mut raws[1..] {
            raw.say(Message::Cancel).await;
        }
        alice.shutdown().await;
        dave.shutdown().await;
    }
}

#[tokio::test]
async fn a_file_that_changes_while_it_is_served_never_delivers_other_bytes() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let (_src, path) = big_file("movie.bin", BIG);

    let id = sent_and_accepted(&mut alice, &mut bob, &path).await;
    bob.wait_progress(id, PART).await;
    // Overwritten in place, so it is as long as it was and the Receiver would get other bytes
    // for the end of it.
    overwrite_the_end(&path);

    for device in [&mut bob, &mut alice] {
        let failed = device.wait_state(id, "failed").await;
        assert_eq!(failed.state, changed("movie.bin"), "{}", device.name);
    }
    assert!(!bob.save_dir.join("movie.bin").exists());
    alice.shutdown().await;
    bob.shutdown().await;
}

// ---- Sending content again --------------------------------------------------------------

/// What Alice's store makes of a file that was sent, is gone or changed, and whose content is
/// sent again from another path (see `sender::import`).
#[tokio::test]
async fn the_same_content_sent_again_from_another_path_after_the_first_file_is_gone_or_changed() {
    for gone in [true, false] {
        let mut alice = TestDevice::start("alice").await;
        let mut bob = TestDevice::start("bob").await;
        let tmp = tempfile::tempdir().unwrap();
        let bytes = pseudo_random_bytes(SIZE, 11);

        // The store lists a file's paths in order, so the first is the one that sorts first.
        let first = file_in(&tmp, "a", "photo.bin", &bytes);
        let id = sent_and_accepted(&mut alice, &mut bob, &first).await;
        bob.wait_state(id, "completed").await;
        alice.wait_state(id, "completed").await;
        if gone {
            std::fs::remove_file(&first).unwrap();
        } else {
            std::fs::write(&first, pseudo_random_bytes(SIZE, 12)).unwrap();
        }

        let second = file_in(&tmp, "b", "again.bin", &bytes);
        let id = sent_and_accepted(&mut alice, &mut bob, &second).await;
        bob.wait_state(id, "completed").await;
        alice.wait_state(id, "completed").await;

        assert_eq!(std::fs::read(bob.save_dir.join("again.bin")).unwrap(), bytes, "gone: {gone}");
        alice.shutdown().await;
        bob.shutdown().await;
    }
}

#[tokio::test]
async fn content_sent_again_is_served_after_the_sender_restarts() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let tmp = tempfile::tempdir().unwrap();
    let bytes = pseudo_random_bytes(SIZE, 13);
    let first = file_in(&tmp, "a", "photo.bin", &bytes);
    let id = sent_and_accepted(&mut alice, &mut bob, &first).await;
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;
    std::fs::remove_file(&first).unwrap();

    // What the store refers to outlives the run that made it, and so does what is known of it.
    alice.restart().await;
    let second = file_in(&tmp, "b", "again.bin", &bytes);
    let id = sent_and_accepted(&mut alice, &mut bob, &second).await;
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;

    assert_eq!(std::fs::read(bob.save_dir.join("again.bin")).unwrap(), bytes);
    alice.shutdown().await;
    bob.shutdown().await;
}

// ---- A Receiver written by hand --------------------------------------------------------

/// A Receiver written by hand, so a test decides what it says and when. It never fetches
/// anything: once it has said yes and been told to go ahead it just sits there, holding one of
/// the Sender's download slots until the test lets go.
struct RawReceiver {
    endpoint: Endpoint,
    send: Option<SendStream>,
    incoming: Option<mpsc::Receiver<Result<Message, FrameError>>>,
    _conn: Option<Connection>,
    /// Set by `read_offer`.
    id: Option<TransferId>,
}

impl RawReceiver {
    async fn start() -> Self {
        let endpoint = raw_peer().await;
        endpoint.set_alpns(vec![protocol::ALPN.to_vec()]);
        Self { endpoint, send: None, incoming: None, _conn: None, id: None }
    }

    fn addr(&self) -> DeviceAddr {
        let id = data_encoding::BASE32_NOPAD.encode(self.endpoint.id().as_bytes());
        DeviceAddr { id: id.parse::<DeviceId>().unwrap(), direct: self.endpoint.bound_sockets(), relay_url: None }
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
        (self.send, self.incoming, self._conn) = (Some(send), Some(incoming), Some(conn));
        offer
    }

    fn id(&self) -> TransferId {
        self.id.expect("read_offer first")
    }

    async fn say(&mut self, msg: Message) {
        write_frame(self.send.as_mut().unwrap(), &msg).await.unwrap();
    }

    /// Says yes, then waits for the go-ahead to fetch: a slot of its own.
    async fn accept(&mut self) {
        self.say(Message::Accept).await;
        assert!(matches!(next(self.incoming.as_mut().unwrap()).await, Message::HashReady { .. }));
    }

    /// Says the Transfer is done, then waits for the Sender to hang up.
    async fn complete(&mut self) {
        self.say(Message::Completed).await;
        let incoming = self.incoming.as_mut().unwrap();
        let hung_up =
            tokio::time::timeout(ANSWER_TIMEOUT, async { while incoming.recv().await.is_some() {} }).await;
        assert!(hung_up.is_ok(), "the Sender did not finish the Transfer");
    }
}

async fn next(incoming: &mut mpsc::Receiver<Result<Message, FrameError>>) -> Message {
    tokio::time::timeout(ANSWER_TIMEOUT, incoming.recv())
        .await
        .expect("timed out waiting for the Sender")
        .expect("the Sender closed the stream")
        .expect("a well-formed frame")
}
