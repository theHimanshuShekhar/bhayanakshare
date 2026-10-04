//! Who may fetch a Transfer's content from the Sender: only the Receiver that accepted it,
//! only while the Transfer is running, and only by a GET of its root hash. Every test is a
//! hostile peer talking raw iroh-blobs to a real Device; no other kind of request, from
//! anyone, is ever served (iroh-blobs 0.103 would accept a PUSH from any peer by default).

mod support;

use std::time::Duration;

use bhayanakshare_core::{
    DeviceAddr, DeviceId, TransferId,
    protocol::{self, FrameError, Hello, Message, spawn_reader, write_frame},
    store,
};
use iroh::{
    Endpoint, EndpointAddr,
    endpoint::{Connection, ReadError, SendStream, VarInt},
};
use iroh_blobs::{
    Hash, HashAndFormat,
    api::blobs::BlobStatus,
    format::collection::Collection,
    protocol::{
        ChunkRangesSeq, ERR_PERMISSION, GetManyRequest, GetRequest, ObserveRequest, PushRequest,
        Request, RequestType,
    },
    store::mem::MemStore,
};
use support::{TestDevice, dial_addr, pseudo_random_bytes, raw_peer};
use tempfile::TempDir;
use tokio::sync::mpsc;

/// How long a test waits for Alice to answer.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

/// The root hash of the Collection Alice builds for a one-file Transfer. Anyone who has the
/// same file knows it, which is what makes it a hash an attacker can ask for.
async fn root_hash(name: &str, bytes: &[u8]) -> Hash {
    let store = MemStore::new();
    let file = store.blobs().add_bytes(bytes.to_vec()).temp_tag().await.unwrap();
    let collection = Collection::from_iter([(name.to_owned(), file.hash())]);
    collection.store(&store).await.unwrap().hash()
}

/// Sends the file to a real Device that accepts it, so the content is certainly in Alice's
/// store, with no Transfer for it running, before a hostile peer asks for it. (Alice hashes
/// while an Offer waits, so without this "refused" could just mean "not imported yet".)
async fn already_delivered(alice: &mut TestDevice, name: &str, bytes: &[u8]) {
    let mut dave = TestDevice::start("dave").await;
    let src = tempfile::tempdir().unwrap();
    let path = src.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    let id = alice.device.send_file(dave.addr(), &path).await.unwrap();
    dave.wait_offer().await;
    dave.device.accept(id).await.unwrap();
    dave.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;
    dave.shutdown().await;
}

/// A Receiver written by hand, so a test decides exactly what it says and when.
struct RawReceiver {
    id: TransferId,
    /// Worked out from the file, not learned from Alice.
    root: Hash,
    endpoint: Endpoint,
    alice: EndpointAddr,
    /// Where it keeps what it fetches.
    store: MemStore,
    send: SendStream,
    incoming: mpsc::Receiver<Result<Message, FrameError>>,
    /// Keeps the control connection alive.
    _conn: Connection,
    /// Alice references the file in place, so it must outlive the Transfer.
    _src: TempDir,
}

impl RawReceiver {
    /// Alice offers it `name`. It has read the Offer and said nothing back.
    async fn offered(alice: &TestDevice, name: &str, bytes: &[u8]) -> Self {
        let endpoint = raw_peer().await;
        endpoint.set_alpns(vec![protocol::ALPN.to_vec()]);
        let src = tempfile::tempdir().unwrap();
        let path = src.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        let device_id = data_encoding::BASE32_NOPAD.encode(endpoint.id().as_bytes());
        let to = DeviceAddr {
            id: device_id.parse::<DeviceId>().unwrap(),
            direct: endpoint.bound_sockets(),
        };
        let id = alice.device.send_file(to, &path).await.unwrap();

        let conn = endpoint.accept().await.expect("Alice dials").await.unwrap();
        let (mut send, recv) = conn.accept_bi().await.unwrap();
        let mut incoming = spawn_reader(recv);
        assert!(matches!(next(&mut incoming).await, Message::Hello(_)));
        write_frame(&mut send, &Message::Hello(Hello::current())).await.unwrap();
        let Message::Offer(offer) = next(&mut incoming).await else { panic!("expected an Offer") };
        assert_eq!((offer.name.as_str(), offer.size), (name, bytes.len() as u64));
        Self {
            id,
            root: root_hash(name, bytes).await,
            endpoint,
            alice: dial_addr(alice),
            store: MemStore::new(),
            send,
            incoming,
            _conn: conn,
            _src: src,
        }
    }

    /// Says yes, then waits for Alice's `HashReady`: her go-ahead to fetch.
    async fn accept(&mut self) {
        write_frame(&mut self.send, &Message::Accept).await.unwrap();
        let Message::HashReady { collection_hash } = next(&mut self.incoming).await else {
            panic!("expected HashReady");
        };
        assert_eq!(Hash::from(collection_hash), self.root);
    }

    async fn decline(&mut self) {
        write_frame(&mut self.send, &Message::Decline).await.unwrap();
    }

    /// Says the file arrived, then waits for Alice to hang up.
    async fn complete(&mut self) {
        write_frame(&mut self.send, &Message::Completed).await.unwrap();
        let hung_up = tokio::time::timeout(ANSWER_TIMEOUT, async {
            while self.incoming.recv().await.is_some() {}
        })
        .await;
        assert!(hung_up.is_ok(), "Alice did not finish the Transfer");
    }

    /// A new connection to Alice's blobs provider, as this Receiver.
    async fn blobs_conn(&self) -> Connection {
        self.endpoint.connect(self.alice.clone(), iroh_blobs::ALPN).await.unwrap()
    }

    /// Fetches the content the way a real Receiver does and checks it is `bytes`. Returns the
    /// hash of the file inside the Collection.
    async fn fetch_and_check(&self, conn: &Connection, name: &str, bytes: &[u8]) -> Hash {
        let content = HashAndFormat::hash_seq(self.root);
        self.store.remote().fetch(conn.clone(), content).await.expect("an accepted fetch works");
        let collection = Collection::load(self.root, &*self.store).await.unwrap();
        let [(got_name, file)] = collection.iter().cloned().collect::<Vec<_>>().try_into().unwrap();
        assert_eq!(got_name, name);
        let got = self.store.blobs().get_bytes(file).await.unwrap();
        assert_eq!(got.as_ref(), bytes);
        file
    }
}

async fn next(incoming: &mut mpsc::Receiver<Result<Message, FrameError>>) -> Message {
    tokio::time::timeout(ANSWER_TIMEOUT, incoming.recv())
        .await
        .expect("timed out waiting for Alice")
        .expect("Alice closed the stream")
        .expect("a well-formed frame")
}

/// What Alice's provider did with one hand-written request.
#[derive(Debug, PartialEq, Eq)]
enum Answer {
    /// It reset the stream with this error code before serving anything.
    Refused(VarInt),
    /// It served the request, or is waiting for the rest of a PUSH.
    Served,
    Other(String),
}

/// Sends `request` on a new stream of `conn` and reports how the provider answered.
async fn ask(conn: &Connection, request: Request) -> Answer {
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    match &request {
        // A PUSH is the one request that is length-prefixed, because data follows it.
        Request::Push(push) => {
            let body = postcard::to_allocvec(push).unwrap();
            let mut bytes = vec![RequestType::Push as u8];
            bytes.extend(postcard::to_allocvec(&(body.len() as u64)).unwrap());
            bytes.extend(body);
            send.write_all(&bytes).await.unwrap();
        }
        // Every other request runs to the end of the stream.
        other => {
            send.write_all(&postcard::to_allocvec(other).unwrap()).await.unwrap();
            send.finish().unwrap();
        }
    }
    match tokio::time::timeout(ANSWER_TIMEOUT, recv.read(&mut [0u8; 1])).await {
        Ok(Err(ReadError::Reset(code))) => Answer::Refused(code),
        Ok(Err(other)) => Answer::Other(other.to_string()),
        Ok(Ok(_)) | Err(_) => Answer::Served,
    }
}

async fn assert_refused(conn: &Connection, request: Request, what: &str) {
    assert_eq!(ask(conn, request).await, Answer::Refused(ERR_PERMISSION), "{what}");
}

fn get(root: Hash) -> Request {
    Request::Get(GetRequest::from(HashAndFormat::hash_seq(root)))
}

fn get_blob(hash: Hash) -> Request {
    Request::Get(GetRequest::from(HashAndFormat::raw(hash)))
}

fn get_many(hashes: &[Hash]) -> Request {
    Request::GetMany(GetManyRequest::from_iter(hashes.iter().copied()))
}

fn observe(hash: Hash) -> Request {
    Request::Observe(ObserveRequest::new(hash))
}

fn push(hash: Hash) -> Request {
    Request::Push(PushRequest::new(hash, ChunkRangesSeq::root()))
}

#[tokio::test]
async fn a_peer_that_has_not_accepted_cannot_fetch_the_content() {
    let mut alice = TestDevice::start("alice").await;
    let bytes = pseudo_random_bytes(200_000, 1);
    already_delivered(&mut alice, "plans.bin", &bytes).await;
    let bob = RawReceiver::offered(&alice, "plans.bin", &bytes).await;

    // The content is in Alice's store and Bob knows its root hash, but has not said yes.
    let conn = bob.blobs_conn().await;
    assert_refused(&conn, get(bob.root), "GET of an Offered Transfer's root").await;
    let content = HashAndFormat::hash_seq(bob.root);
    assert!(bob.store.remote().fetch(conn, content).await.is_err(), "a real fetch must fail");
    assert_eq!(bob.store.remote().local(content).await.unwrap().local_bytes(), 0);

    // A stranger who knows the same hash fares no better.
    let mallory = raw_peer().await;
    let conn = mallory.connect(dial_addr(&alice), iroh_blobs::ALPN).await.unwrap();
    assert_refused(&conn, get(bob.root), "GET by a stranger").await;
    alice.shutdown().await;
}

#[tokio::test]
async fn a_different_peer_cannot_fetch_what_another_peer_accepted() {
    let mut alice = TestDevice::start("alice").await;
    let bytes = pseudo_random_bytes(200_000, 2);
    let mut bob = RawReceiver::offered(&alice, "plans.bin", &bytes).await;
    bob.accept().await;

    // Control: the Receiver that accepted can fetch.
    let bob_conn = bob.blobs_conn().await;
    let file = bob.fetch_and_check(&bob_conn, "plans.bin", &bytes).await;

    // Mallory knows the root hash and even the file's hash.
    let mallory = raw_peer().await;
    let conn = mallory.connect(dial_addr(&alice), iroh_blobs::ALPN).await.unwrap();
    assert_refused(&conn, get(bob.root), "GET of the root by another peer").await;
    assert_refused(&conn, get_blob(file), "GET of the file by another peer").await;
    let content = HashAndFormat::hash_seq(bob.root);
    assert!(MemStore::new().remote().fetch(conn, content).await.is_err());

    bob.complete().await;
    alice.shutdown().await;
}

#[tokio::test]
async fn an_accepted_peer_gets_only_the_transfer_it_accepted() {
    let mut alice = TestDevice::start("alice").await;
    let plans = pseudo_random_bytes(100_000, 3);
    let photo = pseudo_random_bytes(100_000, 4);
    already_delivered(&mut alice, "photo.bin", &photo).await;
    let mut bob = RawReceiver::offered(&alice, "plans.bin", &plans).await;
    let carol = RawReceiver::offered(&alice, "photo.bin", &photo).await;
    bob.accept().await;
    let bob_conn = bob.blobs_conn().await;
    let plans_file = bob.fetch_and_check(&bob_conn, "plans.bin", &plans).await;

    // Bob accepted one Transfer; Carol's content, on the same Sender, is not his.
    assert_refused(&bob_conn, get(carol.root), "GET of another Transfer's root").await;
    // Only the root is served: a file inside the Collection is not requested on its own.
    assert_refused(&bob_conn, get_blob(plans_file), "GET of a child hash").await;
    // And Carol has not accepted hers, so she cannot fetch Bob's either.
    let carol_conn = carol.blobs_conn().await;
    assert_refused(&carol_conn, get(bob.root), "GET of Bob's Transfer by Carol").await;
    assert_refused(&carol_conn, get(carol.root), "GET of Carol's Offered Transfer").await;

    bob.complete().await;
    alice.shutdown().await;
}

#[tokio::test]
async fn a_declined_transfers_content_cannot_be_fetched() {
    let mut alice = TestDevice::start("alice").await;
    let bytes = pseudo_random_bytes(100_000, 5);
    already_delivered(&mut alice, "plans.bin", &bytes).await;
    let mut bob = RawReceiver::offered(&alice, "plans.bin", &bytes).await;
    let before = bob.blobs_conn().await;

    bob.decline().await;
    alice.wait_state(bob.id, "declined").await;

    assert_refused(&before, get(bob.root), "GET on a connection opened before the decline").await;
    assert_refused(&bob.blobs_conn().await, get(bob.root), "GET on a new connection").await;
    alice.shutdown().await;
}

#[tokio::test]
async fn a_completed_transfers_content_cannot_be_fetched_again() {
    let mut alice = TestDevice::start("alice").await;
    let bytes = pseudo_random_bytes(100_000, 6);
    let mut bob = RawReceiver::offered(&alice, "plans.bin", &bytes).await;
    bob.accept().await;
    let conn = bob.blobs_conn().await;
    bob.fetch_and_check(&conn, "plans.bin", &bytes).await;

    bob.complete().await;
    alice.wait_state(bob.id, "completed").await;

    assert_refused(&conn, get(bob.root), "GET on the connection used for the Transfer").await;
    assert_refused(&bob.blobs_conn().await, get(bob.root), "GET on a new connection").await;
    alice.shutdown().await;
}

#[tokio::test]
async fn push_get_many_and_observe_are_refused_for_every_peer() {
    let mut alice = TestDevice::start("alice").await;
    let bytes = pseudo_random_bytes(100_000, 7);
    let mut bob = RawReceiver::offered(&alice, "plans.bin", &bytes).await;
    bob.accept().await;
    let bob_conn = bob.blobs_conn().await;
    let file = bob.fetch_and_check(&bob_conn, "plans.bin", &bytes).await;
    let mallory = raw_peer().await;
    let mallory_conn = mallory.connect(dial_addr(&alice), iroh_blobs::ALPN).await.unwrap();

    // Mallory, who is nobody, and Bob, who accepted this very content.
    for (who, conn) in [("a stranger", &mallory_conn), ("the accepted Receiver", &bob_conn)] {
        let root = bob.root;
        assert_refused(conn, push(Hash::new(b"not in the store")), &format!("PUSH by {who}")).await;
        assert_refused(conn, push(root), &format!("PUSH of the root by {who}")).await;
        assert_refused(conn, get_many(&[file]), &format!("GET_MANY of the file by {who}")).await;
        assert_refused(conn, get_many(&[root]), &format!("GET_MANY of the root by {who}")).await;
        assert_refused(conn, observe(root), &format!("OBSERVE of the root by {who}")).await;
        assert_refused(conn, observe(file), &format!("OBSERVE of the file by {who}")).await;
    }

    // None of that disturbed the Transfer.
    bob.complete().await;
    alice.wait_state(bob.id, "completed").await;
    alice.shutdown().await;
}

#[tokio::test]
async fn a_blob_pushed_at_the_sender_never_reaches_its_store() {
    let mut alice = TestDevice::start("alice").await;
    let bytes = pseudo_random_bytes(100_000, 8);
    let mut bob = RawReceiver::offered(&alice, "plans.bin", &bytes).await;
    bob.accept().await;
    let bob_conn = bob.blobs_conn().await;
    bob.fetch_and_check(&bob_conn, "plans.bin", &bytes).await;
    let mallory = raw_peer().await;
    let mallory_conn = mallory.connect(dial_addr(&alice), iroh_blobs::ALPN).await.unwrap();

    // Each pushes a blob over the 16 KiB inline limit, with the real client, all the way.
    let mut pushed = Vec::new();
    for (conn, seed) in [(&mallory_conn, 9), (&bob_conn, 10)] {
        let attacker = MemStore::new();
        let blob = attacker.blobs().add_bytes(pseudo_random_bytes(1 << 20, seed)).await.unwrap();
        // The client may finish writing before the provider says no, so its result proves
        // nothing; the hand-written PUSH below is the one that reads the provider's answer.
        let request = PushRequest::new(blob.hash, ChunkRangesSeq::root());
        let _ = attacker.remote().execute_push(conn.clone(), request).await;
        assert_refused(conn, push(blob.hash), "PUSH").await;
        pushed.push(blob.hash);
    }

    bob.complete().await;
    alice.wait_state(bob.id, "completed").await;
    alice.shutdown().await;

    // Reopen Alice's store: it holds nothing of what was pushed.
    let store = store::open(&alice.data_dir.join("blobs")).await.unwrap();
    for hash in pushed {
        let status = store.blobs().status(hash).await.unwrap();
        assert!(matches!(status, BlobStatus::NotFound), "{hash} was pushed in: {status:?}");
    }
    store.shutdown().await.unwrap();
}
