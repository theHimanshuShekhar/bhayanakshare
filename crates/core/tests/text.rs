//! Text Transfers: up to 64 KiB of UTF-8 travels inline in the Offer (nothing hashed, nothing
//! fetched, nothing in the save folder), longer text is sent as a file called `text.txt`.
//! Text fits into the rest of the lifecycle: Batches, Auto-accept, expiry and resend, and a
//! restart, which never resumes an inline text because there is no content to resume from.
//! Where a test must see what is on the wire (no `HashReady`, no blob), a hand-written Receiver
//! stands in for a real Device.

mod support;

use std::{path::Path, time::Duration};

use bhayanakshare_core::{
    DeviceAddr, DeviceId, Error, OFFER_TTL_MS, Role, TransferKind, TransferState,
    protocol::{
        self, FrameError, Hello, MAX_INLINE_TEXT, Message, Offer, OfferKind, spawn_reader, write_frame,
    },
};
use iroh::{
    Endpoint,
    endpoint::{Connection, SendStream},
};
use support::{TestDevice, dial_addr, list_dir, raw_peer};
use tokio::sync::mpsc;

const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);
/// Long enough that something that was going to happen has happened.
const QUIET: Duration = Duration::from_millis(400);

/// Text that is nothing like a file name, and that anything careless with markup or encodings
/// would mangle.
const NASTY: &str = "héllo <b>wörld</b> <script>alert(1)</script>\r\n\ttab & \"quotes\" 日本語 🦀\u{1b}[31m";

/// A Receiver written by hand, so a test decides what it says and when. It never opens a blob
/// connection, so anything that needed one would stall.
struct RawReceiver {
    endpoint: Endpoint,
    send: Option<SendStream>,
    incoming: Option<mpsc::Receiver<Result<Message, FrameError>>>,
    conn: Option<Connection>,
}

impl RawReceiver {
    async fn start() -> Self {
        let endpoint = raw_peer().await;
        endpoint.set_alpns(vec![protocol::ALPN.to_vec()]);
        Self { endpoint, send: None, incoming: None, conn: None }
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
        (self.send, self.incoming, self.conn) = (Some(send), Some(incoming), Some(conn));
        offer
    }

    async fn say(&mut self, msg: Message) {
        write_frame(self.send.as_mut().unwrap(), &msg).await.unwrap();
    }

    async fn hear(&mut self) -> Message {
        next(self.incoming.as_mut().unwrap()).await
    }

    /// What the Sender sends in the next `QUIET`, if anything.
    async fn hear_within_quiet(&mut self) -> Option<Message> {
        let incoming = self.incoming.as_mut().unwrap();
        match tokio::time::timeout(QUIET, incoming.recv()).await {
            Ok(Some(Ok(msg))) => Some(msg),
            Ok(_) | Err(_) => None,
        }
    }

    fn hang_up(&mut self) {
        self.conn.take().unwrap().close(0u32.into(), b"gone");
        self.send = None;
        self.incoming = None;
    }
}

async fn next(incoming: &mut mpsc::Receiver<Result<Message, FrameError>>) -> Message {
    tokio::time::timeout(ANSWER_TIMEOUT, incoming.recv())
        .await
        .expect("timed out waiting for the Sender")
        .expect("the Sender closed the stream")
        .expect("a well-formed frame")
}

/// A hand-written Sender's Offer of `offer`, dialled to `bob`, after Hello; returns what Bob
/// says in reply to it.
async fn offer_to(bob: &TestDevice, offer: Offer) -> Result<Message, FrameError> {
    let peer = raw_peer().await;
    let conn = peer.connect(dial_addr(bob), protocol::ALPN).await.unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, &Message::Hello(Hello::current())).await.unwrap();
    assert!(matches!(protocol::read_frame(&mut recv).await.unwrap(), Message::Hello(_)));
    write_frame(&mut send, &Message::Offer(offer)).await.unwrap();
    tokio::time::timeout(ANSWER_TIMEOUT, protocol::read_frame(&mut recv)).await.expect("Bob answers")
}

/// Waits until `dir` holds nothing (or is gone): the Sender's file for a long text is removed
/// a moment after its Transfer ends.
async fn wait_until_empty(dir: &Path) {
    for _ in 0..200 {
        if !dir.exists() || list_dir(dir).is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{} still holds {:?}", dir.display(), list_dir(dir));
}

#[tokio::test]
async fn inline_text_arrives_with_the_offer_and_is_kept_by_both_sides() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;

    let id = alice.device.send_text(bob.addr(), NASTY).await.unwrap();

    // The Receiver sees the text, exactly, in the Offer: nothing has been fetched.
    let offer = bob.wait_offer().await;
    assert_eq!(offer.transfer_id, id);
    assert_eq!(offer.kind, TransferKind::Text);
    assert_eq!(offer.text.as_deref(), Some(NASTY));
    assert_eq!((offer.size, offer.file_count, offer.skipped_links), (NASTY.len() as u64, 0, 0));
    assert_eq!((offer.name.as_str(), offer.items.len(), offer.adjusted_names), ("", 0, 0));
    assert_eq!(offer.peer, alice.device.device_id());
    // It is not kept until it is accepted.
    assert_eq!(bob.device.transfers().await.unwrap()[0].text, None);

    bob.device.accept(id).await.unwrap();
    let done = bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;

    // Nothing to fetch or save: no Transferring, no Saving, no saved path, no folder.
    assert_eq!(done.state, TransferState::Completed { saved_to: None });
    assert_eq!(done.text.as_deref(), Some(NASTY));
    assert_eq!(alice.history(id), ["offered", "accepted", "completed"]);
    assert_eq!(bob.history(id), ["offered", "accepted", "completed"]);
    assert!(bob.log.iter().all(|e| !matches!(e.kind, bhayanakshare_core::EventKind::Progress(_))));
    assert_eq!(list_dir(&bob.save_dir), Vec::<String>::new(), "no file, no incoming store");

    // Both Devices keep the whole text, byte for byte, for Transfer History, across a restart.
    alice.restart().await;
    bob.restart().await;
    for (device, role) in [(&alice, Role::Sender), (&bob, Role::Receiver)] {
        let records = device.device.transfers().await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!((records[0].kind, records[0].role), (TransferKind::Text, role));
        assert_eq!(records[0].text.as_deref(), Some(NASTY));
        assert_eq!(records[0].state, TransferState::Completed { saved_to: None });
    }
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn inline_text_needs_no_hash_no_blob_and_no_fetch() {
    let mut alice = TestDevice::start("alice").await;
    let mut raw = RawReceiver::start().await;

    let id = alice.device.send_text(raw.addr(), "just words").await.unwrap();
    let offer = raw.read_offer().await;
    assert_eq!(offer.kind, OfferKind::Text("just words".into()));

    // The Receiver says yes. The Sender has nothing to hash or serve, so it sends nothing back:
    // no `HashReady`, and it shows Accepted, not Transferring.
    raw.say(Message::Accept).await;
    alice.wait_state(id, "accepted").await;
    assert_eq!(raw.hear_within_quiet().await, None);
    assert_eq!(alice.history(id), ["offered", "accepted"]);

    // It is done once the Receiver says it kept it. The connection is never used for a fetch.
    raw.say(Message::Completed).await;
    alice.wait_state(id, "completed").await;
    assert_eq!(alice.history(id), ["offered", "accepted", "completed"]);
    alice.shutdown().await;
}

#[tokio::test]
async fn a_receiver_that_goes_away_after_accepting_fails_the_transfer_there_is_nothing_to_resume() {
    let mut alice = TestDevice::start("alice").await;
    let mut raw = RawReceiver::start().await;

    let id = alice.device.send_text(raw.addr(), "just words").await.unwrap();
    raw.read_offer().await;
    raw.say(Message::Accept).await;
    alice.wait_state(id, "accepted").await;
    raw.hang_up();

    // No content hash was ever exchanged, so (spec section 4) there is no resuming: it fails
    // at once instead of waiting 24 hours.
    let failed = alice.wait_state(id, "failed").await;
    assert!(matches!(failed.state, TransferState::Failed { .. }));
    assert_eq!(alice.history(id), ["offered", "accepted", "failed"]);
    // Cancelling it after the yes was no longer possible either.
    assert!(matches!(alice.device.cancel(id).await, Err(Error::NotRunning(_))));
    alice.shutdown().await;
}

#[tokio::test]
async fn text_of_exactly_64_kib_is_inline_and_one_byte_more_is_a_file_called_text_txt() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;

    let exact = "a".repeat(MAX_INLINE_TEXT);
    let inline = alice.device.send_text(bob.addr(), &exact).await.unwrap();
    let offer = bob.wait_offer().await;
    assert_eq!((offer.kind, offer.text.as_deref()), (TransferKind::Text, Some(exact.as_str())));
    bob.device.accept(inline).await.unwrap();
    bob.wait_state(inline, "completed").await;
    alice.wait_state(inline, "completed").await;
    assert_eq!(alice.history(inline), ["offered", "accepted", "completed"]);
    assert_eq!(list_dir(&bob.save_dir), Vec::<String>::new());

    let long = "a".repeat(MAX_INLINE_TEXT + 1);
    let file = alice.device.send_text(bob.addr(), &long).await.unwrap();
    let offer = bob.wait_offer().await;
    assert_eq!(offer.transfer_id, file);
    // An ordinary Offer of one file, named `text.txt`, with no text in it.
    assert_eq!(offer.kind, TransferKind::Files);
    assert_eq!((offer.name.as_str(), offer.items.clone(), offer.text), ("text.txt", vec!["text.txt".to_owned()], None));
    assert_eq!((offer.size, offer.file_count), (long.len() as u64, 1));
    bob.device.accept(file).await.unwrap();
    bob.wait_state(file, "completed").await;
    alice.wait_state(file, "completed").await;

    assert_eq!(std::fs::read_to_string(bob.save_dir.join("text.txt")).unwrap(), long);
    assert_eq!(alice.history(file), ["offered", "accepted", "transferring", "completed"]);
    assert_eq!(bob.history(file), ["offered", "accepted", "transferring", "saving", "completed"]);
    // Alice's copy of it, made only to send it, goes when the send is over.
    wait_until_empty(&alice.data_dir.join("outgoing-text")).await;
    alice.shutdown().await;
    bob.shutdown().await;

    // A second long text is saved next to the first, as any file with a taken name is.
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let multibyte = "é".repeat(MAX_INLINE_TEXT / 2 + 1);
    assert_eq!(multibyte.len(), MAX_INLINE_TEXT + 2, "over the limit in bytes, not characters");
    let id = alice.device.send_text(bob.addr(), &multibyte).await.unwrap();
    let offer = bob.wait_offer().await;
    assert_eq!((offer.kind, offer.name.as_str()), (TransferKind::Files, "text.txt"));
    bob.device.accept(id).await.unwrap();
    bob.wait_state(id, "completed").await;
    assert_eq!(std::fs::read_to_string(bob.save_dir.join("text.txt")).unwrap(), multibyte);
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn the_same_long_text_can_be_sent_again_once_the_file_made_for_the_first_is_gone() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let long = "the same words ".repeat(10_000);
    assert!(long.len() > MAX_INLINE_TEXT);

    for round in 0..2 {
        let id = alice.device.send_text(bob.addr(), &long).await.unwrap();
        bob.wait_offer().await;
        bob.device.accept(id).await.unwrap();
        bob.wait_state(id, "completed").await;
        alice.wait_state(id, "completed").await;
        // The content is the same, so the store already knows it from the first round, whose
        // file has gone: the second must not depend on that file, not even after a restart.
        wait_until_empty(&alice.data_dir.join("outgoing-text")).await;
        let name = if round == 0 { "text.txt" } else { "text (1).txt" };
        assert_eq!(std::fs::read_to_string(bob.save_dir.join(name)).unwrap(), long, "round {round}");
        // A restart makes the store load what it holds from disk again.
        alice.restart().await;
    }
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn empty_text_is_refused_before_anything_is_sent() {
    let mut alice = TestDevice::start("alice").await;
    let bob = TestDevice::start("bob").await;

    assert!(matches!(alice.device.send_text(bob.addr(), "").await, Err(Error::EmptyText)));
    let batch = alice.device.send_text_batch(&[bob.addr()], "").await;
    assert!(matches!(batch, Err(Error::EmptyText)));
    assert!(matches!(alice.device.send_text_batch(&[], "hi").await, Err(Error::NoReceivers)));
    let twice = alice.device.send_text_batch(&[bob.addr(), bob.addr()], "hi").await;
    assert!(matches!(twice, Err(Error::DuplicateReceiver(_))));
    assert!(alice.device.transfers().await.unwrap().is_empty());
    alice.shutdown().await;
}

#[tokio::test]
async fn declined_text_is_not_kept_and_cancelled_text_never_arrives() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;

    let declined = alice.device.send_text(bob.addr(), "no thanks").await.unwrap();
    bob.wait_offer().await;
    bob.device.decline(declined).await.unwrap();
    alice.wait_state(declined, "declined").await;

    let cancelled = alice.device.send_text(bob.addr(), "never mind").await.unwrap();
    bob.wait_for("the second Offer", |t| t.transfer_id == cancelled).await;
    alice.device.cancel(cancelled).await.unwrap();
    assert_eq!(
        bob.wait_state(cancelled, "cancelled").await.state,
        TransferState::Cancelled { by: Role::Sender }
    );

    // The Receiver saw both, and kept neither.
    for record in bob.device.transfers().await.unwrap() {
        assert_eq!((record.kind, record.text), (TransferKind::Text, None));
    }
    // The Sender keeps what it sent, whatever became of it.
    let sent: Vec<_> = alice.device.transfers().await.unwrap().into_iter().map(|r| r.text).collect();
    assert_eq!(sent, [Some("no thanks".to_owned()), Some("never mind".to_owned())]);
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn an_unanswered_text_offer_expires_and_can_be_sent_again_with_the_same_text() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let id = alice.device.send_text(bob.addr(), NASTY).await.unwrap();
    bob.wait_offer().await;

    alice.clock.advance(OFFER_TTL_MS);
    bob.clock.advance(OFFER_TTL_MS);
    assert_eq!(alice.wait_state(id, "expired").await.state, TransferState::Expired);
    assert_eq!(bob.wait_state(id, "expired").await.state, TransferState::Expired);
    assert_eq!(bob.device.transfers().await.unwrap()[0].text, None, "never accepted, never kept");

    let again = alice.device.resend(id).await.unwrap();
    let offer = bob.wait_for("the new Offer", |t| t.transfer_id == again && t.state == TransferState::Offered).await;
    assert_eq!((offer.kind, offer.text.as_deref()), (TransferKind::Text, Some(NASTY)));
    bob.device.accept(again).await.unwrap();
    bob.wait_state(again, "completed").await;
    alice.wait_state(again, "completed").await;
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn a_restart_before_the_answer_expires_a_text_offer_and_keeps_nothing() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let id = alice.device.send_text(bob.addr(), "pending").await.unwrap();
    bob.wait_offer().await;

    // An unanswered Offer does not survive a restart (spec section 4), text or not.
    bob.restart().await;
    let records = bob.device.transfers().await.unwrap();
    assert_eq!((records[0].state.clone(), records[0].text.clone()), (TransferState::Expired, None));
    // Alice's connection went with it, and there is no content to resume from.
    alice.wait_state(id, "failed").await;
    assert!(matches!(bob.device.accept(id).await, Err(Error::UnknownTransfer(_))));
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn text_to_a_batch_gives_each_receiver_an_ordinary_offer_that_ends_on_its_own() {
    let mut alice = TestDevice::start("alice").await;
    let mut receivers = Vec::new();
    for name in ["bob", "carol", "dave", "erin", "frank"] {
        receivers.push(TestDevice::start(name).await);
    }
    let to: Vec<_> = receivers.iter().map(TestDevice::addr).collect();

    let batch = alice.device.send_text_batch(&to, NASTY).await.unwrap();

    assert_eq!(batch.transfers.len(), 5);
    for (receiver, id) in receivers.iter_mut().zip(&batch.transfers) {
        let offer = receiver.wait_offer().await;
        // An ordinary text Offer: no Batch, and nothing about the other Receivers.
        assert_eq!((offer.transfer_id, offer.batch_id), (*id, None));
        assert_eq!((offer.kind, offer.text.as_deref()), (TransferKind::Text, Some(NASTY)));
    }
    // One takes it, one declines, one drops out; the others take it too.
    receivers[0].device.accept(batch.transfers[0]).await.unwrap();
    receivers[1].device.decline(batch.transfers[1]).await.unwrap();
    receivers[2].device.cancel(batch.transfers[2]).await.unwrap();
    for i in [3, 4] {
        receivers[i].device.accept(batch.transfers[i]).await.unwrap();
    }
    for i in [0, 3, 4] {
        receivers[i].wait_state(batch.transfers[i], "completed").await;
        alice.wait_state(batch.transfers[i], "completed").await;
    }
    assert_eq!(alice.wait_state(batch.transfers[1], "declined").await.state, TransferState::Declined);
    assert_eq!(
        alice.wait_state(batch.transfers[2], "cancelled").await.state,
        TransferState::Cancelled { by: Role::Receiver }
    );

    // No text Transfer downloads anything, so none of them waited for one of 3 slots.
    for i in [0, 3, 4] {
        assert_eq!(alice.history(batch.transfers[i]), ["offered", "accepted", "completed"]);
    }
    // Alice sees one Batch of five Transfers, each with the text.
    let records = alice.device.transfers().await.unwrap();
    assert!(records.iter().all(|r| r.batch_id == Some(batch.id) && r.text.as_deref() == Some(NASTY)));
    assert_eq!(records.len(), 5);
    for receiver in &mut receivers {
        receiver.shutdown().await;
    }
    alice.shutdown().await;
}

#[tokio::test]
async fn a_failed_text_transfer_in_a_batch_can_be_retried_whether_the_text_is_inline_or_a_file() {
    for text in [NASTY.to_owned(), "é".repeat(MAX_INLINE_TEXT)] {
        let long = text.len() > MAX_INLINE_TEXT;
        let mut alice = TestDevice::start("alice").await;
        let mut bob = TestDevice::start("bob").await;
        let mut carol = TestDevice::start("carol").await;
        let batch = alice.device.send_text_batch(&[bob.addr(), carol.addr()], &text).await.unwrap();
        let [to_bob, to_carol] = batch.transfers[..] else { panic!("one Transfer per Receiver") };
        bob.wait_offer().await;
        carol.wait_offer().await;

        // Bob takes it; Carol's Device goes away before she answers.
        bob.device.accept(to_bob).await.unwrap();
        bob.wait_state(to_bob, "completed").await;
        carol.shutdown().await;
        alice.wait_state(to_carol, "failed").await;

        // Even after Alice's own restart, the Batch knows its text: for a file as for an inline one.
        alice.restart().await;
        carol.restart().await;
        alice.device.note_address(carol.addr());
        let again = alice.device.retry(to_carol).await.unwrap();

        let offer = carol.wait_offer().await;
        assert_eq!(offer.transfer_id, again);
        assert_eq!(alice.wait_state(again, "offered").await.batch_id, Some(batch.id));
        if long {
            assert_eq!((offer.kind, offer.name.as_str()), (TransferKind::Files, "text.txt"));
        } else {
            assert_eq!((offer.kind, offer.text.as_deref()), (TransferKind::Text, Some(text.as_str())));
        }
        carol.device.accept(again).await.unwrap();
        carol.wait_state(again, "completed").await;
        alice.wait_state(again, "completed").await;
        if long {
            assert_eq!(std::fs::read_to_string(carol.save_dir.join("text.txt")).unwrap(), text);
        } else {
            let kept = carol.device.transfers().await.unwrap();
            assert_eq!(kept.last().unwrap().text.as_deref(), Some(text.as_str()));
        }
        for device in [&mut alice, &mut bob, &mut carol] {
            device.shutdown().await;
        }
    }
}

#[tokio::test]
async fn auto_accept_takes_text_without_a_prompt_and_without_any_room_in_the_save_folder() {
    let mut alice = TestDevice::start("alice").await;
    // A disk with nothing free: files would be held back for the Receiver to see the warning.
    let mut bob = TestDevice::start_with_free_space("bob", |_: &Path| Ok(0)).await;
    bob.device.add_contact(alice.device.device_id(), None).await.unwrap();
    bob.device.set_auto_accept(alice.device.device_id(), true).await.unwrap();

    let id = alice.device.send_text(bob.addr(), NASTY).await.unwrap();
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;

    // Bob never had an Offer to answer: the Transfer begins Accepted, and the text is kept.
    assert_eq!(bob.history(id), ["accepted", "completed"]);
    assert_eq!(alice.history(id), ["offered", "accepted", "completed"]);
    assert_eq!(bob.device.transfers().await.unwrap()[0].text.as_deref(), Some(NASTY));
    assert_eq!(list_dir(&bob.save_dir), Vec::<String>::new());
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn auto_accept_is_still_only_for_the_contact_that_has_it() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    bob.device.add_contact(alice.device.device_id(), None).await.unwrap();

    let id = alice.device.send_text(bob.addr(), "hello").await.unwrap();
    bob.wait_offer().await;
    assert_eq!(bob.history(id), ["offered"]);
    bob.device.decline(id).await.unwrap();
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn a_text_offer_has_nothing_to_check_against_the_save_folder() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start_with_free_space("bob", |_: &Path| Ok(0)).await;

    let id = alice.device.send_text(bob.addr(), "no room needed").await.unwrap();
    bob.wait_offer().await;

    // Kept in the database, not the folder: no warning, and Accept is open, even for a
    // folder that does not exist.
    let check = bob.device.check_offer(id, None).await.unwrap();
    assert!(check.passes());
    assert!(bob.device.check_offer(id, Some(&bob.save_dir.join("missing"))).await.unwrap().passes());
    bob.device.accept(id).await.unwrap();
    bob.wait_state(id, "completed").await;
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn a_hand_written_offer_of_too_much_inline_text_is_refused_unseen() {
    let mut bob = TestDevice::start("bob").await;

    let bad = [
        ("one byte over 64 KiB", Offer::text([1; 16], "a".repeat(MAX_INLINE_TEXT + 1))),
        ("far over 64 KiB", Offer::text([2; 16], "a".repeat(5 * 1024 * 1024))),
        ("no text", Offer::text([3; 16], String::new())),
        ("a size that is not the length", Offer { size: 3, ..Offer::text([4; 16], "hello".into()) }),
        ("a file count for text", Offer { file_count: 1, ..Offer::text([5; 16], "hello".into()) }),
    ];
    for (what, offer) in bad {
        let reply = offer_to(&bob, offer).await;
        assert!(matches!(reply, Ok(Message::InvalidOffer)), "{what}: {reply:?}");
    }
    // Nothing was shown or recorded for any of them.
    bob.shutdown().await;
    assert!(bob.log.is_empty(), "{:?}", bob.log);
    assert!(bob.device.transfers().await.unwrap().is_empty());
}

#[tokio::test]
async fn text_that_is_not_utf8_is_refused_unseen() {
    let mut bob = TestDevice::start("bob").await;
    let peer = raw_peer().await;
    let conn = peer.connect(dial_addr(&bob), protocol::ALPN).await.unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, &Message::Hello(Hello::current())).await.unwrap();
    protocol::read_frame(&mut recv).await.unwrap();

    // A well-formed Offer of text, with its last byte changed to one that is not UTF-8.
    let mut body = postcard::to_stdvec(&Message::Offer(Offer::text([6; 16], "caf\u{e9}".into()))).unwrap();
    *body.last_mut().unwrap() = 0xff;
    let mut frame = (body.len() as u32).to_be_bytes().to_vec();
    frame.extend_from_slice(&body);
    tokio::io::AsyncWriteExt::write_all(&mut send, &frame).await.unwrap();

    let reply = tokio::time::timeout(ANSWER_TIMEOUT, protocol::read_frame(&mut recv)).await.unwrap();
    assert!(matches!(reply, Ok(Message::InvalidOffer)), "{reply:?}");
    bob.shutdown().await;
    assert!(bob.log.is_empty(), "{:?}", bob.log);
}

#[tokio::test]
async fn a_sender_cannot_make_up_content_for_a_text_offer() {
    let mut bob = TestDevice::start("bob").await;
    let peer = raw_peer().await;
    let conn = peer.connect(dial_addr(&bob), protocol::ALPN).await.unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, &Message::Hello(Hello::current())).await.unwrap();
    protocol::read_frame(&mut recv).await.unwrap();
    write_frame(&mut send, &Message::Offer(Offer::text([7; 16], "hello".into()))).await.unwrap();
    let offer = bob.wait_offer().await;

    // There is no content hash for text; a Sender that sends one anyway ends the Transfer.
    write_frame(&mut send, &Message::HashReady { collection_hash: [9; 32] }).await.unwrap();
    let failed = bob.wait_state(offer.transfer_id, "failed").await;
    assert!(matches!(failed.state, TransferState::Failed { .. }));
    assert!(matches!(bob.device.accept(offer.transfer_id).await, Err(Error::UnknownTransfer(_))));
    bob.shutdown().await;
    assert_eq!(bob.device.transfers().await.unwrap()[0].text, None);
}

#[tokio::test]
async fn files_left_for_long_text_by_an_earlier_run_are_swept_but_those_still_in_use_stay() {
    let mut alice = TestDevice::start("alice").await;
    let mut raw = RawReceiver::start().await;
    let texts = alice.data_dir.join("outgoing-text");

    // A long text whose Receiver has accepted and been told to fetch: alice is serving it, and
    // must be able to carry on after a restart, so its file has to stay.
    let long = "z".repeat(MAX_INLINE_TEXT + 10);
    alice.device.send_text(raw.addr(), &long).await.unwrap();
    raw.read_offer().await;
    raw.say(Message::Accept).await;
    assert!(matches!(raw.hear().await, Message::HashReady { .. }));
    let in_use = list_dir(&texts);
    assert_eq!(in_use.len(), 1);

    // And one a crash left behind, which nothing needs.
    std::fs::create_dir_all(texts.join("left-behind")).unwrap();
    std::fs::write(texts.join("left-behind").join("text.txt"), "stale").unwrap();

    alice.restart().await;
    assert_eq!(list_dir(&texts), in_use);
    assert_eq!(std::fs::read_to_string(texts.join(&in_use[0]).join("text.txt")).unwrap(), long);
    alice.shutdown().await;
}
