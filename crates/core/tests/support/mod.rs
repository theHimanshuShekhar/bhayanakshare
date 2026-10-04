//! The in-process multi-Device harness. Every integration test starts its Devices here:
//! each gets its own temp data and save folders, a manual clock, and a localhost-only
//! endpoint (relays and discovery off), so Devices reach each other by the addresses the
//! test hands over.

#![allow(dead_code)] // each test binary uses a different subset

use std::{path::PathBuf, sync::Arc, time::Duration};

use bhayanakshare_core::{
    Device, DeviceAddr, DeviceConfig, Event, EventKind, EventStream, FreeSpace, KeySource,
    ManualClock, NearbyDevice, Network, SystemFreeSpace, TransferEvent, TransferId, TransferState,
};
use iroh::{Endpoint, EndpointAddr, RelayMode, TransportAddr, endpoint::presets};
use tempfile::TempDir;

/// How long a test waits for any single event before failing.
const EVENT_TIMEOUT: Duration = Duration::from_secs(30);

pub struct TestDevice {
    pub name: String,
    pub device: Device,
    pub clock: Arc<ManualClock>,
    pub data_dir: PathBuf,
    pub save_dir: PathBuf,
    events: EventStream,
    /// Every event seen so far, in stream order.
    pub log: Vec<Event>,
    /// Parallel to `log`: whether a `wait_for` has already returned that event.
    consumed: Vec<bool>,
    _tmp: TempDir,
}

impl TestDevice {
    pub async fn start(name: &str) -> Self {
        Self::start_with_free_space(name, SystemFreeSpace).await
    }

    /// Like `start`, but the Device sees `free_space` instead of the real disk.
    pub async fn start_with_free_space(name: &str, free_space: impl FreeSpace) -> Self {
        Self::start_on(name, Network::Localhost, free_space).await
    }

    /// Like `start`, but the Device also finds and announces itself over multicast on the
    /// loopback interface, so a test can see Devices discover each other.
    pub async fn start_discovering(name: &str) -> Self {
        Self::start_on(name, Network::LocalhostLan, SystemFreeSpace).await
    }

    async fn start_on(name: &str, network: Network, free_space: impl FreeSpace) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("data");
        let save_dir = tmp.path().join("save");
        std::fs::create_dir_all(&save_dir).unwrap();
        let clock = Arc::new(ManualClock::new(1_000_000));
        let (device, events) = Device::start(DeviceConfig {
            key_source: KeySource::File(data_dir.join("secret.key")),
            data_dir: data_dir.clone(),
            save_dir: save_dir.clone(),
            clock: clock.clone(),
            network,
            free_space: Arc::new(free_space),
        })
        .await
        .unwrap();
        Self {
            name: name.to_owned(),
            device,
            clock,
            data_dir,
            save_dir,
            events,
            log: Vec::new(),
            consumed: Vec::new(),
            _tmp: tmp,
        }
    }

    /// Where other Devices dial this one: its Device ID plus its localhost addresses.
    pub fn addr(&self) -> DeviceAddr {
        self.device.addr()
    }

    /// Waits for the first Transfer event matching `pred` that no earlier wait has returned.
    /// Events that do not match stay available to later waits.
    pub async fn wait_for(
        &mut self,
        what: &str,
        pred: impl Fn(&TransferEvent) -> bool,
    ) -> TransferEvent {
        loop {
            for (i, event) in self.log.iter().enumerate() {
                let EventKind::Transfer(t) = &event.kind else { continue };
                if !self.consumed[i] && pred(t) {
                    self.consumed[i] = true;
                    return t.clone();
                }
            }
            self.read_next(what).await;
        }
    }

    /// Waits until the list of Nearby Devices satisfies `pred` and returns it. Each `Nearby`
    /// event holds the whole list, so this looks at the latest one (an empty list before the
    /// first), not at events no earlier wait has used.
    pub async fn wait_nearby(
        &mut self,
        what: &str,
        pred: impl Fn(&[NearbyDevice]) -> bool,
    ) -> Vec<NearbyDevice> {
        loop {
            let latest = self.log.iter().rev().find_map(|e| match &e.kind {
                EventKind::Nearby(n) => Some(n.devices.clone()),
                _ => None,
            });
            let latest = latest.unwrap_or_default();
            if pred(&latest) {
                return latest;
            }
            self.read_next(what).await;
        }
    }

    /// Reads the next event into the log, or fails the test if none comes in time.
    async fn read_next(&mut self, what: &str) {
        match tokio::time::timeout(EVENT_TIMEOUT, self.events.next()).await {
            Ok(Some(event)) => {
                self.log.push(event);
                self.consumed.push(false);
            }
            Ok(None) => panic!("{}: event stream ended waiting for {what}", self.name),
            Err(_) => panic!(
                "{}: timed out waiting for {what}; events so far:\n{:#?}",
                self.name, self.log
            ),
        }
    }

    /// Waits for a Transfer to reach the state whose label is `label`.
    pub async fn wait_state(&mut self, id: TransferId, label: &str) -> TransferEvent {
        self.wait_for(&format!("{id} -> {label}"), |t| {
            t.transfer_id == id && t.state.label() == label
        })
        .await
    }

    /// Waits for an incoming Offer and returns it.
    pub async fn wait_offer(&mut self) -> TransferEvent {
        self.wait_for("an incoming Offer", |t| {
            t.state == TransferState::Offered && t.role == bhayanakshare_core::Role::Receiver
        })
        .await
    }

    /// The state labels a Transfer went through on this Device, in order.
    pub fn history(&self, id: TransferId) -> Vec<&'static str> {
        self.log
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::Transfer(t) => Some(t),
                EventKind::Progress(_) | EventKind::Nearby(_) => None,
            })
            .filter(|t| t.transfer_id == id)
            .map(|t| t.state.label())
            .collect()
    }

    /// Keeps reading events for `real_time`, so a test can then assert that something did
    /// not happen (the injected clock does not move on its own, so only real time passes).
    pub async fn quiet_for(&mut self, real_time: Duration) {
        let until = tokio::time::Instant::now() + real_time;
        while let Ok(Some(event)) = tokio::time::timeout_at(until, self.events.next()).await {
            self.log.push(event);
            self.consumed.push(false);
        }
    }

    /// Shuts the Device down, then reads the events already queued so `log` is complete.
    pub async fn shutdown(&mut self) {
        self.device.shutdown().await;
        while let Some(event) = self.events.try_next() {
            self.log.push(event);
            self.consumed.push(false);
        }
    }
}

/// Deterministic, incompressible-looking bytes (xorshift), so tests need no RNG.
pub fn pseudo_random_bytes(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        out.extend_from_slice(&x.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes());
    }
    out.truncate(len);
    out
}

/// Names in a directory, sorted.
pub fn list_dir(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// A bare QUIC peer that speaks (or breaks) the control protocol by hand.
pub async fn raw_peer() -> Endpoint {
    Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_ip_transports()
        .bind_addr("127.0.0.1:0")
        .unwrap()
        .bind()
        .await
        .unwrap()
}

pub fn dial_addr(target: &TestDevice) -> EndpointAddr {
    let addr = target.addr();
    EndpointAddr::from_parts(
        iroh::EndpointId::from_bytes(addr.id.as_bytes()).unwrap(),
        addr.direct.into_iter().map(TransportAddr::Ip),
    )
}
