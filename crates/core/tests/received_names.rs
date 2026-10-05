//! Received names are safe to write and never overwrite anything (spec section 6): names that
//! Windows would refuse are adjusted the same way here, two names that become one are kept
//! apart, an item that is already in the save folder arrives under a numbered name, and an
//! Offer whose paths cannot be written is held back, even from a Contact on Auto-accept.
//! The sanitiser's own rules are tested in its module; these go through the Device API.
#![cfg(unix)]

mod support;

use std::path::Path;

use bhayanakshare_core::{
    DeviceId, Error, INCOMING_DIR, TransferId,
    manifest::{Entry, Manifest},
    protocol::{self, Hello, Message, Offer, read_frame, write_frame},
};
use iroh::Endpoint;
use support::{TestDevice, dial_addr, list_dir, raw_peer};

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn read(path: impl AsRef<Path>) -> Vec<u8> {
    std::fs::read(path).unwrap()
}

async fn adjusted_names_recorded(device: &TestDevice, id: TransferId) -> u32 {
    let records = device.device.transfers().await.unwrap();
    records.iter().find(|r| r.id == id).expect("a record of the Transfer").adjusted_names
}

// ---- Names that are not safe ----------------------------------------------------------

#[tokio::test]
async fn names_windows_would_refuse_arrive_adjusted_kept_apart_and_counted() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let src = tempfile::tempdir().unwrap();
    let trip = src.path().join("trip:2024");
    write(&trip.join("README.md"), b"upper");
    write(&trip.join("Readme.md"), b"mixed");
    write(&trip.join("a:b"), b"colon");
    write(&trip.join("a_b"), b"underscore");
    write(&trip.join("end."), b"dot");
    write(&trip.join("why?.txt"), b"question");
    write(&trip.join("fine.txt"), b"fine");
    write(&src.path().join("CON.txt"), b"device");

    let id = alice.device.send(bob.addr(), &[src.path().join("CON.txt"), trip]).await.unwrap();
    // What the Offer sheet shows: the Sender's names as they were, and how many will change.
    let offer = bob.wait_offer().await;
    assert_eq!(offer.items, ["CON.txt", "trip:2024"]);
    // CON.txt, trip:2024, Readme.md (clashes with README.md), a:b, a_b (becomes a_b too),
    // end., why?.txt
    assert_eq!(offer.adjusted_names, 7);
    bob.device.accept(id).await.unwrap();
    let done = bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;

    assert_eq!(list_dir(&bob.save_dir), [INCOMING_DIR, "CON_.txt", "trip_2024"]);
    assert_eq!(read(bob.save_dir.join("CON_.txt")), b"device");
    let trip = bob.save_dir.join("trip_2024");
    assert_eq!(list_dir(&trip), ["README.md", "Readme (1).md", "a_b", "a_b (1)", "end", "fine.txt", "why_.txt"]);
    // Every file kept its own content: nothing was merged or overwritten by a look-alike.
    for (name, bytes) in [
        ("README.md", &b"upper"[..]),
        ("Readme (1).md", b"mixed"),
        ("a_b", b"colon"),
        ("a_b (1)", b"underscore"),
        ("end", b"dot"),
        ("why_.txt", b"question"),
        ("fine.txt", b"fine"),
    ] {
        assert_eq!(read(trip.join(name)), bytes, "{name}");
    }

    // The count is on every event of the Transfer here, and in its record; the Sender, which
    // changed nothing, has none.
    assert_eq!(done.adjusted_names, 7);
    assert_eq!(adjusted_names_recorded(&bob, id).await, 7);
    assert_eq!(adjusted_names_recorded(&alice, id).await, 0);
    assert!(alice.log.iter().all(|e| match &e.kind {
        bhayanakshare_core::EventKind::Transfer(t) => t.adjusted_names == 0,
        _ => true,
    }));
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn names_that_need_no_change_are_not_counted() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let src = tempfile::tempdir().unwrap();
    write(&src.path().join("album/ünï cödé.txt"), b"u");
    write(&src.path().join("album/文件 (2).txt"), b"c");
    write(&src.path().join("album/.hidden"), b"h");

    let id = alice.device.send(bob.addr(), &[src.path().join("album")]).await.unwrap();
    assert_eq!(bob.wait_offer().await.adjusted_names, 0);
    bob.device.accept(id).await.unwrap();
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;

    assert_eq!(list_dir(&bob.save_dir.join("album")), [".hidden", "ünï cödé.txt", "文件 (2).txt"]);
    assert_eq!(adjusted_names_recorded(&bob, id).await, 0);
    alice.shutdown().await;
    bob.shutdown().await;
}

// ---- Clashes with what is already there -----------------------------------------------

#[tokio::test]
async fn what_is_already_in_the_save_folder_survives_and_incoming_items_arrive_renamed() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    write(&bob.save_dir.join("photos/old.jpg"), b"mine");
    write(&bob.save_dir.join("a.txt"), b"mine");
    write(&bob.save_dir.join("a (1).txt"), b"mine too");
    write(&bob.save_dir.join("x_y.txt"), b"mine as well");
    let src = tempfile::tempdir().unwrap();
    write(&src.path().join("photos/new.jpg"), b"theirs");
    write(&src.path().join("photos/old.jpg"), b"theirs, same name");
    write(&src.path().join("a.txt"), b"theirs");
    // Adjusted to a name that is taken: the numbering applies to what it became.
    write(&src.path().join("x:y.txt"), b"theirs, adjusted");

    let paths = ["photos", "a.txt", "x:y.txt"].map(|name| src.path().join(name));
    let id = alice.device.send(bob.addr(), &paths).await.unwrap();
    // A clash with the save folder is not an adjusted name: only x:y.txt changed.
    assert_eq!(bob.wait_offer().await.adjusted_names, 1);
    bob.device.accept(id).await.unwrap();
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;

    assert_eq!(
        list_dir(&bob.save_dir),
        [INCOMING_DIR, "a (1).txt", "a (2).txt", "a.txt", "photos", "photos (1)", "x_y (1).txt", "x_y.txt"]
    );
    // Everything that was there is exactly as it was, and nothing was merged into the folder.
    assert_eq!(read(bob.save_dir.join("a.txt")), b"mine");
    assert_eq!(read(bob.save_dir.join("a (1).txt")), b"mine too");
    assert_eq!(read(bob.save_dir.join("x_y.txt")), b"mine as well");
    assert_eq!(list_dir(&bob.save_dir.join("photos")), ["old.jpg"]);
    assert_eq!(read(bob.save_dir.join("photos/old.jpg")), b"mine");
    // And the incoming items are whole, as units, under the new names.
    assert_eq!(list_dir(&bob.save_dir.join("photos (1)")), ["new.jpg", "old.jpg"]);
    assert_eq!(read(bob.save_dir.join("photos (1)/new.jpg")), b"theirs");
    assert_eq!(read(bob.save_dir.join("photos (1)/old.jpg")), b"theirs, same name");
    assert_eq!(read(bob.save_dir.join("a (2).txt")), b"theirs");
    assert_eq!(read(bob.save_dir.join("x_y (1).txt")), b"theirs, adjusted");
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn an_item_named_like_the_incoming_store_never_lands_in_it_or_replaces_it() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let src = tempfile::tempdir().unwrap();
    write(&src.path().join(INCOMING_DIR).join("f.txt"), b"theirs");
    write(&src.path().join("other/.BHAYANAKSHARE-INCOMING"), b"deeper is fine");

    let paths = [INCOMING_DIR, "other"].map(|name| src.path().join(name));
    let id = alice.device.send(bob.addr(), &paths).await.unwrap();
    assert_eq!(bob.wait_offer().await.adjusted_names, 1);
    bob.device.accept(id).await.unwrap();
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;

    alice.shutdown().await;
    bob.shutdown().await;

    // The incoming store is the Receiver's own and is empty again; the item is beside it.
    assert_eq!(list_dir(&bob.save_dir), [INCOMING_DIR, ".bhayanakshare-incoming (1)", "other"]);
    assert!(list_dir(&bob.save_dir.join(INCOMING_DIR)).is_empty());
    assert_eq!(read(bob.save_dir.join(".bhayanakshare-incoming (1)/f.txt")), b"theirs");
    // Only the top level is the store's place; the same name further down is just a name.
    assert_eq!(read(bob.save_dir.join("other/.BHAYANAKSHARE-INCOMING")), b"deeper is fine");
}

#[tokio::test]
async fn adjusted_names_alone_do_not_hold_auto_accept_back() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    bob.device.add_contact(alice.device.device_id(), None).await.unwrap();
    bob.device.set_auto_accept(alice.device.device_id(), true).await.unwrap();
    let src = tempfile::tempdir().unwrap();
    write(&src.path().join("a:b.txt"), b"adjusted");

    let id = alice.device.send(bob.addr(), &[src.path().join("a:b.txt")]).await.unwrap();
    // No prompt: the Transfer starts out accepted, and carries its count.
    let first = bob.wait_for("the Offer", |t| t.transfer_id == id).await;
    assert_eq!(first.state.label(), "accepted");
    assert_eq!(first.adjusted_names, 1);
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;

    assert_eq!(bob.history(id)[0], "accepted");
    assert_eq!(read(bob.save_dir.join("a_b.txt")), b"adjusted");
    alice.shutdown().await;
    bob.shutdown().await;
}

// ---- Paths that are too long ----------------------------------------------------------

/// A path of `len` bytes made of names of at most 255 bytes.
fn long_path(len: usize) -> String {
    let mut path = String::new();
    while path.len() < len {
        let room = len - path.len();
        if !path.is_empty() {
            path.push('/');
        }
        path.push_str(&"p".repeat(room.saturating_sub(1).min(255)));
    }
    assert_eq!(path.len(), len);
    path
}

/// A Sender written by hand, which has said Hello to `bob` and sent an Offer of one file of
/// `path` that it never serves.
async fn offer_by_a_raw_sender(
    bob: &mut TestDevice,
    peer: &Endpoint,
    id: TransferId,
    path: &str,
) -> (iroh::endpoint::Connection, iroh::endpoint::SendStream, iroh::endpoint::RecvStream) {
    let conn = peer.connect(dial_addr(bob), protocol::ALPN).await.unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, &Message::Hello(Hello::current())).await.unwrap();
    assert!(matches!(read_frame(&mut recv).await.unwrap(), Message::Hello(_)));
    let manifest = Manifest { entries: vec![Entry::file(path, 5)] };
    write_frame(&mut send, &Message::Offer(Offer::new(*id.as_bytes(), manifest, 0))).await.unwrap();
    (conn, send, recv)
}

#[tokio::test]
async fn an_offer_whose_paths_are_too_long_for_the_folder_cannot_be_accepted() {
    let mut bob = TestDevice::start("bob").await;
    let peer = raw_peer().await;
    let elsewhere = tempfile::tempdir().unwrap();

    // Legal for an Offer (under 4096 bytes), but nothing can be written at it: not in the save
    // folder, and not in the one chosen for this Offer either.
    let id = TransferId::from_bytes([1; 16]);
    let (_conn, _send, mut recv) = offer_by_a_raw_sender(&mut bob, &peer, id, &long_path(4090)).await;
    bob.wait_offer().await;
    for folder in [None, Some(elsewhere.path())] {
        let check = bob.device.check_offer(id, folder).await.unwrap();
        assert!(check.paths_too_long, "{folder:?}");
        assert!(check.fits() && !check.passes(), "the space is fine, the paths are not");
        let refused = bob.device.accept_into(id, folder).await.unwrap_err();
        assert!(matches!(refused, Error::PathsTooLong), "{refused:?}");
        assert_eq!(refused.to_string(), "Some paths are too long for this save folder");
    }
    // The Offer still waits, and nothing was made on disk.
    assert_eq!(bob.history(id), ["offered"]);
    assert_eq!(list_dir(&bob.save_dir), Vec::<String>::new());
    assert_eq!(list_dir(elsewhere.path()), Vec::<String>::new());

    bob.device.decline(id).await.unwrap();
    assert!(matches!(read_frame(&mut recv).await.unwrap(), Message::Decline));
    bob.shutdown().await;
}

#[tokio::test]
async fn a_path_that_fits_is_not_held_back() {
    let mut bob = TestDevice::start("bob").await;
    let peer = raw_peer().await;

    // Long, but well within what any save folder leaves room for.
    let id = TransferId::from_bytes([2; 16]);
    let (_conn, _send, mut recv) = offer_by_a_raw_sender(&mut bob, &peer, id, &long_path(500)).await;
    bob.wait_offer().await;
    let check = bob.device.check_offer(id, None).await.unwrap();
    assert!(!check.paths_too_long);
    assert!(check.passes());

    bob.device.accept(id).await.unwrap();
    assert!(matches!(read_frame(&mut recv).await.unwrap(), Message::Accept));
    bob.shutdown().await;
}

#[tokio::test]
async fn too_long_paths_hold_auto_accept_back_and_the_offer_is_prompted() {
    let mut bob = TestDevice::start("bob").await;
    let peer = raw_peer().await;
    let peer_id: DeviceId = data_encoding::BASE32_NOPAD.encode(peer.id().as_bytes()).parse().unwrap();
    bob.device.add_contact(peer_id, None).await.unwrap();
    bob.device.set_auto_accept(peer_id, true).await.unwrap();

    let id = TransferId::from_bytes([3; 16]);
    let (_conn, _send, mut recv) = offer_by_a_raw_sender(&mut bob, &peer, id, &long_path(4090)).await;
    bob.wait_offer().await;

    // A normal prompt, with the warning the sheet shows and Accept refused.
    assert_eq!(bob.history(id), ["offered"]);
    assert!(bob.device.check_offer(id, None).await.unwrap().paths_too_long);
    assert!(matches!(bob.device.accept(id).await, Err(Error::PathsTooLong)));
    assert_eq!(list_dir(&bob.save_dir), Vec::<String>::new());

    bob.device.decline(id).await.unwrap();
    assert!(matches!(read_frame(&mut recv).await.unwrap(), Message::Decline));
    bob.shutdown().await;
}
