//! LAN discovery (spec section 3): finding Nearby Devices, and being found, with
//! `swarm-discovery` under our own service name (never iroh's `irohv1`, so other iroh apps on
//! the network neither show up here nor see us).
//!
//! One `Discoverer` runs at a time, except while Hidden. It listens and announces as the
//! [`Visibility`] setting says, and is changed while it runs:
//!
//! - **Everyone**: a plain announcement whose instance label is the Device ID and whose TXT
//!   carries the Device Name.
//! - **People who have my ID**: the blinded beacon of [`crate::beacon`], whose label and sealed
//!   TXT only a Device holding this Device's ID can recognise and open.
//! - **Hidden**: no `Discoverer` at all, because `swarm-discovery` cannot listen without also
//!   multicasting a query for the service every second or so, and a Hidden Device sends
//!   nothing. So it lists no Nearby Devices either. Instead the [`Responder`] answers a query
//!   that carries this Device's own blinded label, which only a Device holding its ID can ask,
//!   and [`LanLookup`] is how a Device asks: iroh consults it when dialling an ID, so it only
//!   sends when the user sends to an ID.
//!
//! [`announcement`] is where a setting chooses what to say and [`hear`] is where a heard label
//! becomes a Device. swarm-discovery fixes the label when it starts, so a different label (a
//! new beacon epoch, or a change between plain and beacon) means a new `Discoverer`.
//! swarm-discovery says no goodbye when a Device stops announcing or is dropped: the others
//! drop it from their lists when they have not heard it for three of its turns, a few seconds.
//!
//! Discovery may not be able to start: the mDNS port (UDP 5353) is held by a program that does
//! not share it or is refused, or no network interface can join the group. The Device works
//! without it, and says so in its [`DiscoveryStatus`], reported on the event stream when it
//! changes and readable through [`Discovery::status`], so a UI that reloaded is right too. It
//! covers what has to run: the `Discoverer`, or the responder while Hidden. A [`LanLookup`] that
//! cannot bind when sending to an ID fails that one dial and is not a status. A start that failed
//! is recovered, not left for a restart: [`Discovery::refresh`] tries it again (as it does on every
//! new epoch, setting change or address change) and so does a tick every [`DISCOVERY_CHECK`], which
//! also starts a responder that has stopped. The status goes back to working as soon as a try
//! binds.
//!
//! What is heard is kept as a table of Nearby Devices, reported on the event stream whenever it
//! changes, and its addresses are handed to iroh, so dialling a Nearby Device by its ID alone
//! reaches it. `UserData` is never set on the endpoint: the Device Name stays on the LAN and
//! is not published to n0.
//!
//! A plain announcement is unauthenticated, so its name is only shown (cleaned, with the
//! Fingerprint next to it) and never stored in a Contact. A beacon can only be made by a Device
//! holding the ID, so the name in one does refresh the Contact's stored Device Name (as `Hello`,
//! over an authenticated connection, also does), while a Nickname still takes priority. Addresses
//! are only used to dial, where iroh checks the key.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fmt, io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use iroh::{Endpoint, EndpointAddr, TransportAddr, Watcher, address_lookup::MemoryLookup};
use serde::{Deserialize, Serialize};
use swarm_discovery::{Discoverer, DropGuard};
use tokio::sync::mpsc;

use crate::{
    beacon::{self, Index},
    clock,
    contacts::KnownAddress,
    db::{Db, DbError},
    device::{Network, Shared, direct_addrs},
    device_name,
    event::{DiscoveryStatusEvent, EventKind, NearbyEvent},
    identity::DeviceId,
    responder::{self, LanLookup, Responder},
};

/// Our mDNS service name (`_bhayanakshare._udp.local.`). At most 15 characters (RFC 6335).
pub(crate) const SERVICE_NAME: &str = "bhayanakshare";

/// The TXT attribute carrying the Device Name.
const NAME_KEY: &str = "name";

/// A TXT attribute holds at most 254 bytes, key included.
const MAX_NAME_BYTES: usize = 250;

/// How many addresses an announcement lists. Receivers drop packets over 1472 bytes, which
/// a machine with many virtual interfaces could otherwise exceed.
const MAX_ANNOUNCED_ADDRS: usize = 12;

/// How many Nearby Devices are kept. Anyone on the LAN can announce any number of them.
const MAX_NEARBY: usize = 256;

/// Who can see a Device as a Nearby Device (spec section 3). Governs discovery only: anyone
/// holding the Device ID can still send to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    /// A plain announcement with the Device Name: anyone on the LAN sees it.
    Everyone,
    /// Only Devices that hold this Device's ID: a blinded beacon only they can recognise.
    #[default]
    IdHolders,
    /// Nobody: announces nothing and looks for nobody, and only answers a Device that asks for
    /// it by its ID.
    Hidden,
}

impl Visibility {
    /// The setting the choice is stored under.
    const SETTING: &'static str = "visibility";

    fn as_setting(self) -> &'static str {
        match self {
            Self::Everyone => "everyone",
            Self::IdHolders => "id_holders",
            Self::Hidden => "hidden",
        }
    }

    fn from_setting(value: &str) -> Option<Self> {
        [Self::Everyone, Self::IdHolders, Self::Hidden]
            .into_iter()
            .find(|v| v.as_setting() == value)
    }

    /// The stored choice, else the default. A setting that cannot be read also gives the
    /// default, which announces nothing.
    pub(crate) async fn load(db: &Db) -> Self {
        match db.setting(Self::SETTING).await {
            Ok(Some(value)) => Self::from_setting(&value).unwrap_or_default(),
            Ok(None) => Self::default(),
            Err(e) => {
                tracing::warn!("could not read the Visibility: {e}");
                Self::default()
            }
        }
    }

    pub(crate) async fn store(self, db: &Db) -> Result<(), DbError> {
        db.set_setting(Self::SETTING, self.as_setting()).await
    }
}

/// A Device found on the LAN.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct NearbyDevice {
    pub id: DeviceId,
    /// What the Device calls itself, as it announced. Untrusted text: show it next to the
    /// Fingerprint. Absent if it announced none.
    pub name: Option<String>,
}

/// Whether LAN discovery is running, as the user needs to know it. While Hidden it is the
/// responder that has to run, since there is no `Discoverer`. A failing lookup when sending to
/// an ID ([`LanLookup`]) is not a status: it happens per dial, and a Device that is not there is
/// the usual reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DiscoveryStatus {
    Working,
    /// Discovery could not start, and is tried again every [`DISCOVERY_CHECK`].
    Unavailable { reason: UnavailableReason },
}

/// Why LAN discovery could not start, in the terms the UI words differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum UnavailableReason {
    /// The mDNS port (UDP 5353) could not be bound: another program holds it without sharing it,
    /// or the system or a firewall refused.
    PortInUse,
    /// The mDNS group could not be joined on any network interface: none is up, or none supports
    /// multicast.
    NoInterface,
    Other,
}

/// The mDNS port could not be used, and why.
#[derive(Debug)]
pub(crate) struct MdnsError {
    pub(crate) reason: UnavailableReason,
    source: io::Error,
}

impl MdnsError {
    pub(crate) fn new(reason: UnavailableReason, source: io::Error) -> Self {
        Self { reason, source }
    }
}

impl fmt::Display for MdnsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.source.fmt(f)
    }
}

impl std::error::Error for MdnsError {}

impl From<MdnsError> for io::Error {
    fn from(e: MdnsError) -> Self {
        e.source
    }
}

/// What this Device says on the LAN.
#[derive(Debug, PartialEq, Eq)]
struct Announcement {
    /// The instance label to announce under.
    label: String,
    /// The addresses to announce, by port.
    ports: Vec<(u16, Vec<IpAddr>)>,
    txt: Vec<(String, String)>,
}

/// What this Device announces under `visibility`, or `None` to stay silent. `addrs` are the
/// addresses another Device could dial it on, and `epoch` is the beacon time.
fn announcement(
    visibility: Visibility,
    id: DeviceId,
    name: &str,
    epoch: beacon::Epoch,
    addrs: &[SocketAddr],
) -> Option<Announcement> {
    match visibility {
        Visibility::Everyone => {
            let mut ports: BTreeMap<u16, Vec<IpAddr>> = BTreeMap::new();
            for addr in addrs {
                ports.entry(addr.port()).or_default().push(addr.ip());
            }
            Some(Announcement {
                label: plain_label(id),
                ports: ports.into_iter().collect(),
                txt: vec![(
                    NAME_KEY.to_owned(),
                    device_name::truncate_bytes(name, MAX_NAME_BYTES).to_owned(),
                )],
            })
        }
        Visibility::IdHolders => {
            // The real ports are sealed; the SRV record carries a constant one.
            let (label, sealed) = beacon::seal(
                &id,
                epoch,
                name,
                port_of(addrs, SocketAddr::is_ipv4),
                port_of(addrs, SocketAddr::is_ipv6),
            );
            let ips: BTreeSet<IpAddr> = addrs.iter().map(SocketAddr::ip).collect();
            Some(Announcement {
                label,
                ports: if ips.is_empty() {
                    Vec::new()
                } else {
                    vec![(beacon::DECOY_PORT, ips.into_iter().collect())]
                },
                txt: vec![(beacon::TXT_KEY.to_owned(), sealed)],
            })
        }
        Visibility::Hidden => None,
    }
}

/// The instance label of a plain announcement: the Device ID.
fn plain_label(id: DeviceId) -> String {
    id.to_string().to_lowercase()
}

/// The port of the first of `addrs` of the given address family, or 0 if there is none.
pub(crate) fn port_of(addrs: &[SocketAddr], family: fn(&SocketAddr) -> bool) -> u16 {
    addrs.iter().find(|a| family(a)).map_or(0, SocketAddr::port)
}

/// Whether another Device could dial `addr`. A Device on the real network does not announce a
/// loopback address (the Loopback test network does), and an IPv6 link-local address is
/// useless without the interface it belongs to.
pub(crate) fn dialable(addr: &SocketAddr, loopback_ok: bool) -> bool {
    let ip = addr.ip();
    let link_local_v6 = matches!(ip, IpAddr::V6(v6) if v6.segments()[0] & 0xffc0 == 0xfe80);
    addr.port() != 0
        && !ip.is_unspecified()
        && !ip.is_multicast()
        && !link_local_v6
        && (loopback_ok || !ip.is_loopback())
}

/// One instance as the `Discoverer` heard it, before it is read.
struct Instance {
    label: String,
    addrs: Vec<(IpAddr, u16)>,
    /// The TXT `name` of a plain announcement.
    name: Option<String>,
    /// The TXT value of a beacon, still sealed.
    beacon: Option<String>,
}

/// What an instance means.
#[derive(Debug, PartialEq, Eq)]
enum Heard {
    Seen {
        id: DeviceId,
        /// The label it was heard under: a beacon's changes every epoch.
        label: String,
        name: Option<String>,
        addrs: Vec<SocketAddr>,
        /// Whether it came in a beacon, so only a Device holding the ID can have made it.
        sealed: bool,
    },
    /// The instance that was heard under `label` has expired.
    Gone { label: String },
}

/// Reads one heard instance: a plain announcement, or a beacon of a Device whose ID this Device
/// holds (`index`). `None` for anything else, and for this Device's own.
fn hear(own: DeviceId, loopback_ok: bool, index: &Index, heard: &Instance) -> Option<Heard> {
    let label = heard.label.as_str();
    let plain: Option<DeviceId> = label.parse().ok();
    if plain == Some(own) {
        return None;
    }
    // swarm-discovery reports an instance that expired as one with no addresses.
    if heard.addrs.is_empty() {
        return Some(Heard::Gone { label: label.to_owned() });
    }
    let (id, name, addrs, sealed) = match plain {
        Some(id) => (id, heard.name.clone(), heard.addrs.clone(), false),
        None => {
            let (id, epoch) = index.recognise(label)?;
            let contents = beacon::open(&id, epoch, label, heard.beacon.as_deref()?)?;
            // The ports are in the seal; the addresses of the announcement say where.
            let addrs = heard
                .addrs
                .iter()
                .filter_map(|&(ip, _)| {
                    let port = if ip.is_ipv4() { contents.v4_port } else { contents.v6_port };
                    (port != 0).then_some((ip, port))
                })
                .collect();
            (id, contents.name, addrs, true)
        }
    };
    let addrs: Vec<SocketAddr> = addrs
        .iter()
        .map(|&(ip, port)| SocketAddr::new(ip, port))
        .filter(|addr| dialable(addr, loopback_ok))
        .collect();
    if addrs.is_empty() {
        return None;
    }
    Some(Heard::Seen {
        id,
        label: label.to_owned(),
        name: name.as_deref().and_then(device_name::sanitize),
        addrs,
        sealed,
    })
}

/// One Nearby Device in the [`Table`].
struct Entry {
    name: Option<String>,
    label: String,
    sealed: bool,
}

/// The Nearby Devices heard so far, by ID.
#[derive(Default)]
struct Table(HashMap<DeviceId, Entry>);

impl Table {
    /// Applies what was heard; `true` if the list a user would see changed. Hearing a Device
    /// again unchanged (it answers every query), or under a new label, is not a change.
    fn apply(&mut self, heard: &Heard) -> bool {
        match heard {
            Heard::Seen { id, label, name, sealed, .. } => {
                if !self.0.contains_key(id) && self.0.len() >= MAX_NEARBY {
                    return false;
                }
                let entry = Entry { name: name.clone(), label: label.clone(), sealed: *sealed };
                self.0.insert(*id, entry).is_none_or(|old| old.name != *name)
            }
            Heard::Gone { label } => match self.id_with_label(label) {
                Some(id) => self.0.remove(&id).is_some(),
                None => false,
            },
        }
    }

    /// The Device last heard under `label`. A Device heard under a newer label has moved on,
    /// so the expiry of an older one is not its departure.
    fn id_with_label(&self, label: &str) -> Option<DeviceId> {
        self.0.iter().find(|(_, e)| e.label == label).map(|(id, _)| *id)
    }

    /// The list, by name (ignoring case) then ID, so it does not shuffle between events.
    fn snapshot(&self) -> Vec<NearbyDevice> {
        let mut list: Vec<_> = self
            .0
            .iter()
            .map(|(id, e)| NearbyDevice { id: *id, name: e.name.clone() })
            .collect();
        list.sort_by_cached_key(|d| (d.name.as_deref().map(str::to_lowercase), d.id.to_string()));
        list
    }
}

/// The running `Discoverer` and what it was started with.
struct Running {
    /// `None` if it could not start, which the next [`Discovery::refresh`] tries again.
    guard: Option<DropGuard>,
    /// Why what has to run (the `Discoverer`, or the responder while Hidden) could not start, as
    /// of the last try. `None` while it runs.
    problem: Option<UnavailableReason>,
    /// The instance label `guard` was started with.
    label: String,
    tx: mpsc::UnboundedSender<Instance>,
    interfaces: Vec<Ipv4Addr>,
    /// Whether the Visibility is Hidden: no `Discoverer`, and a responder instead.
    hidden: bool,
    /// Answers lookups, while the Visibility is Hidden and only then. Stopped or failed ones are
    /// started again by [`Discovery::revive`].
    responder: Option<Responder>,
}

impl Running {
    /// Whether what has to run is running: nothing to start again.
    fn working(&self) -> bool {
        self.problem.is_none()
            && (!self.hidden || self.responder.as_ref().is_some_and(|r| !r.is_finished()))
    }
}

/// A Device's LAN discovery, owned by [`Shared`].
pub(crate) struct Discovery {
    network: Network,
    /// `None` before it starts, and after shutdown. Also serialises changes to what is
    /// announced.
    running: tokio::sync::Mutex<Option<Running>>,
    table: Mutex<Table>,
    /// Where Nearby Devices' addresses go, for iroh to dial with.
    lookup: MemoryLookup,
    /// Whether a `Discoverer` is running, so heard instances mean something: it is not while
    /// Hidden, and what it said before then must not fill the list again.
    listening: AtomicBool,
    /// Whether discovery works, as last reported.
    status: Mutex<DiscoveryStatus>,
    /// Set when Contacts are added or removed: the beacons to recognise are those of the
    /// Devices whose IDs this Device holds.
    contacts_changed: AtomicBool,
}

impl Discovery {
    /// Makes the address lookup iroh will consult for Nearby Devices. Nothing is on the network
    /// until [`start`](Self::start).
    pub(crate) fn new(network: Network, endpoint: &Endpoint) -> Self {
        let lookup = MemoryLookup::with_provenance("lan");
        match endpoint.address_lookup() {
            Ok(services) => services.add(lookup.clone()),
            Err(e) => tracing::warn!("LAN discovery cannot hand addresses to iroh: {e}"),
        }
        Self {
            network,
            running: tokio::sync::Mutex::new(None),
            table: Mutex::default(),
            lookup,
            listening: AtomicBool::new(false),
            status: Mutex::new(DiscoveryStatus::Working),
            contacts_changed: AtomicBool::new(false),
        }
    }

    /// Starts listening, and announcing as the Visibility says. Discovery is a convenience: if it
    /// cannot start (no multicast-capable network, port 5353 refused) the Device works without
    /// it, says so in its [`DiscoveryStatus`], and tries again every [`DISCOVERY_CHECK`].
    pub(crate) async fn start(&self, sh: &Arc<Shared>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let interfaces: Vec<Ipv4Addr> = self.interfaces().await.into_iter().collect();
        // How this Device asks for a Hidden one when dialling an ID.
        match sh.endpoint.address_lookup() {
            Ok(services) => {
                services.add(LanLookup::new(interfaces.clone(), sh.clock.clone(), self.loopback_ok()))
            }
            Err(e) => tracing::warn!("LAN lookups cannot hand addresses to iroh: {e}"),
        }
        *self.running.lock().await = Some(Running {
            guard: None,
            problem: None,
            label: String::new(),
            tx,
            interfaces,
            hidden: false,
            responder: None,
        });
        self.refresh(sh).await;
        sh.tasks.spawn(ingest(sh.clone(), rx));
        sh.tasks.spawn(follow_addresses(sh.clone()));
        sh.tasks.spawn(rotate_beacon(sh.clone()));
        sh.tasks.spawn(keep_trying(sh.clone()));
    }

    /// Whether this Device is on the loopback test network, where loopback addresses are real.
    fn loopback_ok(&self) -> bool {
        self.network == Network::LocalhostLan
    }

    /// The Devices heard so far.
    pub(crate) fn nearby(&self) -> Vec<NearbyDevice> {
        self.table.lock().unwrap_or_else(|e| e.into_inner()).snapshot()
    }

    /// Whether discovery is working.
    pub(crate) fn status(&self) -> DiscoveryStatus {
        *self.status.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Notes that a Contact was added or removed.
    pub(crate) fn contacts_changed(&self) {
        self.contacts_changed.store(true, Ordering::Release);
    }

    /// Announces what the Visibility, Device Name, addresses and time call for now.
    pub(crate) async fn refresh(&self, sh: &Shared) {
        let mut running = self.running.lock().await;
        let Some(running) = running.as_mut() else { return };
        let visibility = Visibility::load(&sh.db).await;
        let name = sh.device_name().await;
        let addrs = self.announced_addrs(&sh.endpoint);
        let announce = announcement(visibility, sh.id, &name, beacon::epoch_of(sh.now()), &addrs);

        running.hidden = visibility == Visibility::Hidden;
        if running.hidden {
            // Nothing is sent, so no `Discoverer`: it would query for the service all the time.
            // What it had heard goes with it.
            self.listening.store(false, Ordering::Release);
            running.guard = None;
            running.label.clear();
            self.forget_all(sh);
            self.start_responder(sh, running);
            self.report(sh, running);
            return;
        }
        running.responder = None;

        // A different label (a new epoch, or beacon for plain) needs a new Discoverer.
        let wanted = announce.as_ref().map(|a| a.label.as_str());
        if running.guard.is_none() || wanted.is_some_and(|label| label != running.label) {
            let label = wanted.map_or_else(|| plain_label(sh.id), str::to_owned);
            running.guard = None;
            match self.spawn(&label, &running.tx, &running.interfaces) {
                Ok(guard) => {
                    running.guard = Some(guard);
                    running.problem = None;
                }
                Err(e) => {
                    let repeat = running.problem.replace(e.reason).is_some();
                    let what = format!("LAN discovery could not start for {}", sh.id.fingerprint());
                    log_failure(repeat, &what, &e);
                }
            }
            running.label = label;
            self.listening.store(running.guard.is_some(), Ordering::Release);
        }
        self.report(sh, running);
        let Some(guard) = &running.guard else { return };

        // Removing everything also drops the TXT attributes, so they are set again below.
        guard.remove_all();
        let Some(announce) = announce else { return };
        for (port, ips) in announce.ports {
            guard.add(port, ips);
        }
        for (key, value) in announce.txt {
            if let Err(e) = guard.set_txt_attribute(key, Some(value)) {
                tracing::warn!("could not announce this Device: {e}");
            }
        }
    }

    /// Starts the responder if there is none running. A failure is logged and left for
    /// [`Discovery::revive`] to try again.
    fn start_responder(&self, sh: &Shared, running: &mut Running) {
        if running.responder.as_ref().is_some_and(|r| !r.is_finished()) {
            return;
        }
        let (endpoint, clock) = (sh.endpoint.clone(), sh.clock.clone());
        match Responder::start(sh.id, endpoint, clock, &running.interfaces, self.loopback_ok()) {
            Ok(responder) => {
                running.responder = Some(responder);
                running.problem = None;
            }
            Err(e) => {
                running.responder = None;
                let repeat = running.problem.replace(e.reason).is_some();
                log_failure(repeat, "this Device cannot answer lookups while Hidden", &e);
            }
        }
    }

    /// Reports the status if it has changed since it was last reported.
    fn report(&self, sh: &Shared, running: &Running) {
        let status = match running.problem {
            None => DiscoveryStatus::Working,
            Some(reason) => DiscoveryStatus::Unavailable { reason },
        };
        let mut current = self.status.lock().unwrap_or_else(|e| e.into_inner());
        if *current != status {
            *current = status;
            drop(current);
            sh.events.emit(sh.now(), EventKind::DiscoveryStatus(DiscoveryStatusEvent { status }));
        }
    }

    /// Tries again whatever could not start: the `Discoverer`, or the responder while Hidden, or
    /// one that has stopped.
    async fn revive(&self, sh: &Shared) {
        let stuck = self.running.lock().await.as_ref().is_some_and(|r| !r.working());
        if stuck {
            self.refresh(sh).await;
        }
    }

    /// Starts a `Discoverer` that announces under `label` and reports what it hears to `tx`.
    fn spawn(
        &self,
        label: &str,
        tx: &mpsc::UnboundedSender<Instance>,
        interfaces: &[Ipv4Addr],
    ) -> Result<DropGuard, MdnsError> {
        // swarm-discovery starts with whichever of IPv4 and IPv6 it can bind and, when it can
        // bind neither, says only that (`CannotBind`), not why. IPv4 is what the interfaces, the
        // announcement and a Hidden Device's responder use, so it is tried first here, where the
        // reason is known, and a Device that cannot use the port on it is not discoverable.
        drop(responder::bind_mdns(interfaces)?);
        let tx = tx.clone();
        let spawned = Discoverer::new_interactive(SERVICE_NAME.to_owned(), label.to_owned())
            .with_multicast_interfaces_v4(interfaces.to_vec())
            .with_callback(move |label, peer| {
                let instance = Instance {
                    label: label.to_owned(),
                    addrs: peer.addrs().to_vec(),
                    name: peer.txt_attribute(NAME_KEY).flatten().map(str::to_owned),
                    beacon: peer.txt_attribute(beacon::TXT_KEY).flatten().map(str::to_owned),
                };
                // The receiver is gone once the Device shuts down.
                let _ = tx.send(instance);
            })
            .spawn(&tokio::runtime::Handle::current());
        spawned.map_err(|e| MdnsError::new(UnavailableReason::Other, io::Error::other(e)))
    }

    /// Stops announcing and listening.
    pub(crate) async fn shutdown(&self) {
        self.running.lock().await.take();
    }

    /// The addresses an announcement lists, IPv4 first.
    fn announced_addrs(&self, endpoint: &Endpoint) -> Vec<SocketAddr> {
        let loopback_ok = self.loopback_ok();
        let mut addrs: Vec<_> =
            direct_addrs(endpoint).into_iter().filter(|a| dialable(a, loopback_ok)).collect();
        addrs.sort_by_key(|a| (a.is_ipv6(), *a));
        addrs.truncate(MAX_ANNOUNCED_ADDRS);
        addrs
    }

    /// The IPv4 addresses of the interfaces to announce and listen on. `swarm-discovery` alone
    /// uses only the default route's interface, which a VPN or a second uplink can move away
    /// from the LAN. Empty (use the default) if none can be found.
    async fn interfaces(&self) -> BTreeSet<Ipv4Addr> {
        if self.network == Network::LocalhostLan {
            return BTreeSet::from([Ipv4Addr::LOCALHOST]);
        }
        netwatch::interfaces::State::new()
            .await
            .interfaces
            .values()
            .filter(|interface| interface.is_up())
            .flat_map(|interface| interface.addrs())
            .filter_map(|net| match net.addr() {
                IpAddr::V4(addr) if !addr.is_loopback() => Some(addr),
                _ => None,
            })
            .collect()
    }

    /// Applies one heard instance: hands its addresses to iroh and, if the list of Nearby
    /// Devices changed, reports the new list.
    fn take_in(&self, sh: &Shared, heard: &Heard) {
        let mut table = self.table.lock().unwrap_or_else(|e| e.into_inner());
        let gone = match heard {
            Heard::Gone { label } => table.id_with_label(label),
            Heard::Seen { .. } => None,
        };
        let changed = table.apply(heard);
        match heard {
            // Only what the table kept is handed to iroh, so the bound holds for both.
            Heard::Seen { id, addrs, .. } if table.0.contains_key(id) => {
                let addr = EndpointAddr::from_parts(
                    id.endpoint_id(),
                    addrs.iter().copied().map(TransportAddr::Ip),
                );
                self.lookup.set_endpoint_info(addr);
            }
            Heard::Seen { .. } => {}
            Heard::Gone { .. } => {
                if let Some(id) = gone {
                    self.lookup.remove_endpoint_info(id.endpoint_id());
                }
            }
        }
        if changed {
            let devices = table.snapshot();
            drop(table);
            sh.events.emit(sh.now(), EventKind::Nearby(NearbyEvent { devices }));
        }
    }

    /// Empties the list, as when this Device stops listening.
    fn forget_all(&self, sh: &Shared) {
        let mut table = self.table.lock().unwrap_or_else(|e| e.into_inner());
        let ids: Vec<DeviceId> = table.0.keys().copied().collect();
        table.0.clear();
        for id in &ids {
            self.lookup.remove_endpoint_info(id.endpoint_id());
        }
        drop(table);
        if !ids.is_empty() {
            sh.events.emit(sh.now(), EventKind::Nearby(NearbyEvent { devices: Vec::new() }));
        }
    }

    /// Drops the Devices that are Nearby only because their beacon was recognised, but whose ID
    /// is no longer held: their beacons are not recognised any more, so they would never expire.
    fn forget_unheld(&self, sh: &Shared, held: &HashSet<DeviceId>) {
        let mut table = self.table.lock().unwrap_or_else(|e| e.into_inner());
        let unheld: Vec<DeviceId> =
            table.0.iter().filter(|(id, e)| e.sealed && !held.contains(id)).map(|(id, _)| *id).collect();
        for id in &unheld {
            table.0.remove(id);
            self.lookup.remove_endpoint_info(id.endpoint_id());
        }
        if !unheld.is_empty() {
            let devices = table.snapshot();
            drop(table);
            sh.events.emit(sh.now(), EventKind::Nearby(NearbyEvent { devices }));
        }
    }
}

/// Applies what the `Discoverer` hears, in the order heard.
async fn ingest(sh: Arc<Shared>, mut rx: mpsc::UnboundedReceiver<Instance>) {
    let own = sh.id;
    let loopback_ok = sh.discovery.loopback_ok();
    // The beacons to recognise: those of the Contacts, around now. Made again when either moves.
    let mut index = Index::default();
    let mut indexed_at = None;
    // The names already written to Contacts, so a beacon heard every second writes once.
    let mut learned: HashMap<DeviceId, String> = HashMap::new();
    loop {
        let instance = tokio::select! {
            () = sh.cancel.cancelled() => return,
            heard = rx.recv() => match heard {
                Some(heard) => heard,
                None => return,
            },
        };
        // Said before this Device went Hidden, and still on its way.
        if !sh.discovery.listening.load(Ordering::Acquire) {
            continue;
        }
        let epoch = beacon::epoch_of(sh.now());
        let contacts_changed = sh.discovery.contacts_changed.swap(false, Ordering::AcqRel);
        if contacts_changed || indexed_at != Some(epoch) {
            match sh.db.contacts().await {
                Ok(contacts) => {
                    let held: HashSet<DeviceId> =
                        contacts.iter().map(|c| c.id).filter(|id| *id != own).collect();
                    index = Index::new(held.iter().copied(), epoch);
                    sh.discovery.forget_unheld(&sh, &held);
                    learned.clear();
                }
                Err(e) => tracing::warn!("could not read the Contacts to recognise beacons: {e}"),
            }
            indexed_at = Some(epoch);
        }
        let Some(heard) = hear(own, loopback_ok, &index, &instance) else { continue };
        sh.discovery.take_in(&sh, &heard);
        // A beacon is made by a Device holding the ID, so its name refreshes the Contact's.
        if let Heard::Seen { id, name: Some(name), sealed: true, .. } = &heard {
            if learned.get(id) != Some(name) {
                let result = sh.db.update_contact_connection(*id, KnownAddress::default(), Some(name.clone()));
                match result.await {
                    Ok(_) => {
                        learned.insert(*id, name.clone());
                    }
                    Err(e) => tracing::warn!("could not record a Contact's name: {e}"),
                }
            }
        }
    }
}

/// Announces again whenever this Device's addresses change.
async fn follow_addresses(sh: Arc<Shared>) {
    let mut addrs = sh.endpoint.watch_addr();
    loop {
        tokio::select! {
            () = sh.cancel.cancelled() => return,
            changed = addrs.updated() => {
                if changed.is_err() {
                    return;
                }
                sh.discovery.refresh(&sh).await;
            }
        }
    }
}

/// Announces again, under a new label, each time a new beacon epoch begins.
async fn rotate_beacon(sh: Arc<Shared>) {
    loop {
        let next = beacon::next_epoch_starts(sh.now());
        tokio::select! {
            () = sh.cancel.cancelled() => return,
            () = clock::sleep_until(&*sh.clock, next) => {}
        }
        sh.discovery.refresh(&sh).await;
    }
}

/// How often discovery that could not start, or a responder that has stopped, is started again.
const DISCOVERY_CHECK: std::time::Duration = std::time::Duration::from_secs(15);

/// Logs why something could not start. Once for each time it starts failing: a try that fails
/// again every [`DISCOVERY_CHECK`] would fill the log.
fn log_failure(repeat: bool, what: &str, e: &MdnsError) {
    if repeat {
        tracing::debug!("{what}: {e}");
    } else {
        tracing::warn!("{what}: {e}");
    }
}

/// Starts discovery, or the responder of a Hidden Device, again if it could not start or has
/// stopped.
async fn keep_trying(sh: Arc<Shared>) {
    loop {
        tokio::select! {
            () = sh.cancel.cancelled() => return,
            () = tokio::time::sleep(DISCOVERY_CHECK) => {}
        }
        sh.discovery.revive(&sh).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> DeviceId {
        DeviceId::from_endpoint_id(iroh::SecretKey::from_bytes(&[n; 32]).public())
    }

    fn label(n: u8) -> String {
        plain_label(id(n))
    }

    const ADDR: (IpAddr, u16) = (IpAddr::V4(Ipv4Addr::new(192, 168, 1, 7)), 4000);

    fn instance(label: &str, addrs: &[(IpAddr, u16)], name: Option<&str>) -> Instance {
        Instance {
            label: label.to_owned(),
            addrs: addrs.to_vec(),
            name: name.map(str::to_owned),
            beacon: None,
        }
    }

    fn hear_on_lan(label: &str, addrs: &[(IpAddr, u16)], name: Option<&str>) -> Option<Heard> {
        hear(id(1), false, &Index::default(), &instance(label, addrs, name))
    }

    fn lan_addrs() -> Vec<SocketAddr> {
        vec!["192.168.1.7:4000".parse().unwrap(), "[2001:db8::7]:4001".parse().unwrap()]
    }

    /// What Device `from` announces as a beacon in `epoch`, as another Device hears it.
    fn instance_of_beacon(from: u8, name: &str, epoch: beacon::Epoch) -> Instance {
        let a = announcement(Visibility::IdHolders, id(from), name, epoch, &lan_addrs()).unwrap();
        Instance {
            label: a.label,
            addrs: a.ports.iter().flat_map(|(p, ips)| ips.iter().map(|ip| (*ip, *p))).collect(),
            name: None,
            beacon: Some(a.txt[0].1.clone()),
        }
    }

    #[test]
    fn everyone_announces_the_device_name_and_nothing_else() {
        let a = announcement(Visibility::Everyone, id(2), "Mum's laptop", 7, &lan_addrs()).unwrap();
        // In particular no `user-data`, which iroh's own lookup uses for UserData.
        assert_eq!(a.txt, [("name".to_owned(), "Mum's laptop".to_owned())]);
        assert_eq!(a.label, label(2));
        assert_eq!(
            a.ports,
            [
                (4000, vec!["192.168.1.7".parse::<IpAddr>().unwrap()]),
                (4001, vec!["2001:db8::7".parse::<IpAddr>().unwrap()]),
            ]
        );
    }

    #[test]
    fn hidden_announces_nothing() {
        assert_eq!(announcement(Visibility::Hidden, id(2), "x", 7, &lan_addrs()), None);
    }

    #[test]
    fn people_who_have_my_id_announce_a_beacon_that_shows_nothing_of_the_device() {
        let a = announcement(Visibility::IdHolders, id(2), "Mum's laptop", 7, &lan_addrs()).unwrap();
        assert_eq!(a.label, beacon::label(&id(2), 7));
        assert_ne!(a.label, label(2), "not the Device ID");
        assert_eq!(a.txt.len(), 1);
        assert_eq!(a.txt[0].0, beacon::TXT_KEY);
        // The real ports are sealed: only the constant one is on the wire.
        assert_eq!(a.ports.len(), 1);
        assert_eq!(a.ports[0].0, beacon::DECOY_PORT);
        // Nothing of the name, the ID or the ports, in the clear.
        let wire = format!("{a:?}");
        for secret in ["Mum", "4000", "4001", &label(2), &id(2).to_string()] {
            assert!(!wire.contains(secret), "{secret} in {wire}");
        }
    }

    #[test]
    fn a_beacon_without_addresses_announces_no_addresses() {
        let a = announcement(Visibility::IdHolders, id(2), "x", 7, &[]).unwrap();
        assert!(a.ports.is_empty());
    }

    #[test]
    fn a_long_name_is_cut_to_fit_one_attribute_on_a_character_boundary() {
        // 64 four-byte characters are 256 bytes: too long for a TXT attribute.
        let name = "🦀".repeat(64);
        let a = announcement(Visibility::Everyone, id(2), &name, 7, &lan_addrs()).unwrap();
        let value = &a.txt[0].1;
        assert!(name.starts_with(value.as_str()) && !value.is_empty());
        assert!(NAME_KEY.len() + value.len() <= 254);
    }

    #[test]
    fn the_service_name_is_ours_and_a_legal_service_name() {
        assert_ne!(SERVICE_NAME, "irohv1");
        assert!(SERVICE_NAME.len() <= 15);
    }

    #[test]
    fn a_plain_announcement_is_read_as_a_device_with_a_cleaned_name() {
        assert_eq!(
            hear_on_lan(&label(2), &[ADDR], Some("  Dad\u{7}'s PC ")),
            Some(Heard::Seen {
                id: id(2),
                label: label(2),
                name: Some("Dad's PC".to_owned()),
                addrs: vec!["192.168.1.7:4000".parse().unwrap()],
                sealed: false,
            })
        );
        // No name announced, or one that is empty once cleaned.
        for name in [None, Some("\n")] {
            let Some(Heard::Seen { name, .. }) = hear_on_lan(&label(2), &[ADDR], name) else {
                panic!("not seen");
            };
            assert_eq!(name, None);
        }
    }

    #[test]
    fn an_instance_with_no_addresses_has_gone() {
        assert_eq!(hear_on_lan(&label(2), &[], None), Some(Heard::Gone { label: label(2) }));
    }

    #[test]
    fn this_device_and_foreign_labels_are_ignored() {
        assert_eq!(hear_on_lan(&label(1), &[ADDR], Some("me")), None);
        assert_eq!(hear_on_lan(&label(1), &[], None), None);
        assert_eq!(hear_on_lan("not-a-device-id", &[ADDR], None), None);
        // 32 hex characters, the shape of a beacon of a Device whose ID is not held.
        assert_eq!(hear_on_lan(&"ab".repeat(16), &[ADDR], None), None);
    }

    #[test]
    fn addresses_nobody_could_dial_are_dropped() {
        let loopback = (IpAddr::V4(Ipv4Addr::LOCALHOST), 4000);
        let unspecified = (IpAddr::V4(Ipv4Addr::UNSPECIFIED), 4000);
        let multicast = (IpAddr::V4(Ipv4Addr::new(224, 0, 0, 251)), 4000);
        let link_local = ("fe80::1".parse().unwrap(), 4000);
        let no_port = (ADDR.0, 0);
        for bad in [loopback, unspecified, multicast, link_local, no_port] {
            assert_eq!(hear_on_lan(&label(2), &[bad], None), None, "{bad:?}");
        }
        // The good ones in the same announcement survive.
        let Some(Heard::Seen { addrs, .. }) = hear_on_lan(&label(2), &[loopback, ADDR], None) else {
            panic!("not seen");
        };
        assert_eq!(addrs, ["192.168.1.7:4000".parse::<SocketAddr>().unwrap()]);
        // The test network is on the loopback interface.
        let heard = instance(&label(2), &[loopback], None);
        assert!(hear(id(1), true, &Index::default(), &heard).is_some());
    }

    #[test]
    fn a_holder_of_the_id_reads_the_beacon_and_gets_the_name_and_the_real_addresses() {
        let heard = instance_of_beacon(2, "  Mum's\u{7} laptop ", 7);
        let index = Index::new([id(2)], 7);
        assert_eq!(
            hear(id(1), false, &index, &heard),
            Some(Heard::Seen {
                id: id(2),
                label: beacon::label(&id(2), 7),
                name: Some("Mum's laptop".to_owned()),
                // The IPs are announced, the ports come from the seal.
                addrs: lan_addrs(),
                sealed: true,
            })
        );
        // Also from the epoch before and after: the clocks of two Devices differ a little.
        for listener_epoch in [6, 8] {
            let index = Index::new([id(2)], listener_epoch);
            assert!(hear(id(1), false, &index, &heard).is_some(), "epoch {listener_epoch}");
        }
    }

    #[test]
    fn a_device_without_the_id_sees_nothing_of_the_beacon() {
        let heard = instance_of_beacon(2, "Mum's laptop", 7);
        // Holding other IDs, or none, recognises nothing...
        assert_eq!(hear(id(1), false, &Index::default(), &heard), None);
        assert_eq!(hear(id(1), false, &Index::new([id(3), id(4)], 7), &heard), None);
        // ...and nor does a listener whose clock is far from the announcer's.
        assert_eq!(hear(id(1), false, &Index::new([id(2)], 9), &heard), None);
        // A forged beacon under the right label, without the key, does not open.
        let forged = Instance { beacon: Some("x".repeat(251)), ..instance_of_beacon(2, "x", 7) };
        assert_eq!(hear(id(1), false, &Index::new([id(2)], 7), &forged), None);
        let stripped = Instance { beacon: None, ..instance_of_beacon(2, "x", 7) };
        assert_eq!(hear(id(1), false, &Index::new([id(2)], 7), &stripped), None);
    }

    #[test]
    fn a_beacon_that_expires_has_gone_under_its_label() {
        let label = beacon::label(&id(2), 7);
        let expired = instance(&label, &[], None);
        assert_eq!(hear(id(1), false, &Index::default(), &expired), Some(Heard::Gone { label }));
    }

    fn seen(n: u8, name: Option<&str>) -> Heard {
        hear(id(0), false, &Index::default(), &instance(&label(n), &[ADDR], name)).unwrap()
    }

    fn seen_by_key(key: [u8; 32]) -> Heard {
        let device = DeviceId::from_endpoint_id(iroh::SecretKey::from_bytes(&key).public());
        let heard = instance(&plain_label(device), &[ADDR], None);
        hear(id(0), false, &Index::default(), &heard).unwrap()
    }

    #[test]
    fn the_table_changes_only_when_what_a_user_sees_changes() {
        let mut table = Table::default();
        assert!(table.apply(&seen(2, Some("Dad"))), "a new Device");
        assert!(!table.apply(&seen(2, Some("Dad"))), "heard again, unchanged");
        assert!(table.apply(&seen(2, Some("Dad's PC"))), "renamed");
        assert!(table.apply(&seen(3, None)), "another Device");
        assert!(table.apply(&Heard::Gone { label: label(2) }), "one left");
        assert!(!table.apply(&Heard::Gone { label: label(2) }), "already gone");
        assert!(!table.apply(&Heard::Gone { label: label(9) }), "never seen");
        assert_eq!(table.snapshot(), [NearbyDevice { id: id(3), name: None }]);
    }

    #[test]
    fn a_device_that_moved_to_a_new_label_has_not_gone_when_the_old_one_expires() {
        let index = Index::new([id(2)], 7);
        let (old, new) = (beacon::label(&id(2), 6), beacon::label(&id(2), 7));
        let hear_beacon = |epoch| hear(id(1), false, &index, &instance_of_beacon(2, "Mum", epoch));
        let mut table = Table::default();
        assert!(table.apply(&hear_beacon(6).unwrap()));
        // The next epoch's beacon is the same Device: not a change to the list.
        assert!(!table.apply(&hear_beacon(7).unwrap()));
        assert_eq!(table.id_with_label(&new), Some(id(2)));
        assert_eq!(table.id_with_label(&old), None);
        // The old label expiring does not remove it; the current one expiring does.
        assert!(!table.apply(&Heard::Gone { label: old }));
        assert_eq!(table.snapshot().len(), 1);
        assert!(table.apply(&Heard::Gone { label: new }));
        assert!(table.snapshot().is_empty());
    }

    #[test]
    fn the_table_is_bounded() {
        // Secret keys from a counter: 300 distinct Devices.
        let device = |n: u16| {
            let mut key = [1u8; 32];
            key[..2].copy_from_slice(&n.to_le_bytes());
            seen_by_key(key)
        };
        let mut table = Table::default();
        for n in 0..MAX_NEARBY as u16 {
            assert!(table.apply(&device(n)));
        }
        // One more is not kept, while one already kept is just heard again.
        assert!(!table.apply(&device(MAX_NEARBY as u16)));
        assert!(!table.apply(&device(0)));
        assert_eq!(table.snapshot().len(), MAX_NEARBY);
        // Once one leaves there is room again.
        let Heard::Seen { label: first, .. } = device(0) else { unreachable!() };
        assert!(table.apply(&Heard::Gone { label: first }));
        assert!(table.apply(&device(MAX_NEARBY as u16)));
    }

    #[test]
    fn the_list_is_sorted_by_name_then_id() {
        let mut table = Table::default();
        for (n, name) in [(1, Some("bob")), (2, Some("Alice")), (3, None), (4, Some("alice"))] {
            table.apply(&seen(n, name));
        }
        let order: Vec<_> = table.snapshot().iter().map(|d| d.name.clone()).collect();
        assert_eq!(
            order,
            [None, Some("Alice".to_owned()), Some("alice".to_owned()), Some("bob".to_owned())]
        );
    }

    #[test]
    fn a_visibility_is_stored_as_text_and_read_back() {
        for v in [Visibility::Everyone, Visibility::IdHolders, Visibility::Hidden] {
            assert_eq!(Visibility::from_setting(v.as_setting()), Some(v));
        }
        assert_eq!(Visibility::from_setting("nonsense"), None);
        assert_eq!(Visibility::default(), Visibility::IdHolders);
    }
}
