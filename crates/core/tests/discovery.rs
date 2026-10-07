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
//! (as CI should) to make a missing multicast a failure instead. The probe and the raw mDNS
//! helpers are in `support::multicast`; what a Hidden Device does is in `hidden.rs`.

mod support;

use std::{
    net::{IpAddr, Ipv4Addr},
    sync::{Arc, Mutex},
    time::Duration,
};

use bhayanakshare_core::{DeviceId, NearbyDevice, Visibility};
use support::{TestDevice, multicast::multicast_available};
use swarm_discovery::{Discoverer, DropGuard};

/// How long to wait to be sure a Device is not heard. The announcements come about every second.
const SILENCE: Duration = Duration::from_secs(4);

/// How long a beacon label lasts (`beacon::EPOCH_MS`), which is part of the design.
const EPOCH_MS: i64 = 10 * 60 * 1000;

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

/// What Settings → Device Name does: the default Visibility, nothing restarted, and the new name
/// is in the Nearby list of a Device that holds the ID (within `wait_nearby`'s bound) while the
/// old one is not.
#[tokio::test]
async fn renaming_a_device_at_the_default_visibility_shows_the_new_name_nearby_without_a_restart() {
    if !multicast_available() {
        return;
    }
    let mut alice = TestDevice::start_discovering("alice").await;
    alice.device.set_device_name("Alice's laptop").await.unwrap();
    let mut bob = TestDevice::start_discovering("bob").await;
    let alice_id = alice.device.device_id();
    bob.device.add_contact(alice_id, None).await.unwrap();
    assert_eq!(alice.device.visibility().await, Visibility::IdHolders, "the default");
    bob.wait_nearby("the first name", is(alice_id, Some("Alice's laptop"))).await;

    assert_eq!(alice.device.set_device_name("  Alice's desktop ").await.unwrap(), "Alice's desktop");
    let seen = bob.wait_nearby("the new name", is(alice_id, Some("Alice's desktop"))).await;
    assert!(!is(alice_id, Some("Alice's laptop"))(&seen), "{seen:?}");
    assert_eq!(alice.device.device_name().await, "Alice's desktop");

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
