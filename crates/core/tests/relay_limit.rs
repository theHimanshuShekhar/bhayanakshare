//! On demand, not part of `cargo test`: a Transfer through a relay that rate-limits its clients
//! slows down but completes (spec section 3; research/n0-infrastructure.md, section 2).
//!
//! The Devices have no direct paths at all (`Network::Relay`), so every byte goes through a
//! self-hosted `iroh-relay` run here with a low limit on what it reads from each client. Run:
//!
//! ```sh
//! cargo test -p bhayanakshare-core --features relay-tests --test relay_limit -- --nocapture
//! ```

mod support;

use std::{
    net::Ipv4Addr,
    num::NonZeroU32,
    time::{Duration, Instant},
};

use iroh_relay::server::{ClientRateLimit, RelayConfig, Server, ServerConfig};
use support::{TestDevice, pseudo_random_bytes};

/// What the relay reads from each client per second.
const RATE: u32 = 200_000;
const SIZE: usize = 1_500_000;

#[tokio::test]
async fn a_transfer_through_a_rate_limited_relay_slows_down_but_completes() {
    let mut relay = RelayConfig::new((Ipv4Addr::LOCALHOST, 0));
    relay.limits.client_rx = Some(ClientRateLimit::new(NonZeroU32::new(RATE).unwrap()));
    let mut config = ServerConfig::default();
    config.relay = Some(relay);
    let server = Server::spawn(config).await.unwrap();
    // The Devices keep the URL for as long as they run, which is the rest of the process.
    let url: &'static str = Box::leak(format!("http://{}", server.http_addr().unwrap()).into_boxed_str());

    let mut alice = TestDevice::start_on_relay("alice", url).await;
    let mut bob = TestDevice::start_on_relay("bob", url).await;
    assert!(alice.addr().direct.is_empty(), "no direct paths in this test");
    // A Device has a relay URL to be dialled by once it has connected to its relay.
    let bob_addr = loop {
        let addr = bob.addr();
        if addr.relay_url.is_some() {
            break addr;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let bytes = pseudo_random_bytes(SIZE, 21);
    let src = tempfile::tempdir().unwrap();
    let path = src.path().join("big.bin");
    std::fs::write(&path, &bytes).unwrap();

    let started = Instant::now();
    let id = alice.device.send_file(bob_addr, &path).await.unwrap();
    bob.wait_offer().await;
    bob.device.accept(id).await.unwrap();
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;
    let took = started.elapsed();
    eprintln!("{SIZE} bytes through a relay limited to {RATE} B/s took {took:?}");

    assert_eq!(std::fs::read(bob.save_dir.join("big.bin")).unwrap(), bytes);
    // At the limit it takes SIZE / RATE = 7.5 s; unthrottled on this machine it takes well
    // under one. Half the limit's time is slack for the burst the bucket allows.
    assert!(took.as_secs_f64() > SIZE as f64 / f64::from(RATE) / 2.0, "not slowed: {took:?}");
    alice.shutdown().await;
    bob.shutdown().await;
    server.shutdown().await.unwrap();
}
