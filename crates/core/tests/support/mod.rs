//! The in-process multi-Device harness. Every integration test starts its Devices here:
//! each gets its own temp data and save folders, a manual clock, and a localhost-only
//! endpoint (relays and discovery off), so Devices reach each other by the addresses the
//! test hands over.

#![allow(dead_code)] // each test binary uses a different subset

pub mod multicast;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use bhayanakshare_core::{
    Clock, Device, DeviceAddr, DeviceConfig, Event, EventKind, EventStream, FreeSpace, KeySource,
    ManualClock, NearbyDevice, Network, ProgressEvent, SystemFreeSpace, TransferEvent, TransferId,
    TransferState, VersionMismatchEvent,
    manifest::{Entry, Manifest},
    protocol::{Offer, OfferKind},
};
use iroh::{Endpoint, EndpointAddr, RelayMode, TransportAddr, endpoint::presets};
use tempfile::TempDir;

/// How long a test waits for any single event before failing. It is the limit for a hang, not
/// a speed: the 256 MB Transfers of the resume and crash tests take up to a minute on a Windows
/// runner, where they take a few seconds on Linux.
const EVENT_TIMEOUT: Duration = Duration::from_secs(if cfg!(windows) { 150 } else { 30 });

/// How long a test lets a Device take to shut down.
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(30);

pub struct TestDevice {
    pub name: String,
    pub device: Device,
    pub clock: Arc<ManualClock>,
    pub data_dir: PathBuf,
    pub save_dir: PathBuf,
    network: Network,
    free_space: Arc<dyn FreeSpace>,
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

    /// Like `start`, but the Device has no direct paths: it is reached only through the relay
    /// at `url`, which a test runs itself.
    pub async fn start_on_relay(name: &str, url: &'static str) -> Self {
        Self::start_on(name, Network::Relay(url), SystemFreeSpace).await
    }

    async fn start_on(name: &str, network: Network, free_space: impl FreeSpace) -> Self {
        Self::build(name, tempfile::tempdir().unwrap(), network, free_space).await
    }

    /// Starts a Device in `tmp` (`data` and `save` folders inside), which may already hold the
    /// folders of one that ran before, as after a crash.
    pub async fn start_in(name: &str, tmp: TempDir, free_space: impl FreeSpace) -> Self {
        Self::build(name, tmp, Network::Localhost, free_space).await
    }

    async fn build(name: &str, tmp: TempDir, network: Network, free_space: impl FreeSpace) -> Self {
        let data_dir = tmp.path().join("data");
        let save_dir = tmp.path().join("save");
        std::fs::create_dir_all(&save_dir).unwrap();
        let clock = Arc::new(ManualClock::new(1_000_000));
        let free_space: Arc<dyn FreeSpace> = Arc::new(free_space);
        let config = config(&data_dir, &save_dir, clock.clone(), network, free_space.clone());
        let (device, events) = Device::start(config).await.unwrap();
        Self {
            name: name.to_owned(),
            device,
            clock,
            data_dir,
            save_dir,
            network,
            free_space,
            events,
            log: Vec::new(),
            consumed: Vec::new(),
            _tmp: tmp,
        }
    }

    /// Stops the Device cleanly and starts another on the same folders and clock, as when the
    /// app is quit and opened again. Its events join `log` after the old ones.
    pub async fn restart(&mut self) {
        self.shutdown().await;
        let clock = self.clock.clone();
        let config = config(&self.data_dir, &self.save_dir, clock, self.network, self.free_space.clone());
        let (device, events) = Device::start(config).await.unwrap();
        self.device = device;
        self.events = events;
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

    /// Waits for the Device to report a refused connection for the versions of BhayanakShare.
    pub async fn wait_version_mismatch(&mut self) -> VersionMismatchEvent {
        loop {
            let seen = self.log.iter().find_map(|e| match &e.kind {
                EventKind::VersionMismatch(m) => Some(m.clone()),
                _ => None,
            });
            if let Some(mismatch) = seen {
                return mismatch;
            }
            self.read_next("a version mismatch").await;
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

    /// Waits until this Device has reported receiving at least `bytes` of the Transfer, and
    /// returns that report. Progress reports are paced by the Device's clock, which a test
    /// holds still, so this moves the clock along while it waits.
    pub async fn wait_progress(&mut self, id: TransferId, bytes: u64) -> ProgressEvent {
        let give_up = tokio::time::Instant::now() + EVENT_TIMEOUT;
        loop {
            let reached = self.log.iter().find_map(|e| match &e.kind {
                EventKind::Progress(p) if p.transfer_id == id && p.bytes >= bytes => Some(p.clone()),
                _ => None,
            });
            if let Some(progress) = reached {
                return progress;
            }
            assert!(
                tokio::time::Instant::now() < give_up,
                "{}: timed out waiting for {bytes} bytes of {id}; events so far:\n{:#?}",
                self.name,
                self.log
            );
            self.clock.advance(150);
            if let Ok(next) = tokio::time::timeout(Duration::from_millis(20), self.events.next()).await {
                let event = next.unwrap_or_else(|| panic!("{}: event stream ended", self.name));
                self.log.push(event);
                self.consumed.push(false);
            }
        }
    }

    /// The Transfer's progress reports on this Device so far, in order.
    pub fn progress(&self, id: TransferId) -> Vec<ProgressEvent> {
        self.log
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::Progress(p) if p.transfer_id == id => Some(p.clone()),
                _ => None,
            })
            .collect()
    }

    /// The state labels a Transfer went through on this Device, in order.
    pub fn history(&self, id: TransferId) -> Vec<&'static str> {
        self.log
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::Transfer(t) => Some(t),
                EventKind::Progress(_)
                | EventKind::Preparing(_)
                | EventKind::Nearby(_)
                | EventKind::VersionMismatch(_) => None,
            })
            .filter(|t| t.transfer_id == id)
            .map(|t| t.state.label())
            .collect()
    }

    /// Whether the Transfer is Preparing, as each of its reports on this Device said, in order.
    pub fn preparing(&self, id: TransferId) -> Vec<bool> {
        self.log
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::Preparing(p) if p.transfer_id == id => Some(p.preparing),
                _ => None,
            })
            .collect()
    }

    /// Waits until this Device has reported the Transfer as Preparing (or no longer).
    pub async fn wait_preparing(&mut self, id: TransferId, preparing: bool) {
        while !self.preparing(id).contains(&preparing) {
            self.read_next(&format!("{id} preparing: {preparing}")).await;
        }
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
        self.device.shutdown(SHUTDOWN_DEADLINE).await;
        while let Some(event) = self.events.try_next() {
            self.log.push(event);
            self.consumed.push(false);
        }
    }
}

/// The configuration of a Device that lives in `data_dir` and saves to `save_dir`.
pub fn config(
    data_dir: &Path,
    save_dir: &Path,
    clock: Arc<dyn Clock>,
    network: Network,
    free_space: Arc<dyn FreeSpace>,
) -> DeviceConfig {
    DeviceConfig {
        key_source: KeySource::File(data_dir.join("secret.key")),
        data_dir: data_dir.to_owned(),
        save_dir: save_dir.to_owned(),
        clock,
        network,
        free_space,
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

/// An Offer of one file, as a hand-written Sender makes it.
pub fn one_file_offer(transfer_id: [u8; 16], name: &str, size: u64) -> Offer {
    Offer::new(transfer_id, Manifest { entries: vec![Entry::file(name, size)] }, 0)
}

/// The manifest of an Offer of files.
pub fn manifest_of(offer: &Offer) -> &Manifest {
    match &offer.kind {
        OfferKind::Files(manifest) => manifest,
        OfferKind::Text(_) => panic!("expected an Offer of files, got text"),
    }
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
