//! LAN discovery: Devices find each other over multicast, as Devices on one LAN do.
//!
//! This is its own seam. The Devices here run on `Network::LocalhostLan`: they bind to
//! 127.0.0.1 and use real mDNS multicast on the loopback interface, port 5353 included, so
//! they also hear any other Device on this machine (another test binary, another agent). Every
//! check therefore looks for one specific Device and never expects an exact list.
//!
//! Multicast is not available everywhere (a sandbox without it, port 5353 held exclusively).
//! When a probe finds it missing, each test says so on stderr and returns without testing
//! anything, so run with `--nocapture` to see that. Set `BHAYANAKSHARE_REQUIRE_MULTICAST=1`
//! (as CI should) to make a missing multicast a failure instead.

mod support;

use std::{
    mem::MaybeUninit,
    net::{IpAddr, Ipv4Addr, SocketAddrV4},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use bhayanakshare_core::{Clock, DeviceId, NearbyDevice, Visibility};
use socket2::{Domain, Protocol, Socket, Type};
use support::TestDevice;
use swarm_discovery::{Discoverer, DropGuard};

/// How long to wait to be sure a Device is not heard. The announcements come about every second.
const SILENCE: Duration = Duration::from_secs(4);

/// How long a beacon label lasts (`beacon::EPOCH_MS`), which is part of the design.
const EPOCH_MS: i64 = 10 * 60 * 1000;

/// Why multicast on the loopback interface cannot be used here, or `None` if it can. Does what
/// swarm-discovery does: binds port 5353 shared, joins the mDNS group on 127.0.0.1, and sends to
/// it from the loopback interface.
fn multicast_problem() -> Option<String> {
    let probe = || -> std::io::Result<()> {
        let group = Ipv4Addr::new(224, 0, 0, 251);
        let udp = || Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP));

        let mdns_port = udp()?;
        mdns_port.set_reuse_address(true)?;
        mdns_port.set_reuse_port(true)?;
        mdns_port.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 5353).into())?;

        let rx = udp()?;
        rx.set_reuse_address(true)?;
        rx.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0).into())?;
        rx.join_multicast_v4(&group, &Ipv4Addr::LOCALHOST)?;
        rx.set_read_timeout(Some(Duration::from_secs(2)))?;
        let port = rx.local_addr()?.as_socket().expect("an IP socket").port();

        let tx = udp()?;
        tx.set_multicast_if_v4(&Ipv4Addr::LOCALHOST)?;
        tx.set_multicast_loop_v4(true)?;
        tx.send_to(b"probe", &SocketAddrV4::new(group, port).into())?;
        let mut buf = [MaybeUninit::uninit(); 16];
        rx.recv(&mut buf).map(|_| ())
    };
    probe().err().map(|e| e.to_string())
}

/// `true` if multicast works; otherwise says why the test is not running, or fails if multicast
/// was required.
fn multicast_available() -> bool {
    static PROBLEM: OnceLock<Option<String>> = OnceLock::new();
    let problem = PROBLEM.get_or_init(multicast_problem);
    let Some(problem) = problem else { return true };
    assert!(
        std::env::var_os("BHAYANAKSHARE_REQUIRE_MULTICAST").is_none(),
        "multicast over the loopback interface is required but not available: {problem}"
    );
    eprintln!(
        "SKIPPED: this test needs multicast over the loopback interface, which is not \
         available here ({problem}). LAN discovery was NOT tested."
    );
    false
}

fn is(id: DeviceId, name: Option<&str>) -> impl Fn(&[NearbyDevice]) -> bool {
    move |list| list.iter().any(|d| d.id == id && name.is_none_or(|n| d.name.as_deref() == Some(n)))
}

fn is_absent(id: DeviceId) -> impl Fn(&[NearbyDevice]) -> bool {
    move |list| list.iter().all(|d| d.id != id)
}

async fn everyone(name: &str, device_name: &str) -> TestDevice {
    let device = TestDevice::start_discovering(name).await;
    device.device.set_device_name(device_name).await.unwrap();
    device.device.set_visibility(Visibility::Everyone).await.unwrap();
    device
}

#[tokio::test]
async fn two_everyone_devices_see_each_other_with_their_names() {
    if !multicast_available() {
        return;
    }
    let mut alice = everyone("alice", "Alice's laptop").await;
    let mut bob = everyone("bob", "Bob's PC").await;
    let (alice_id, bob_id) = (alice.device.device_id(), bob.device.device_id());

    let seen = bob.wait_nearby("Alice's laptop nearby", is(alice_id, Some("Alice's laptop"))).await;
    alice.wait_nearby("Bob's PC nearby", is(bob_id, Some("Bob's PC"))).await;

    // The same list is there to ask for, and a Device is never Nearby to itself.
    assert!(is(alice_id, Some("Alice's laptop"))(&seen));
    assert!(is(alice_id, Some("Alice's laptop"))(&bob.device.nearby()));
    assert!(is_absent(alice_id)(&alice.device.nearby()));
    assert!(is_absent(bob_id)(&bob.device.nearby()));

    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn a_nearby_device_can_be_sent_to_by_its_id_alone() {
    if !multicast_available() {
        return;
    }
    let mut alice = everyone("alice", "Alice's laptop").await;
    let mut bob = TestDevice::start_discovering("bob").await;
    let alice_id = alice.device.device_id();
    bob.wait_nearby("Alice nearby", is(alice_id, None)).await;

    // No address is handed over: the ones heard on the LAN are what reaches Alice.
    let src = tempfile::tempdir().unwrap();
    let path = src.path().join("hello.txt");
    std::fs::write(&path, b"hello from the LAN").unwrap();
    let id = bob.device.send_file(alice_id, &path).await.unwrap();
    let offer = alice.wait_offer().await;
    assert_eq!(offer.peer, bob.device.device_id());
    alice.device.accept(id).await.unwrap();
    alice.wait_state(id, "completed").await;
    bob.wait_state(id, "completed").await;
    assert_eq!(std::fs::read(alice.save_dir.join("hello.txt")).unwrap(), b"hello from the LAN");

    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn only_everyone_is_announced_in_the_clear() {
    if !multicast_available() {
        return;
    }
    // The default announces a beacon and Hidden announces nothing, and the Everyone Device
    // holds neither ID, so it sees neither of them once they could have been heard.
    let mut listener = everyone("listener", "Listener").await;
    let mut quiet = TestDevice::start_discovering("quiet").await;
    let mut hidden = TestDevice::start_discovering("hidden").await;
    hidden.device.set_visibility(Visibility::Hidden).await.unwrap();
    let mut loud = everyone("loud", "Loud").await;
    assert_eq!(quiet.device.visibility().await, Visibility::IdHolders, "the default");

    // `loud` is heard, and by then the others had as long to be.
    let loud_id = loud.device.device_id();
    listener.wait_nearby("Loud nearby", is(loud_id, Some("Loud"))).await;
    listener.quiet_for(SILENCE).await;
    let heard = listener.device.nearby();
    assert!(is_absent(quiet.device.device_id())(&heard), "{heard:?}");
    assert!(is_absent(hidden.device.device_id())(&heard), "{heard:?}");
    // They still hear everyone, whatever their own setting.
    quiet.wait_nearby("Loud nearby", is(loud_id, Some("Loud"))).await;

    listener.shutdown().await;
    quiet.shutdown().await;
    hidden.shutdown().await;
    loud.shutdown().await;
}

#[tokio::test]
async fn changing_the_visibility_takes_effect_at_once() {
    if !multicast_available() {
        return;
    }
    let mut alice = TestDevice::start_discovering("alice").await;
    let mut bob = TestDevice::start_discovering("bob").await;
    let alice_id = alice.device.device_id();

    // Not announced until Everyone is chosen...
    bob.quiet_for(SILENCE).await;
    assert!(is_absent(alice_id)(&bob.device.nearby()));
    alice.device.set_visibility(Visibility::Everyone).await.unwrap();
    bob.wait_nearby("Alice nearby", is(alice_id, None)).await;

    // ...and gone from the list soon after it is taken away. Expiry takes a few announcement
    // periods, which is why this waits for the event rather than a fixed time.
    alice.device.set_visibility(Visibility::Hidden).await.unwrap();
    bob.wait_nearby("Alice gone", is_absent(alice_id)).await;
    assert!(is_absent(alice_id)(&bob.device.nearby()));

    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn a_new_device_name_is_announced_at_once() {
    if !multicast_available() {
        return;
    }
    let mut alice = everyone("alice", "Alice's laptop").await;
    let mut bob = TestDevice::start_discovering("bob").await;
    let alice_id = alice.device.device_id();
    bob.wait_nearby("the first name", is(alice_id, Some("Alice's laptop"))).await;

    alice.device.set_device_name("Alice's desktop").await.unwrap();
    bob.wait_nearby("the new name", is(alice_id, Some("Alice's desktop"))).await;

    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn an_announced_name_is_never_stored_in_a_contact() {
    if !multicast_available() {
        return;
    }
    // Anyone on the LAN can announce any Device ID under any name, so a name heard there is
    // shown but never written to a Contact. (A connection, which is authenticated, does.)
    let mut alice = TestDevice::start_discovering("alice").await;
    let mut bob = everyone("bob", "Bob's PC").await;
    let bob_id = bob.device.device_id();
    alice.device.add_contact(bob_id, Some("Bobby")).await.unwrap();

    alice.wait_nearby("Bob nearby", is(bob_id, Some("Bob's PC"))).await;
    bob.device.set_device_name("Bob's laptop").await.unwrap();
    alice.wait_nearby("Bob renamed", is(bob_id, Some("Bob's laptop"))).await;

    let contacts = alice.device.contacts().await.unwrap();
    assert_eq!(contacts.len(), 1, "a Nearby Device is not a Contact: {contacts:?}");
    assert_eq!(contacts[0].device_name.as_deref(), Some("Bobby"));

    alice.shutdown().await;
    bob.shutdown().await;
}

/// Needs no multicast: the setting is kept whether or not anything is discovered.
#[tokio::test]
async fn the_visibility_defaults_to_people_who_have_my_id_and_is_kept() {
    let mut alice = TestDevice::start("alice").await;
    assert_eq!(alice.device.visibility().await, Visibility::IdHolders);
    for v in [Visibility::Everyone, Visibility::Hidden, Visibility::IdHolders] {
        alice.device.set_visibility(v).await.unwrap();
        assert_eq!(alice.device.visibility().await, v);
    }
    // Without discovery nothing is ever Nearby.
    assert!(alice.device.nearby().is_empty());
    alice.shutdown().await;
}

// "People who have my ID": the blinded beacon. Alice is at the default Visibility throughout.

/// A device that does what a stranger on the LAN can: listens to the service and notes everything
/// announced, without being able to read it.
struct Onlooker {
    heard: Arc<Mutex<Vec<Announced>>>,
    _guard: DropGuard,
}

struct Announced {
    label: String,
    addrs: Vec<(IpAddr, u16)>,
    txt: Vec<(String, Option<String>)>,
}

impl Onlooker {
    fn start() -> Self {
        let heard = Arc::new(Mutex::new(Vec::new()));
        let sink = heard.clone();
        let guard = Discoverer::new_interactive("bhayanakshare".to_owned(), "onlooker".to_owned())
            .with_multicast_interfaces_v4(vec![Ipv4Addr::LOCALHOST])
            .with_callback(move |label, peer| {
                sink.lock().unwrap().push(Announced {
                    label: label.to_owned(),
                    addrs: peer.addrs().to_vec(),
                    txt: peer
                        .txt_attributes()
                        .map(|(k, v)| (k.to_owned(), v.map(str::to_owned)))
                        .collect(),
                });
            })
            .spawn(&tokio::runtime::Handle::current())
            .unwrap();
        Self { heard, _guard: guard }
    }

    /// Everything heard so far, as one text to search.
    fn transcript(&self) -> String {
        let heard = self.heard.lock().unwrap();
        heard.iter().map(|a| format!("{} {:?} {:?}\n", a.label, a.addrs, a.txt)).collect()
    }

    /// How many announcements were beacons: a 32-character hex label and a `b` attribute.
    fn beacons(&self) -> usize {
        let heard = self.heard.lock().unwrap();
        let is_beacon = |a: &&Announced| {
            a.label.len() == 32
                && a.label.bytes().all(|b| b.is_ascii_hexdigit())
                && a.txt.iter().any(|(k, _)| k == "b")
        };
        heard.iter().filter(is_beacon).count()
    }
}

/// Waits until `bob`'s Contact `id` has `name` as its Device Name. As long as the other tests'
/// waits for an event: with many Devices on the multicast group, a Device is answered less often.
async fn wait_contact_name(bob: &TestDevice, id: DeviceId, name: &str) {
    for _ in 0..300 {
        let contacts = bob.device.contacts().await.unwrap();
        if contacts.iter().any(|c| c.id == id && c.device_name.as_deref() == Some(name)) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the Contact never got the Device Name {name:?}: {:?}", bob.device.contacts().await);
}

#[tokio::test]
async fn a_device_that_holds_the_id_sees_the_device_nearby_with_its_name() {
    if !multicast_available() {
        return;
    }
    let mut alice = TestDevice::start_discovering("alice").await;
    alice.device.set_device_name("Alice's laptop").await.unwrap();
    assert_eq!(alice.device.visibility().await, Visibility::IdHolders, "the default");
    let mut bob = TestDevice::start_discovering("bob").await;
    let alice_id = alice.device.device_id();

    // Bob does not have Alice's ID yet, so Alice is not there to be seen...
    bob.quiet_for(SILENCE).await;
    assert!(is_absent(alice_id)(&bob.device.nearby()));
    // ...and once he saves it, she is, with her name.
    bob.device.add_contact(alice_id, None).await.unwrap();
    let seen = bob.wait_nearby("Alice nearby", is(alice_id, Some("Alice's laptop"))).await;
    assert!(is(alice_id, Some("Alice's laptop"))(&seen));
    assert!(is(alice_id, Some("Alice's laptop"))(&bob.device.nearby()));

    // It is one-sided: Alice does not hold Bob's ID, so she does not see him.
    alice.quiet_for(SILENCE).await;
    assert!(is_absent(bob.device.device_id())(&alice.device.nearby()));

    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn a_device_found_by_its_beacon_can_be_sent_to_by_its_id_alone() {
    if !multicast_available() {
        return;
    }
    // The ports are sealed in the beacon, and are what reaches Alice: nothing else hands Bob
    // an address.
    let mut alice = TestDevice::start_discovering("alice").await;
    let mut bob = TestDevice::start_discovering("bob").await;
    let alice_id = alice.device.device_id();
    bob.device.add_contact(alice_id, None).await.unwrap();
    bob.wait_nearby("Alice nearby", is(alice_id, None)).await;

    let src = tempfile::tempdir().unwrap();
    let path = src.path().join("hello.txt");
    std::fs::write(&path, b"hello by beacon").unwrap();
    let id = bob.device.send_file(alice_id, &path).await.unwrap();
    alice.wait_offer().await;
    alice.device.accept(id).await.unwrap();
    alice.wait_state(id, "completed").await;
    bob.wait_state(id, "completed").await;
    assert_eq!(std::fs::read(alice.save_dir.join("hello.txt")).unwrap(), b"hello by beacon");

    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn a_device_without_the_id_sees_nothing_identifying() {
    if !multicast_available() {
        return;
    }
    let onlooker = Onlooker::start();
    let mut alice = TestDevice::start_discovering("alice").await;
    // Not a name any other test uses, so what is said below is about Alice alone.
    alice.device.set_device_name("Beacon-test Alice 7Q").await.unwrap();
    let mut bob = TestDevice::start_discovering("bob").await;
    let mut carol = TestDevice::start_discovering("carol").await;
    let (alice_id, carol_id) = (alice.device.device_id(), carol.device.device_id());
    // Bob holds the ID, so Alice is certainly announcing; Carol does not. Carol holds Bob's
    // instead: a Device with Contacts recognises the beacons of those and no others.
    bob.device.add_contact(alice_id, None).await.unwrap();
    carol.device.add_contact(bob.device.device_id(), None).await.unwrap();
    bob.wait_nearby("Alice nearby", is(alice_id, Some("Beacon-test Alice 7Q"))).await;
    carol.wait_nearby("Bob nearby", is(bob.device.device_id(), None)).await;
    carol.quiet_for(SILENCE).await;

    // Carol sees no trace of her: not by ID, and not in a Device list under any name.
    let heard = carol.device.nearby();
    assert!(is_absent(alice_id)(&heard), "{heard:?}");
    assert!(heard.iter().all(|d| d.name.as_deref() != Some("Beacon-test Alice 7Q")), "{heard:?}");
    assert!(is_absent(carol_id)(&alice.device.nearby()));

    // Nor does anything on the wire: the onlooker heard beacons, and none of what Alice is
    // (her ID, her name, her real ports) was in any of the announcements.
    assert!(onlooker.beacons() > 0, "no beacon was announced:\n{}", onlooker.transcript());
    let transcript = onlooker.transcript().to_lowercase();
    assert!(!transcript.contains(&alice_id.to_string().to_lowercase()));
    assert!(!transcript.contains("beacon-test alice"));
    for addr in alice.addr().direct {
        assert!(
            !transcript.contains(&format!("{:?}", (addr.ip(), addr.port())).to_lowercase()),
            "Alice's real address {addr} was announced in the clear:\n{transcript}"
        );
    }

    alice.shutdown().await;
    bob.shutdown().await;
    carol.shutdown().await;
}

#[tokio::test]
async fn the_beacon_changes_with_the_epoch_and_the_id_holder_follows_it() {
    if !multicast_available() {
        return;
    }
    let mut alice = TestDevice::start_discovering("alice").await;
    alice.device.set_device_name("Alice's laptop").await.unwrap();
    let mut bob = TestDevice::start_discovering("bob").await;
    let alice_id = alice.device.device_id();
    bob.device.add_contact(alice_id, None).await.unwrap();
    bob.wait_nearby("Alice nearby", is(alice_id, Some("Alice's laptop"))).await;

    // One epoch on, Alice announces under a new label. Bob's clock is a whole epoch behind hers,
    // which his window allows, so he keeps seeing her without a break.
    alice.clock.advance(EPOCH_MS);
    bob.quiet_for(SILENCE).await;
    assert!(is(alice_id, Some("Alice's laptop"))(&bob.device.nearby()));
    let seen_at = bob.log.iter().position(|e| is_nearby_with(e, alice_id)).unwrap();
    assert!(
        bob.log[seen_at..].iter().all(|e| !is_nearby_without(e, alice_id)),
        "Alice dropped out of the list when her beacon rotated: {:#?}",
        bob.log
    );

    // Three epochs on she is outside the window: the beacon of that epoch is not one Bob
    // recognises, which also shows the old one is no longer announced...
    alice.clock.advance(2 * EPOCH_MS);
    bob.wait_nearby("Alice out of reach of Bob's clock", is_absent(alice_id)).await;
    // ...until his clock has caught up.
    bob.clock.advance(3 * EPOCH_MS);
    bob.wait_nearby("Alice again", is(alice_id, Some("Alice's laptop"))).await;

    alice.shutdown().await;
    bob.shutdown().await;
}

fn is_nearby_with(event: &bhayanakshare_core::Event, id: DeviceId) -> bool {
    matches!(&event.kind, bhayanakshare_core::EventKind::Nearby(n) if is(id, None)(&n.devices))
}

fn is_nearby_without(event: &bhayanakshare_core::Event, id: DeviceId) -> bool {
    matches!(&event.kind, bhayanakshare_core::EventKind::Nearby(n) if is_absent(id)(&n.devices))
}

#[tokio::test]
async fn a_contacts_device_name_refreshes_from_its_beacon_and_the_nickname_stays() {
    if !multicast_available() {
        return;
    }
    let mut alice = TestDevice::start_discovering("alice").await;
    alice.device.set_device_name("Alice's laptop").await.unwrap();
    let mut bob = TestDevice::start_discovering("bob").await;
    let alice_id = alice.device.device_id();
    bob.device.add_contact(alice_id, Some("Old name from a link")).await.unwrap();

    wait_contact_name(&bob, alice_id, "Alice's laptop").await;
    alice.device.set_device_name("Alice's desktop").await.unwrap();
    wait_contact_name(&bob, alice_id, "Alice's desktop").await;

    // A Nickname is still what Bob sees, whatever Alice calls herself.
    bob.device.set_nickname(alice_id, Some("Mum")).await.unwrap();
    alice.device.set_device_name("Alice's tablet").await.unwrap();
    wait_contact_name(&bob, alice_id, "Alice's tablet").await;
    let contact = bob.device.contacts().await.unwrap().remove(0);
    assert_eq!(contact.display_name(), Some("Mum"));

    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn removing_a_contact_takes_its_beacon_out_of_the_nearby_list() {
    if !multicast_available() {
        return;
    }
    let mut alice = TestDevice::start_discovering("alice").await;
    let mut bob = TestDevice::start_discovering("bob").await;
    let alice_id = alice.device.device_id();
    bob.device.add_contact(alice_id, None).await.unwrap();
    bob.wait_nearby("Alice nearby", is(alice_id, None)).await;

    // Bob no longer holds her ID, so her beacon means nothing to him, though she goes on
    // announcing it.
    bob.device.remove_contact(alice_id).await.unwrap();
    bob.wait_nearby("Alice gone", is_absent(alice_id)).await;
    bob.quiet_for(SILENCE).await;
    assert!(is_absent(alice_id)(&bob.device.nearby()));

    alice.shutdown().await;
    bob.shutdown().await;
}

// Hidden: nothing is announced, and a Device that holds the ID can still find it by asking.

/// The mDNS group and port, which is where a Device asks for a Hidden one.
const MDNS: (Ipv4Addr, u16) = (Ipv4Addr::new(224, 0, 0, 251), 5353);

/// The blinded label `id` has in `epoch`: what a beacon is announced under and what a Hidden
/// Device is asked for. This is the design's contract (`beacon::label`), spelled out so that a
/// change to it fails here.
fn blinded_label(id: DeviceId, epoch: i64) -> String {
    let mut material = id.as_bytes().to_vec();
    material.extend_from_slice(&(epoch as u64).to_be_bytes());
    let key = blake3::derive_key("bhayanakshare 2026-10 beacon label", &material);
    data_encoding::HEXLOWER.encode(&key[..16])
}

/// The query for `label` as it goes on the wire: a TXT question for `<label>._bhayanakshare._udp.local.`.
fn dns_query(label: &str) -> Vec<u8> {
    let mut packet = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    for part in [label, "_bhayanakshare", "_udp", "local"] {
        packet.push(part.len() as u8);
        packet.extend_from_slice(part.as_bytes());
    }
    packet.extend_from_slice(&[0, 0, 16, 0, 1]);
    packet
}

/// A socket on the loopback interface that can send to the mDNS group and hear what is sent there
/// (`port` 5353, shared) or what is sent to it (0).
fn loopback_socket(port: u16) -> std::io::Result<tokio::net::UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    if port != 0 {
        socket.set_reuse_address(true)?;
        socket.set_reuse_port(true)?;
    }
    socket.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port).into())?;
    if port != 0 {
        socket.join_multicast_v4(&MDNS.0, &Ipv4Addr::LOCALHOST)?;
    }
    socket.set_multicast_if_v4(&Ipv4Addr::LOCALHOST)?;
    socket.set_multicast_loop_v4(true)?;
    socket.set_nonblocking(true)?;
    tokio::net::UdpSocket::from_std(socket.into())
}

/// Sends each packet to the mDNS group, as a Device without any ID might, and returns what comes
/// back to the sender within `wait`.
async fn ask(packets: &[Vec<u8>], wait: Duration) -> Vec<Vec<u8>> {
    let socket = loopback_socket(0).unwrap();
    for packet in packets {
        socket.send_to(packet, MDNS).await.unwrap();
    }
    let until = tokio::time::Instant::now() + wait;
    let mut answers = Vec::new();
    let mut buf = [0u8; 1500];
    while let Ok(received) = tokio::time::timeout_at(until, socket.recv(&mut buf)).await {
        answers.push(buf[..received.unwrap()].to_vec());
    }
    answers
}

/// Every packet sent to the mDNS group from the time it starts: what anyone on the LAN can read.
struct Sniffer {
    packets: Arc<Mutex<Vec<Vec<u8>>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Sniffer {
    fn start() -> Self {
        let socket = loopback_socket(5353).unwrap();
        let packets = Arc::new(Mutex::new(Vec::new()));
        let sink = packets.clone();
        let task = tokio::spawn(async move {
            let mut buf = [0u8; 1500];
            while let Ok(len) = socket.recv(&mut buf).await {
                sink.lock().unwrap().push(buf[..len].to_vec());
            }
        });
        Self { packets, task }
    }

    /// The packets containing `needle`, in the lowercase text they would be in.
    fn containing(&self, needle: &str) -> Vec<Vec<u8>> {
        let needle = needle.to_lowercase().into_bytes();
        let packets = self.packets.lock().unwrap();
        let has = |p: &&Vec<u8>| p.to_ascii_lowercase().windows(needle.len()).any(|w| w == needle);
        packets.iter().filter(has).cloned().collect()
    }

    fn heard(&self) -> usize {
        self.packets.lock().unwrap().len()
    }
}

impl Drop for Sniffer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Whether a DNS packet is a query (its response bit is clear).
fn is_query(packet: &[u8]) -> bool {
    packet[2] & 0x80 == 0
}

fn epoch_of(device: &TestDevice) -> i64 {
    device.clock.now() / EPOCH_MS
}

#[tokio::test]
async fn a_hidden_device_is_never_listed_but_an_id_holder_can_send_to_it() {
    if !multicast_available() {
        return;
    }
    let sniffer = Sniffer::start();
    let mut alice = TestDevice::start_discovering("alice").await;
    alice.device.set_device_name("Hidden-test Alice 4M").await.unwrap();
    alice.device.set_visibility(Visibility::Hidden).await.unwrap();
    let mut bob = TestDevice::start_discovering("bob").await;
    let mut carol = TestDevice::start_discovering("carol").await;
    let alice_id = alice.device.device_id();
    let labels: Vec<String> = (-1..=1).map(|d| blinded_label(alice_id, epoch_of(&alice) + d)).collect();
    // Bob holds Alice's ID, which would make him recognise a beacon of hers; Carol does not.
    bob.device.add_contact(alice_id, None).await.unwrap();

    // Alice announces nothing: on the wire there is nothing of her, and nobody lists her.
    tokio::join!(bob.quiet_for(SILENCE), carol.quiet_for(SILENCE));
    assert!(sniffer.heard() > 0, "heard nothing at all, so this proves nothing");
    for secret in [alice_id.to_string(), "Hidden-test Alice".to_owned()].iter().chain(&labels) {
        assert!(sniffer.containing(secret).is_empty(), "{secret} was on the wire");
    }
    let (seen_by_bob, seen_by_carol) = (bob.device.nearby(), carol.device.nearby());
    assert!(is_absent(alice_id)(&seen_by_bob), "{seen_by_bob:?}");
    assert!(is_absent(alice_id)(&seen_by_carol), "{seen_by_carol:?}");
    assert!(is_absent(carol.device.device_id())(&alice.device.nearby()));

    // Bob dials her by her ID alone (no relay, no lookup, no address): he asks the LAN.
    let src = tempfile::tempdir().unwrap();
    let path = src.path().join("hello.txt");
    std::fs::write(&path, b"hello to a hidden device").unwrap();
    let id = bob.device.send_file(alice_id, &path).await.unwrap();
    alice.wait_offer().await;
    alice.device.accept(id).await.unwrap();
    alice.wait_state(id, "completed").await;
    bob.wait_state(id, "completed").await;
    assert_eq!(std::fs::read(alice.save_dir.join("hello.txt")).unwrap(), b"hello to a hidden device");

    // The ask was on the LAN, and it carried only her blinded label, in a query. Still
    // nothing of her ID or name (the answer went to Bob alone), and still not listed.
    let asked: Vec<_> = labels.iter().flat_map(|l| sniffer.containing(l)).collect();
    assert!(!asked.is_empty() && asked.iter().all(|p| is_query(p)), "{asked:?}");
    for secret in [alice_id.to_string(), "Hidden-test Alice".to_owned()] {
        assert!(sniffer.containing(&secret).is_empty(), "{secret} was on the wire");
    }
    bob.quiet_for(Duration::from_secs(1)).await;
    assert!(is_absent(alice_id)(&bob.device.nearby()), "{:?}", bob.device.nearby());
    assert!(is_absent(alice_id)(&carol.device.nearby()));

    alice.shutdown().await;
    bob.shutdown().await;
    carol.shutdown().await;
}

#[tokio::test]
async fn a_hidden_device_answers_only_a_query_with_its_own_token() {
    if !multicast_available() {
        return;
    }
    let mut alice = TestDevice::start_discovering("alice").await;
    alice.device.set_device_name("Hidden-test Alice 9R").await.unwrap();
    alice.device.set_visibility(Visibility::Hidden).await.unwrap();
    let mut bob = TestDevice::start_discovering("bob").await;
    let alice_id = alice.device.device_id();
    let epoch = epoch_of(&alice);
    let wait = Duration::from_millis(800);

    // The token of her ID in this epoch and the ones next to it, which cover clocks a little apart.
    for e in [epoch - 1, epoch, epoch + 1] {
        let answers = ask(&[dns_query(&blinded_label(alice_id, e))], Duration::from_secs(5)).await;
        assert_eq!(answers.len(), 1, "epoch {e}");
        // A response, to the asker alone, of a size that is the same whatever it says: nothing
        // in the clear but the label.
        let answer = &answers[0];
        assert!(!is_query(answer) && answer.len() < 512, "{} bytes", answer.len());
        assert!(!String::from_utf8_lossy(answer).to_lowercase().contains("alice"));
    }

    // Nothing else is answered: a token of another epoch or another Device, the ID itself, a
    // made-up token, a query that is not quite right, or the browsing every Device does.
    let ours = dns_query(&blinded_label(alice_id, epoch));
    let mut trailing = ours.clone();
    trailing.push(0);
    let mut not_txt = ours.clone();
    *not_txt.last_mut().unwrap() = 1;
    not_txt[ours.len() - 3] = 12;
    let browse = {
        let mut packet = dns_query("x")[..12].to_vec();
        packet.extend_from_slice(b"\x0e_bhayanakshare\x04_udp\x05local\x00\x00\x0c\x00\x01");
        packet
    };
    let ignored = [
        ("two epochs on", dns_query(&blinded_label(alice_id, epoch + 2))),
        ("another Device's", dns_query(&blinded_label(bob.device.device_id(), epoch))),
        ("the ID", dns_query(&alice_id.to_string().to_lowercase())),
        ("made up", dns_query(&"ab".repeat(16))),
        ("a trailing byte", trailing),
        ("another type", not_txt),
        ("a browse", browse),
        ("noise", vec![0xff; 100]),
    ];
    for (what, packet) in ignored {
        assert!(ask(&[packet], wait).await.is_empty(), "{what} was answered");
    }

    // However it is asked, it is answered only so often: a replayed query is no amplifier.
    let answers = ask(&vec![ours; 60], Duration::from_millis(1500)).await;
    assert!(!answers.is_empty() && answers.len() <= 20, "{} answers to 60 queries", answers.len());

    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn changing_to_and_from_hidden_takes_effect_at_once() {
    if !multicast_available() {
        return;
    }
    let mut alice = TestDevice::start_discovering("alice").await;
    alice.device.set_device_name("Alice's laptop").await.unwrap();
    let mut bob = TestDevice::start_discovering("bob").await;
    let alice_id = alice.device.device_id();
    let token = dns_query(&blinded_label(alice_id, epoch_of(&alice)));
    let wait = Duration::from_millis(800);
    bob.device.add_contact(alice_id, None).await.unwrap();

    // At the default Visibility she announces a beacon and answers no lookup.
    bob.wait_nearby("Alice nearby", is(alice_id, Some("Alice's laptop"))).await;
    assert!(ask(&[token.clone()], wait).await.is_empty());

    // Hidden: her beacon stops, and she answers the lookup the moment it is chosen.
    alice.device.set_visibility(Visibility::Hidden).await.unwrap();
    assert_eq!(ask(&[token.clone()], Duration::from_secs(5)).await.len(), 1);
    bob.wait_nearby("Alice gone", is_absent(alice_id)).await;

    // Bob can still send to her by ID.
    let src = tempfile::tempdir().unwrap();
    let path = src.path().join("hello.txt");
    std::fs::write(&path, b"hello once hidden").unwrap();
    let id = bob.device.send_file(alice_id, &path).await.unwrap();
    alice.wait_offer().await;
    alice.device.accept(id).await.unwrap();
    alice.wait_state(id, "completed").await;
    bob.wait_state(id, "completed").await;
    assert_eq!(std::fs::read(alice.save_dir.join("hello.txt")).unwrap(), b"hello once hidden");

    // Everyone, and Hidden is over: she is listed again and no longer answers the lookup.
    alice.device.set_visibility(Visibility::Everyone).await.unwrap();
    bob.wait_nearby("Alice nearby again", is(alice_id, Some("Alice's laptop"))).await;
    assert!(ask(&[token.clone()], wait).await.is_empty());

    // And once more: Hidden answers again.
    alice.device.set_visibility(Visibility::Hidden).await.unwrap();
    assert_eq!(ask(&[token], Duration::from_secs(5)).await.len(), 1);

    alice.shutdown().await;
    bob.shutdown().await;
}
