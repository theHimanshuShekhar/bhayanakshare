//! Restart behaviour and hostile or broken peers, still through the public surface.

mod support;

use std::time::Duration;

use bhayanakshare_core::{
    Device, DeviceConfig, KeySource, ManualClock, Network, SystemFreeSpace, TransferState,
    protocol::{self, FrameError, Hello, Message, Offer, read_frame, write_frame},
};
use support::{TestDevice, dial_addr, list_dir, raw_peer};

#[tokio::test]
async fn identity_settings_and_history_survive_a_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let config = || DeviceConfig {
        data_dir: tmp.path().join("data"),
        save_dir: tmp.path().join("save"),
        key_source: KeySource::File(tmp.path().join("data").join("secret.key")),
        clock: std::sync::Arc::new(ManualClock::new(42)),
        network: Network::Localhost,
        free_space: std::sync::Arc::new(SystemFreeSpace),
    };

    let (first, _events) = Device::start(config()).await.unwrap();
    let id = first.device_id();
    first.set_setting("device_name", "Laptop").await.unwrap();
    first.shutdown().await;
    drop(first);

    // Same data folder: same key, same database, and the Sender's store reopens cleanly.
    let (second, _events) = Device::start(config()).await.unwrap();
    assert_eq!(second.device_id(), id);
    assert_eq!(second.setting("device_name").await.unwrap().as_deref(), Some("Laptop"));
    second.shutdown().await;
}

#[tokio::test]
async fn two_live_devices_cannot_share_a_data_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let config = || DeviceConfig {
        data_dir: tmp.path().join("data"),
        save_dir: tmp.path().join("save"),
        key_source: KeySource::File(tmp.path().join("secret.key")),
        clock: std::sync::Arc::new(ManualClock::new(0)),
        network: Network::Localhost,
        free_space: std::sync::Arc::new(SystemFreeSpace),
    };
    let (first, _events) = Device::start(config()).await.unwrap();
    // The store registry refuses the second open instead of hanging inside iroh-blobs.
    let second = tokio::time::timeout(Duration::from_secs(10), Device::start(config()))
        .await
        .expect("second start must fail fast, not hang");
    assert!(second.is_err());
    first.shutdown().await;
}

#[tokio::test]
async fn an_offer_with_a_path_in_its_name_is_refused_and_nothing_is_written() {
    let mut bob = TestDevice::start("bob").await;
    let peer = raw_peer().await;
    let conn = peer.connect(dial_addr(&bob), protocol::ALPN).await.unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();

    write_frame(&mut send, &Message::Hello(Hello::current())).await.unwrap();
    assert!(matches!(read_frame(&mut recv).await.unwrap(), Message::Hello(_)));
    write_frame(
        &mut send,
        &Message::Offer(Offer { transfer_id: [5; 16], name: "../evil.txt".into(), size: 4 }),
    )
    .await
    .unwrap();

    // Bob hangs up without ever showing the Offer.
    let reply = tokio::time::timeout(Duration::from_secs(10), read_frame(&mut recv)).await;
    assert!(matches!(reply, Ok(Err(FrameError::Closed | FrameError::Io(_)))), "{reply:?}");
    bob.shutdown().await;
    assert!(bob.log.is_empty(), "an invalid Offer must not surface: {:?}", bob.log);
    assert!(bob.device.transfers().await.unwrap().is_empty());
    assert!(!bob.save_dir.parent().unwrap().join("evil.txt").exists());
    assert_eq!(list_dir(&bob.save_dir), Vec::<String>::new());
}

#[tokio::test]
async fn a_peer_on_another_protocol_version_is_refused_before_any_offer() {
    let mut bob = TestDevice::start("bob").await;
    let peer = raw_peer().await;
    let conn = peer.connect(dial_addr(&bob), protocol::ALPN).await.unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();

    let hello = Hello { protocol_version: protocol::PROTOCOL_VERSION + 1, app_version: "9.9.9".into() };
    write_frame(&mut send, &Message::Hello(hello)).await.unwrap();
    write_frame(
        &mut send,
        &Message::Offer(Offer { transfer_id: [6; 16], name: "ok.txt".into(), size: 1 }),
    )
    .await
    .unwrap();

    // Bob says Hello, then drops the stream rather than reading the Offer.
    let mut closed = false;
    for _ in 0..2 {
        match tokio::time::timeout(Duration::from_secs(10), read_frame(&mut recv)).await {
            Ok(Ok(Message::Hello(_))) => {}
            Ok(Err(_)) => closed = true,
            other => panic!("unexpected reply {other:?}"),
        }
    }
    assert!(closed);
    bob.shutdown().await;
    assert!(bob.log.is_empty(), "{:?}", bob.log);
}

#[tokio::test]
async fn a_sender_that_disappears_before_the_decision_fails_the_offer() {
    let mut bob = TestDevice::start("bob").await;
    let peer = raw_peer().await;
    let conn = peer.connect(dial_addr(&bob), protocol::ALPN).await.unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, &Message::Hello(Hello::current())).await.unwrap();
    read_frame(&mut recv).await.unwrap();
    write_frame(
        &mut send,
        &Message::Offer(Offer { transfer_id: [7; 16], name: "gone.txt".into(), size: 10 }),
    )
    .await
    .unwrap();

    let offer = bob.wait_offer().await;
    conn.close(0u32.into(), b"changed my mind");
    let failed = bob.wait_state(offer.transfer_id, "failed").await;
    assert!(matches!(failed.state, TransferState::Failed { .. }));
    // The pending Offer is gone, so a late Accept is rejected instead of starting anything.
    assert!(bob.device.accept(offer.transfer_id).await.is_err());
    bob.shutdown().await;
    assert_eq!(list_dir(&bob.save_dir), Vec::<String>::new());
}
