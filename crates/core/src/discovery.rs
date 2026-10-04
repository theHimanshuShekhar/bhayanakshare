//! LAN discovery (spec section 3): finding Nearby Devices, and being found, with
//! `swarm-discovery` under our own service name (never iroh's `irohv1`, so other iroh apps on
//! the network neither show up here nor see us).
//!
//! One `Discoverer` runs for the Device's whole life. It always listens; what it announces
//! follows the [`Visibility`] setting and is changed while it runs. Only **Everyone** announces
//! so far: a plain announcement whose instance label is the Device ID and whose TXT carries the
//! Device Name. The other settings announce nothing yet: [`announcement`] is where a setting
//! chooses what to say and [`hear`] is where a heard label becomes a Device. (A beacon with a
//! label of its own needs the `Discoverer` restarted, since swarm-discovery fixes the label
//! when it starts.)
//!
//! What is heard is kept as a table of Nearby Devices, reported on the event stream whenever it
//! changes, and its addresses are handed to iroh, so dialling a Nearby Device by its ID alone
//! reaches it. `UserData` is never set on the endpoint: the Device Name stays on the LAN and
//! is not published to n0.
//!
//! Everything heard is unauthenticated, so a name is only shown (cleaned, with the Fingerprint
//! next to it) and never stored in a Contact (a Contact's name is refreshed by `Hello`, over an
//! authenticated connection), and addresses are only used to dial, where iroh checks the key.

use std::{
    collections::{BTreeSet, HashMap},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
};

use iroh::{Endpoint, EndpointAddr, TransportAddr, Watcher, address_lookup::MemoryLookup};
use serde::{Deserialize, Serialize};
use swarm_discovery::{Discoverer, DropGuard};
use tokio::sync::mpsc;

use crate::{
    db::{Db, DbError},
    device::{Network, Shared, direct_addrs},
    device_name,
    event::{EventKind, NearbyEvent},
    identity::DeviceId,
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
    /// Only Devices that hold this Device's ID. Announces nothing until the beacon exists.
    #[default]
    IdHolders,
    /// Nobody. Announces nothing until the responder exists.
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

/// The TXT attributes this Device announces under `visibility`, or `None` to stay silent.
fn announcement(visibility: Visibility, name: &str) -> Option<Vec<(String, String)>> {
    match visibility {
        Visibility::Everyone => {
            let mut end = name.len().min(MAX_NAME_BYTES);
            while !name.is_char_boundary(end) {
                end -= 1;
            }
            Some(vec![(NAME_KEY.to_owned(), name[..end].to_owned())])
        }
        Visibility::IdHolders | Visibility::Hidden => None,
    }
}

/// Whether another Device could dial `addr`. A Device on the real network does not announce a
/// loopback address (the Loopback test network does), and an IPv6 link-local address is
/// useless without the interface it belongs to.
fn dialable(addr: &SocketAddr, loopback_ok: bool) -> bool {
    let ip = addr.ip();
    let link_local_v6 = matches!(ip, IpAddr::V6(v6) if v6.segments()[0] & 0xffc0 == 0xfe80);
    addr.port() != 0
        && !ip.is_unspecified()
        && !ip.is_multicast()
        && !link_local_v6
        && (loopback_ok || !ip.is_loopback())
}

/// What the `Discoverer` heard about one instance.
#[derive(Debug, PartialEq, Eq)]
enum Heard {
    Seen { id: DeviceId, name: Option<String>, addrs: Vec<SocketAddr> },
    Gone(DeviceId),
}

/// Reads one heard instance: its label, addresses and TXT name. `None` for anything that is not
/// another Device's plain announcement (our own, or a label that is not a Device ID).
fn hear(
    own: DeviceId,
    loopback_ok: bool,
    label: &str,
    addrs: &[(IpAddr, u16)],
    name: Option<&str>,
) -> Option<Heard> {
    let id: DeviceId = label.parse().ok().filter(|id| *id != own)?;
    // swarm-discovery reports an instance that expired as one with no addresses.
    if addrs.is_empty() {
        return Some(Heard::Gone(id));
    }
    let addrs: Vec<SocketAddr> = addrs
        .iter()
        .map(|&(ip, port)| SocketAddr::new(ip, port))
        .filter(|addr| dialable(addr, loopback_ok))
        .collect();
    if addrs.is_empty() {
        return None;
    }
    Some(Heard::Seen { id, name: name.and_then(device_name::sanitize), addrs })
}

/// The Nearby Devices heard so far, by ID.
#[derive(Default)]
struct Table(HashMap<DeviceId, Option<String>>);

impl Table {
    /// Applies what was heard; `true` if the list a user would see changed. Hearing a Device
    /// again unchanged (it answers every query) is not a change.
    fn apply(&mut self, heard: &Heard) -> bool {
        match heard {
            Heard::Seen { id, name, .. } => {
                if !self.0.contains_key(id) && self.0.len() >= MAX_NEARBY {
                    return false;
                }
                self.0.insert(*id, name.clone()).as_ref() != Some(name)
            }
            Heard::Gone(id) => self.0.remove(id).is_some(),
        }
    }

    /// The list, by name (ignoring case) then ID, so it does not shuffle between events.
    fn snapshot(&self) -> Vec<NearbyDevice> {
        let mut list: Vec<_> =
            self.0.iter().map(|(id, name)| NearbyDevice { id: *id, name: name.clone() }).collect();
        list.sort_by_cached_key(|d| (d.name.as_deref().map(str::to_lowercase), d.id.to_string()));
        list
    }
}

/// A Device's LAN discovery, owned by [`Shared`].
pub(crate) struct Discovery {
    network: Network,
    /// The running `Discoverer`; `None` before it starts, after shutdown, or if it could not
    /// start. Also serialises changes to what it announces.
    discoverer: tokio::sync::Mutex<Option<DropGuard>>,
    table: Mutex<Table>,
    /// Where Nearby Devices' addresses go, for iroh to dial with.
    lookup: MemoryLookup,
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
            discoverer: tokio::sync::Mutex::new(None),
            table: Mutex::default(),
            lookup,
        }
    }

    /// Starts listening, and announcing as the Visibility says. Discovery is a convenience: if it
    /// cannot start (no multicast-capable network, port 5353 refused) the Device works without
    /// it, and the Nearby area shows the firewall hint.
    pub(crate) async fn start(&self, sh: &Arc<Shared>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let own = sh.id;
        let loopback_ok = self.network == Network::LocalhostLan;
        let interfaces = self.interfaces().await;
        let spawned = Discoverer::new_interactive(
            SERVICE_NAME.to_owned(),
            own.to_string().to_lowercase(),
        )
        .with_multicast_interfaces_v4(interfaces.iter().copied().collect())
        .with_callback(move |label, peer| {
            let name = peer.txt_attribute(NAME_KEY).flatten();
            if let Some(heard) = hear(own, loopback_ok, label, peer.addrs(), name) {
                // The receiver is gone once the Device shuts down.
                let _ = tx.send(heard);
            }
        })
        .spawn(&tokio::runtime::Handle::current());
        match spawned {
            Ok(guard) => *self.discoverer.lock().await = Some(guard),
            Err(e) => {
                tracing::warn!("LAN discovery could not start: {e}");
                return;
            }
        }
        self.refresh(sh).await;
        sh.tasks.spawn(ingest(sh.clone(), rx));
        sh.tasks.spawn(follow_addresses(sh.clone()));
    }

    /// The Devices heard so far.
    pub(crate) fn nearby(&self) -> Vec<NearbyDevice> {
        self.table.lock().unwrap_or_else(|e| e.into_inner()).snapshot()
    }

    /// Announces what the Visibility, Device Name and addresses call for now.
    pub(crate) async fn refresh(&self, sh: &Shared) {
        let discoverer = self.discoverer.lock().await;
        let Some(discoverer) = discoverer.as_ref() else { return };
        let visibility = Visibility::load(&sh.db).await;
        let name = sh.device_name().await;

        // Removing everything also drops the TXT attributes, so they are set again below.
        discoverer.remove_all();
        let Some(txt) = announcement(visibility, &name) else { return };
        let mut by_port: HashMap<u16, Vec<IpAddr>> = HashMap::new();
        for addr in self.announced_addrs(&sh.endpoint) {
            by_port.entry(addr.port()).or_default().push(addr.ip());
        }
        for (port, ips) in by_port {
            discoverer.add(port, ips);
        }
        for (key, value) in txt {
            if let Err(e) = discoverer.set_txt_attribute(key, Some(value)) {
                tracing::warn!("could not announce the Device Name: {e}");
            }
        }
    }

    /// Stops announcing and listening.
    pub(crate) async fn shutdown(&self) {
        self.discoverer.lock().await.take();
    }

    /// The addresses an announcement lists, IPv4 first.
    fn announced_addrs(&self, endpoint: &Endpoint) -> Vec<SocketAddr> {
        let loopback_ok = self.network == Network::LocalhostLan;
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
    fn take_in(&self, sh: &Shared, heard: Heard) {
        let mut table = self.table.lock().unwrap_or_else(|e| e.into_inner());
        let changed = table.apply(&heard);
        match &heard {
            // Only what the table kept is handed to iroh, so the bound holds for both.
            Heard::Seen { id, addrs, .. } if table.0.contains_key(id) => {
                let addr = EndpointAddr::from_parts(
                    id.endpoint_id(),
                    addrs.iter().copied().map(TransportAddr::Ip),
                );
                self.lookup.set_endpoint_info(addr);
            }
            Heard::Seen { .. } => {}
            Heard::Gone(id) => {
                self.lookup.remove_endpoint_info(id.endpoint_id());
            }
        }
        if changed {
            let devices = table.snapshot();
            drop(table);
            sh.events.emit(sh.now(), EventKind::Nearby(NearbyEvent { devices }));
        }
    }
}

/// Applies what the `Discoverer` hears, in the order heard.
async fn ingest(sh: Arc<Shared>, mut rx: mpsc::UnboundedReceiver<Heard>) {
    loop {
        let heard = tokio::select! {
            () = sh.cancel.cancelled() => return,
            heard = rx.recv() => match heard {
                Some(heard) => heard,
                None => return,
            },
        };
        sh.discovery.take_in(&sh, heard);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> DeviceId {
        DeviceId::from_endpoint_id(iroh::SecretKey::from_bytes(&[n; 32]).public())
    }

    fn label(n: u8) -> String {
        id(n).to_string().to_lowercase()
    }

    const ADDR: (IpAddr, u16) = (IpAddr::V4(Ipv4Addr::new(192, 168, 1, 7)), 4000);

    fn hear_on_lan(label: &str, addrs: &[(IpAddr, u16)], name: Option<&str>) -> Option<Heard> {
        hear(id(1), false, label, addrs, name)
    }

    #[test]
    fn everyone_announces_the_device_name_and_nothing_else() {
        let txt = announcement(Visibility::Everyone, "Mum's laptop").unwrap();
        // In particular no `user-data`, which iroh's own lookup uses for UserData.
        assert_eq!(txt, [("name".to_owned(), "Mum's laptop".to_owned())]);
    }

    #[test]
    fn the_other_settings_announce_nothing() {
        assert_eq!(announcement(Visibility::IdHolders, "x"), None);
        assert_eq!(announcement(Visibility::Hidden, "x"), None);
    }

    #[test]
    fn a_long_name_is_cut_to_fit_one_attribute_on_a_character_boundary() {
        // 64 four-byte characters are 256 bytes: too long for a TXT attribute.
        let name = "🦀".repeat(64);
        let txt = announcement(Visibility::Everyone, &name).unwrap();
        let value = &txt[0].1;
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
                name: Some("Dad's PC".to_owned()),
                addrs: vec!["192.168.1.7:4000".parse().unwrap()],
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
        assert_eq!(hear_on_lan(&label(2), &[], None), Some(Heard::Gone(id(2))));
    }

    #[test]
    fn this_device_and_foreign_labels_are_ignored() {
        assert_eq!(hear_on_lan(&label(1), &[ADDR], Some("me")), None);
        assert_eq!(hear_on_lan(&label(1), &[], None), None);
        assert_eq!(hear_on_lan("not-a-device-id", &[ADDR], None), None);
        // 32 hex characters, the shape a blinded beacon label may take later.
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
        assert!(hear(id(1), true, &label(2), &[loopback], None).is_some());
    }

    fn seen(n: u8, name: Option<&str>) -> Heard {
        hear(id(0), false, &label(n), &[ADDR], name).unwrap()
    }

    fn seen_by_key(key: [u8; 32]) -> Heard {
        let device = DeviceId::from_endpoint_id(iroh::SecretKey::from_bytes(&key).public());
        hear(id(0), false, &device.to_string(), &[ADDR], None).unwrap()
    }

    #[test]
    fn the_table_changes_only_when_what_a_user_sees_changes() {
        let mut table = Table::default();
        assert!(table.apply(&seen(2, Some("Dad"))), "a new Device");
        assert!(!table.apply(&seen(2, Some("Dad"))), "heard again, unchanged");
        assert!(table.apply(&seen(2, Some("Dad's PC"))), "renamed");
        assert!(table.apply(&seen(3, None)), "another Device");
        assert!(table.apply(&Heard::Gone(id(2))), "one left");
        assert!(!table.apply(&Heard::Gone(id(2))), "already gone");
        assert!(!table.apply(&Heard::Gone(id(9))), "never seen");
        assert_eq!(table.snapshot(), [NearbyDevice { id: id(3), name: None }]);
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
        let Heard::Seen { id: first, .. } = device(0) else { unreachable!() };
        assert!(table.apply(&Heard::Gone(first)));
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
