//! Devices on different protocol versions refuse each other before any Offer, and both sides
//! are told who has to update. One side of each test is a Device, the other a peer written by
//! hand, so a test decides which version it speaks.

mod support;

use std::time::Duration;

use bhayanakshare_core::{
    DeviceAddr, DeviceId, EventKind, Outdated, TransferState,
    protocol::{self, FrameError, Hello, Message, PROTOCOL_VERSION, spawn_reader, write_frame},
};
use support::{TestDevice, dial_addr, one_file_offer, raw_peer};
use tokio::sync::mpsc;

const TIMEOUT: Duration = Duration::from_secs(10);

/// A `Hello` from a Device on protocol `version`.
fn hello(version: u32, name: &str) -> Hello {
    Hello { protocol_version: version, app_version: format!("0.0.{version}"), device_name: name.into() }
}

type Incoming = mpsc::Receiver<Result<Message, FrameError>>;

async fn next(incoming: &mut Incoming) -> Option<Message> {
    match tokio::time::timeout(TIMEOUT, incoming.recv()).await.expect("timed out") {
        Some(Ok(msg)) => Some(msg),
        Some(Err(_)) | None => None,
    }
}

/// A Sender on protocol `version` dials `bob` and says Hello and then, as if nothing were
/// wrong, makes an Offer. Bob must answer with his own Hello, which the Sender can read, and
/// hang up without a word more.
async fn sender_dials(bob: &TestDevice, version: u32) {
    let peer = raw_peer().await;
    let conn = peer.connect(dial_addr(bob), protocol::ALPN).await.unwrap();
    let (mut send, recv) = conn.open_bi().await.unwrap();
    let mut incoming = spawn_reader(recv);

    write_frame(&mut send, &Message::Hello(hello(version, "Hand Sender"))).await.unwrap();
    write_frame(&mut send, &Message::Offer(one_file_offer([6; 16], "ok.txt", 1))).await.unwrap();
    let Some(Message::Hello(theirs)) = next(&mut incoming).await else { panic!("expected Bob's Hello") };
    assert_eq!(theirs.protocol_version, PROTOCOL_VERSION);
    send.finish().unwrap();
    assert_eq!(next(&mut incoming).await, None, "Bob answers nothing after Hello");
}

/// A Receiver on protocol `version`, which answers Hello and then reports what comes after.
struct HandReceiver {
    endpoint: iroh::Endpoint,
}

impl HandReceiver {
    async fn start() -> Self {
        let endpoint = raw_peer().await;
        endpoint.set_alpns(vec![protocol::ALPN.to_vec()]);
        Self { endpoint }
    }

    fn addr(&self) -> DeviceAddr {
        let id = data_encoding::BASE32_NOPAD.encode(self.endpoint.id().as_bytes());
        DeviceAddr { id: id.parse::<DeviceId>().unwrap(), direct: self.endpoint.bound_sockets(), relay_url: None }
    }

    /// Takes a Sender's call, checks it said Hello on this build's version, answers with
    /// `version`, and returns whatever else the Sender sent.
    async fn greet(&self, version: u32) -> Vec<Message> {
        let conn = self.endpoint.accept().await.expect("the Sender dials").await.unwrap();
        let (mut send, recv) = conn.accept_bi().await.unwrap();
        let mut incoming = spawn_reader(recv);
        let Some(Message::Hello(theirs)) = next(&mut incoming).await else { panic!("expected a Hello") };
        assert_eq!(theirs.protocol_version, PROTOCOL_VERSION);
        write_frame(&mut send, &Message::Hello(hello(version, "Hand Receiver"))).await.unwrap();
        send.finish().unwrap();
        let mut rest = Vec::new();
        while let Some(msg) = next(&mut incoming).await {
            rest.push(msg);
        }
        rest
    }
}

#[tokio::test]
async fn a_receiver_refuses_an_older_sender_and_says_the_sender_must_update() {
    let mut bob = TestDevice::start("bob").await;
    sender_dials(&bob, PROTOCOL_VERSION - 1).await;

    let mismatch = bob.wait_version_mismatch().await;
    assert_eq!(mismatch.outdated, Outdated::Peer);
    assert_eq!(mismatch.peer_name.as_deref(), Some("Hand Sender"));
    assert_eq!(mismatch.peer_app_version, Some(format!("0.0.{}", PROTOCOL_VERSION - 1)));
    bob.shutdown().await;
    assert!(
        bob.log.iter().all(|e| !matches!(e.kind, EventKind::Transfer(_))),
        "the Offer must not surface: {:?}",
        bob.log
    );
    assert!(bob.device.transfers().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_receiver_refuses_a_newer_sender_and_says_it_must_update_itself() {
    let mut bob = TestDevice::start("bob").await;
    sender_dials(&bob, PROTOCOL_VERSION + 1).await;

    let mismatch = bob.wait_version_mismatch().await;
    assert_eq!(mismatch.outdated, Outdated::ThisDevice);
    assert_eq!(mismatch.peer_name.as_deref(), Some("Hand Sender"));
    bob.shutdown().await;
    assert!(bob.log.iter().all(|e| !matches!(e.kind, EventKind::Transfer(_))), "{:?}", bob.log);
    assert!(bob.device.transfers().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_sender_fails_an_older_receiver_before_any_offer_and_says_to_ask_them_to_update() {
    let mut alice = TestDevice::start("alice").await;
    let receiver = HandReceiver::start().await;

    let id = alice.device.send_text(receiver.addr(), "hello").await.unwrap();
    let (rest, failed) = tokio::join!(
        receiver.greet(PROTOCOL_VERSION - 1),
        alice.wait_state(id, "failed"),
    );

    assert!(rest.is_empty(), "no Offer may follow a refused Hello: {rest:?}");
    assert_eq!(
        failed.state,
        TransferState::Failed {
            reason: "Hand Receiver is running an older BhayanakShare. Ask them to update.".into()
        }
    );
    let mismatch = alice.wait_version_mismatch().await;
    assert_eq!(mismatch.outdated, Outdated::Peer);
    assert_eq!(mismatch.peer, receiver.addr().id);
    assert_eq!(mismatch.peer_name.as_deref(), Some("Hand Receiver"));
}

#[tokio::test]
async fn a_sender_fails_a_newer_receiver_before_any_offer_and_says_to_update() {
    let mut alice = TestDevice::start("alice").await;
    let receiver = HandReceiver::start().await;

    let id = alice.device.send_text(receiver.addr(), "hello").await.unwrap();
    let (rest, failed) = tokio::join!(
        receiver.greet(PROTOCOL_VERSION + 1),
        alice.wait_state(id, "failed"),
    );

    assert!(rest.is_empty(), "no Offer may follow a refused Hello: {rest:?}");
    assert_eq!(
        failed.state,
        TransferState::Failed {
            reason: "Hand Receiver is running a newer BhayanakShare. Update this Device, then try again."
                .into()
        }
    );
    assert_eq!(alice.wait_version_mismatch().await.outdated, Outdated::ThisDevice);
}

#[tokio::test]
async fn devices_on_the_same_version_transfer_and_report_no_mismatch() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;

    let id = alice.device.send_text(bob.addr(), "hello").await.unwrap();
    bob.wait_offer().await;
    bob.device.accept(id).await.unwrap();
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;

    alice.shutdown().await;
    bob.shutdown().await;
    for device in [&alice, &bob] {
        assert!(
            device.log.iter().all(|e| !matches!(e.kind, EventKind::VersionMismatch(_))),
            "{}: {:?}",
            device.name,
            device.log
        );
    }
}
