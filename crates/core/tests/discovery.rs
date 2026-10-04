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
    net::{Ipv4Addr, SocketAddrV4},
    sync::OnceLock,
    time::Duration,
};

use bhayanakshare_core::{DeviceId, NearbyDevice, Visibility};
use socket2::{Domain, Protocol, Socket, Type};
use support::TestDevice;

/// How long to wait to be sure a Device is not heard. The announcements come about every second.
const SILENCE: Duration = Duration::from_secs(4);

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
async fn only_everyone_is_announced() {
    if !multicast_available() {
        return;
    }
    // Neither the default nor Hidden announces anything yet, so the Everyone Device sees both
    // of them only once they could have been heard.
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
