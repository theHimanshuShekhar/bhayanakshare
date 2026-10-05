//! What the Receiver checks before it says yes: the Offer must fit in the save folder (and
//! the folder can be chosen per Offer), and the Sender may not send more than it offered.
//! The Receiver's disk is a closure that answers per folder, so no test fills a real one.

mod support;

use std::path::{Path, PathBuf};

use bhayanakshare_core::{
    Error, INCOMING_DIR, SpaceCheck, TransferState,
    protocol::{self, Hello, Message, read_frame, write_frame},
};
use iroh::protocol::Router;
use iroh_blobs::{BlobsProtocol, Hash, format::collection::Collection, store::mem::MemStore};
use support::{TestDevice, dial_addr, list_dir, pseudo_random_bytes, raw_peer};

const SMALL_DISK: u64 = 1024;

/// A Receiver whose default save folder has `SMALL_DISK` bytes free and `roomy` has plenty.
async fn bob_with_a_small_disk(roomy: &Path) -> TestDevice {
    let roomy = roomy.to_owned();
    TestDevice::start_with_free_space("bob", move |dir: &Path| {
        Ok(if dir == roomy { 1 << 40 } else { SMALL_DISK })
    })
    .await
}

fn write_source(name: &str, bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
    let src = tempfile::tempdir().unwrap();
    let path = src.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    (src, path)
}

#[tokio::test]
async fn an_offer_larger_than_the_free_space_cannot_be_accepted() {
    let roomy = tempfile::tempdir().unwrap();
    let mut alice = TestDevice::start("alice").await;
    let mut bob = bob_with_a_small_disk(roomy.path()).await;
    let (_src, path) = write_source("big.bin", &pseudo_random_bytes(4096, 1));

    let id = alice.device.send_file(bob.addr(), &path).await.unwrap();
    bob.wait_offer().await;

    let check = bob.device.check_offer(id, None).await.unwrap();
    assert_eq!(check, SpaceCheck { needed: 4096, free: Some(SMALL_DISK), paths_too_long: false });
    assert!(!check.fits());
    let refused = bob.device.accept(id).await.unwrap_err();
    assert!(
        matches!(refused, Error::NotEnoughSpace { needed: 4096, free: SMALL_DISK }),
        "{refused:?}"
    );
    // Nothing moved, and the Offer still waits for an answer.
    assert_eq!(bob.history(id), ["offered"]);
    assert_eq!(list_dir(&bob.save_dir), Vec::<String>::new());

    bob.device.decline(id).await.unwrap();
    alice.wait_state(id, "declined").await;
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn the_checks_run_again_for_another_folder_and_the_offer_goes_there() {
    let roomy = tempfile::tempdir().unwrap();
    let mut alice = TestDevice::start("alice").await;
    let mut bob = bob_with_a_small_disk(roomy.path()).await;
    let bytes = pseudo_random_bytes(4096, 2);
    let (_src, path) = write_source("big.bin", &bytes);

    let id = alice.device.send_file(bob.addr(), &path).await.unwrap();
    bob.wait_offer().await;
    assert!(!bob.device.check_offer(id, None).await.unwrap().fits());
    let elsewhere = bob.device.check_offer(id, Some(roomy.path())).await.unwrap();
    assert_eq!(elsewhere, SpaceCheck { needed: 4096, free: Some(1 << 40), paths_too_long: false });
    assert!(elsewhere.fits());

    // The folder applies to this Offer only, and the incoming store is made inside it.
    bob.device.accept_into(id, Some(roomy.path())).await.unwrap();
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;
    alice.shutdown().await;
    bob.shutdown().await;

    assert_eq!(std::fs::read(roomy.path().join("big.bin")).unwrap(), bytes);
    assert_eq!(list_dir(roomy.path()), [INCOMING_DIR, "big.bin"]);
    assert_eq!(list_dir(&bob.save_dir), Vec::<String>::new(), "the default folder is untouched");
    let TransferState::Completed { saved_to: Some(saved) } = bob.device.transfers().await.unwrap()[0].state.clone()
    else {
        panic!("not completed")
    };
    assert_eq!(Path::new(&saved), roomy.path().join("big.bin"));
}

#[tokio::test]
async fn a_folder_that_does_not_exist_is_refused() {
    let roomy = tempfile::tempdir().unwrap();
    let mut alice = TestDevice::start("alice").await;
    let mut bob = bob_with_a_small_disk(roomy.path()).await;
    let (_src, path) = write_source("a.txt", b"hi");

    let id = alice.device.send_file(bob.addr(), &path).await.unwrap();
    bob.wait_offer().await;
    let missing = roomy.path().join("missing");
    assert!(matches!(
        bob.device.check_offer(id, Some(&missing)).await,
        Err(Error::NotAFolder(_))
    ));
    assert!(matches!(
        bob.device.accept_into(id, Some(&missing)).await,
        Err(Error::NotAFolder(_))
    ));
    // The Offer is still there to be answered.
    bob.device.accept(id).await.unwrap();
    bob.wait_state(id, "completed").await;
    alice.shutdown().await;
    bob.shutdown().await;
}

/// A Sender written by hand that offers 512 bytes and then serves 16 MiB.
#[tokio::test]
async fn a_sender_that_sends_more_than_it_offered_is_cut_off() {
    let roomy = tempfile::tempdir().unwrap();
    let mut bob = bob_with_a_small_disk(roomy.path()).await;

    // What it really serves: much more than the Offer says, from an in-memory store.
    let actual = pseudo_random_bytes(16 * 1024 * 1024, 3);
    let store = MemStore::new();
    let file = store.blobs().add_bytes(actual).temp_tag().await.unwrap();
    let collection = Collection::from_iter([("big.bin".to_owned(), file.hash())]);
    let root: Hash = collection.store(&store).await.unwrap().hash();
    let endpoint = raw_peer().await;
    let _provider = Router::builder(endpoint.clone())
        .accept(iroh_blobs::ALPN, BlobsProtocol::new(&store, None))
        .spawn();

    let conn = endpoint.connect(dial_addr(&bob), protocol::ALPN).await.unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, &Message::Hello(Hello::current())).await.unwrap();
    assert!(matches!(read_frame(&mut recv).await.unwrap(), Message::Hello(_)));
    let id = protocol_id(7);
    write_frame(
        &mut send,
        &Message::Offer(support::one_file_offer(*id.as_bytes(), "big.bin", 512)),
    )
    .await
    .unwrap();

    // 512 bytes fit the small disk, so Bob can accept.
    bob.wait_offer().await;
    bob.device.accept_into(id, Some(roomy.path())).await.unwrap();
    assert!(matches!(read_frame(&mut recv).await.unwrap(), Message::Accept));
    write_frame(&mut send, &Message::HashReady { collection_hash: *root.as_bytes() })
        .await
        .unwrap();

    // Bob stops as soon as more than the Offer's size has arrived: the file is not downloaded
    // in full only to be found the wrong size afterwards.
    let failed = bob.wait_state(id, "failed").await;
    let TransferState::Failed { reason } = failed.state else { unreachable!() };
    assert!(reason.contains("more than it offered"), "{reason}");
    bob.shutdown().await;
    assert_eq!(list_dir(roomy.path()), [INCOMING_DIR]);
    assert_eq!(list_dir(&roomy.path().join(INCOMING_DIR)), Vec::<String>::new());
}

fn protocol_id(n: u8) -> bhayanakshare_core::TransferId {
    bhayanakshare_core::TransferId::from_bytes([n; 16])
}
