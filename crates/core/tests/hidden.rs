//! Hidden Visibility: a Device that sends nothing, is listed by nobody, and still answers a
//! Device that asks for it by its ID.
//!
//! Same harness and real multicast as `discovery.rs`, but its own binary and one test at a
//! time. A Device that sends nothing can only be shown by listening for a while and hearing
//! nothing, and how fast a Device drops out of a list depends on how many Devices are in the
//! swarm; neither means anything while other tests announce on the same machine.

mod support;

use std::time::{Duration, Instant};

use bhayanakshare_core::{Clock, DeviceId, NearbyDevice, Visibility};
use support::{
    TestDevice,
    multicast::{
        Sniffer, ask, blinded_label, contains, dns_query, is_answer_to_a_question, is_query,
        multicast_available,
    },
};

/// How long to listen to be sure nothing is sent. A Device with discovery on sends a query
/// about every 0.7 seconds.
const SILENCE: Duration = Duration::from_secs(4);

/// How long a beacon label lasts (`beacon::EPOCH_MS`), which is part of the design.
const EPOCH_MS: i64 = 10 * 60 * 1000;

/// How long it takes a Device that was listed to drop out of the list of another once it stops
/// announcing. swarm-discovery says no goodbye: a peer is dropped when nothing has been heard
/// from it for three of its turns, which is 2.1 s in a swarm of two, and that is looked at every
/// 0.86 s. The rest is slack for a loaded machine.
const DROP_WITHIN: Duration = Duration::from_secs(6);

/// How long to wait for the answer to a query that should be answered.
const ANSWERED: Duration = Duration::from_secs(2);

/// One test at a time: see the top of the file. Held until the test ends.
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn is(id: DeviceId) -> impl Fn(&[NearbyDevice]) -> bool {
    move |list| list.iter().any(|d| d.id == id)
}

fn is_absent(id: DeviceId) -> impl Fn(&[NearbyDevice]) -> bool {
    move |list| list.iter().all(|d| d.id != id)
}

fn epoch_of(device: &TestDevice) -> i64 {
    device.clock.now() / EPOCH_MS
}

/// Whether `device` answers the query for its ID in `epoch`.
async fn answers(device: &TestDevice, epoch: i64) -> bool {
    let label = blinded_label(device.device.device_id(), epoch);
    !ask(&label, &[dns_query(&label)], ANSWERED).await.is_empty()
}

/// Hides `alice` and waits for `observer` to drop her from its list; returns how long it took.
async fn hide_and_wait_until_gone(alice: &TestDevice, observer: &mut TestDevice) -> Duration {
    let id = alice.device.device_id();
    let start = Instant::now();
    alice.device.set_visibility(Visibility::Hidden).await.unwrap();
    observer.wait_nearby("Alice dropped from the list", is_absent(id)).await;
    start.elapsed()
}

#[tokio::test]
async fn a_hidden_device_sends_nothing() {
    if !multicast_available() {
        return;
    }
    let _alone = ONE_AT_A_TIME.lock().await;
    let sniffer = Sniffer::start();
    let mut alice = TestDevice::start_discovering("alice").await;

    // Hidden at runtime: after what was in flight, not a packet of ours in four seconds.
    alice.device.set_visibility(Visibility::Hidden).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    sniffer.clear();
    tokio::time::sleep(SILENCE).await;
    let sent = sniffer.of_the_service();
    assert!(sent.is_empty(), "a Hidden Device sent {} packets", sent.len());

    // Hidden from the start: the setting is kept, so the next start is Hidden.
    sniffer.clear();
    alice.restart().await;
    assert_eq!(alice.device.visibility().await, Visibility::Hidden);
    tokio::time::sleep(SILENCE).await;
    let sent = sniffer.of_the_service();
    assert!(sent.is_empty(), "a Device started Hidden sent {} packets", sent.len());

    // The same sniffer hears the Device as soon as it is not Hidden, so silence means something.
    alice.device.set_visibility(Visibility::IdHolders).await.unwrap();
    tokio::time::sleep(SILENCE).await;
    assert!(!sniffer.of_the_service().is_empty(), "the sniffer hears nothing even now");

    alice.shutdown().await;
}

#[tokio::test]
async fn a_hidden_device_is_never_listed_but_an_id_holder_can_send_to_it() {
    if !multicast_available() {
        return;
    }
    let _alone = ONE_AT_A_TIME.lock().await;
    let sniffer = Sniffer::start();
    let mut alice = TestDevice::start_discovering("alice").await;
    alice.device.set_device_name("Hidden-test Alice 4M").await.unwrap();
    alice.device.set_visibility(Visibility::Hidden).await.unwrap();
    // What she sent before this, at the default, is not what is tested.
    tokio::time::sleep(Duration::from_millis(300)).await;
    sniffer.clear();
    let mut bob = TestDevice::start_discovering("bob").await;
    let mut carol = TestDevice::start_discovering("carol").await;
    let alice_id = alice.device.device_id();
    let labels: Vec<String> = (-1..=1).map(|d| blinded_label(alice_id, epoch_of(&alice) + d)).collect();
    // Bob holds Alice's ID, which would make him recognise a beacon of hers; Carol does not.
    bob.device.add_contact(alice_id, None).await.unwrap();

    // Alice announces nothing: on the wire there is nothing of her, and nobody lists her.
    tokio::join!(bob.quiet_for(SILENCE), carol.quiet_for(SILENCE));
    assert!(sniffer.heard() > 0, "the sniffer hears nothing, so this proves nothing");
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

    // The ask and the answer were on the group, where a firewall that lets mDNS in lets them
    // through, and nobody who heard them learned her ID or name. Nobody lists her still.
    let with_her_label: Vec<_> = labels.iter().flat_map(|l| sniffer.containing(l)).collect();
    assert!(with_her_label.iter().any(|p| is_query(p)), "the question was not heard");
    assert!(with_her_label.iter().any(|p| is_answer_to_a_question(p)), "the answer was not on the group");
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
    let _alone = ONE_AT_A_TIME.lock().await;
    let mut alice = TestDevice::start_discovering("alice").await;
    alice.device.set_device_name("Hidden-test Alice 9R").await.unwrap();
    alice.device.set_visibility(Visibility::Hidden).await.unwrap();
    let mut bob = TestDevice::start_discovering("bob").await;
    let alice_id = alice.device.device_id();
    let epoch = epoch_of(&alice);

    // The token of her ID in this epoch and the ones next to it, which cover clocks a little apart.
    for e in [epoch - 1, epoch, epoch + 1] {
        let label = blinded_label(alice_id, e);
        let answered = ask(&label, &[dns_query(&label)], ANSWERED).await;
        assert_eq!(answered.len(), 1, "epoch {e}");
        // A response of a bounded size, with nothing of her in the clear but the label.
        let answer = &answered[0];
        assert!(!is_query(answer) && answer.len() < 512, "{} bytes", answer.len());
        assert!(!contains(answer, "alice"));
    }

    // Nothing else is answered: a token of another epoch or another Device, the ID itself, a
    // made-up token, a query that is not quite right, or the browsing every Device does.
    let label = blinded_label(alice_id, epoch);
    let ours = dns_query(&label);
    let mut trailing = ours.clone();
    trailing.push(0);
    let mut not_txt = ours.clone();
    let type_at = ours.len() - 3;
    not_txt[type_at] = 12;
    let mut browse = dns_query("x")[..12].to_vec();
    browse.extend_from_slice(b"\x0e_bhayanakshare\x04_udp\x05local\x00\x00\x0c\x00\x01");
    let other = |l: String| (l.clone(), dns_query(&l));
    let ignored = [
        ("two epochs on", other(blinded_label(alice_id, epoch + 2))),
        ("two epochs back", other(blinded_label(alice_id, epoch - 2))),
        ("another Device's", other(blinded_label(bob.device.device_id(), epoch))),
        ("the ID", other(alice_id.to_string().to_lowercase())),
        ("made up", other("ab".repeat(16))),
        ("a trailing byte", (label.clone(), trailing)),
        ("another type", (label.clone(), not_txt)),
        ("a browse", (label.clone(), browse)),
        ("noise", (label.clone(), vec![0xff; 100])),
    ];
    for (what, (looked_for, packet)) in ignored {
        let answered = ask(&looked_for, &[packet], Duration::from_millis(800)).await;
        assert!(answered.is_empty(), "{what} was answered");
    }

    // However it is asked, it is answered only so often.
    let answered = ask(&label, &vec![ours; 60], Duration::from_millis(1500)).await;
    assert!(!answered.is_empty() && answered.len() <= 20, "{} answers to 60 queries", answered.len());

    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn a_hidden_device_still_answers_when_the_epoch_rolls_over() {
    if !multicast_available() {
        return;
    }
    let _alone = ONE_AT_A_TIME.lock().await;
    let mut alice = TestDevice::start_discovering("alice").await;
    alice.device.set_visibility(Visibility::Hidden).await.unwrap();
    let mut bob = TestDevice::start_discovering("bob").await;
    let alice_id = alice.device.device_id();
    let first = epoch_of(&alice);

    // Her clock is one epoch ahead of Bob's: he asks for the label of the epoch she was just in,
    // which she still answers, and sends.
    alice.clock.advance(EPOCH_MS);
    assert_eq!(epoch_of(&alice), first + 1);
    let src = tempfile::tempdir().unwrap();
    let path = src.path().join("hello.txt");
    std::fs::write(&path, b"hello across an epoch").unwrap();
    let id = bob.device.send_file(alice_id, &path).await.unwrap();
    alice.wait_offer().await;
    alice.device.accept(id).await.unwrap();
    alice.wait_state(id, "completed").await;
    bob.wait_state(id, "completed").await;
    assert_eq!(std::fs::read(alice.save_dir.join("hello.txt")).unwrap(), b"hello across an epoch");

    // Her labels are the new epoch's and its neighbours', not the old ones.
    alice.clock.advance(2 * EPOCH_MS);
    let now = epoch_of(&alice);
    assert!(answers(&alice, now).await, "the label of the epoch she is in");
    assert!(answers(&alice, now - 1).await, "the one before");
    assert!(answers(&alice, now + 1).await, "the one after");
    assert!(!answers(&alice, first).await, "a label three epochs old");
    assert!(!answers(&alice, now + 2).await, "a label two epochs ahead");

    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test]
async fn changing_to_and_from_hidden_takes_effect_at_once() {
    if !multicast_available() {
        return;
    }
    let _alone = ONE_AT_A_TIME.lock().await;
    let mut alice = TestDevice::start_discovering("alice").await;
    alice.device.set_device_name("Alice's laptop").await.unwrap();
    let mut bob = TestDevice::start_discovering("bob").await;
    let (alice_id, bob_id) = (alice.device.device_id(), bob.device.device_id());
    let epoch = epoch_of(&alice);
    bob.device.add_contact(alice_id, None).await.unwrap();
    alice.device.add_contact(bob_id, None).await.unwrap();

    // At the default Visibility she announces a beacon, listens, and answers no lookup.
    bob.wait_nearby("Alice nearby", is(alice_id)).await;
    alice.wait_nearby("Bob nearby", is(bob_id)).await;
    assert!(!answers(&alice, epoch).await);

    // Hidden: she is dropped from Bob's list within the bound, she answers the lookup the
    // moment it is chosen, and her own list is emptied: she neither is seen nor sees.
    let took = hide_and_wait_until_gone(&alice, &mut bob).await;
    assert!(took < DROP_WITHIN, "Alice was still listed after {took:?}");
    eprintln!("a Device that went Hidden left another's list after {took:?}");
    assert!(answers(&alice, epoch).await);
    alice.wait_nearby("Bob not listed", |list| list.is_empty()).await;
    bob.device.set_visibility(Visibility::Everyone).await.unwrap();
    alice.quiet_for(SILENCE).await;
    assert!(alice.device.nearby().is_empty(), "{:?}", alice.device.nearby());

    // Not Hidden again: listed again, listening again, and no longer answering.
    alice.device.set_visibility(Visibility::Everyone).await.unwrap();
    bob.wait_nearby("Alice nearby again", is(alice_id)).await;
    alice.wait_nearby("Bob nearby again", is(bob_id)).await;
    assert!(!answers(&alice, epoch).await);

    // And once more, from Everyone this time.
    let took = hide_and_wait_until_gone(&alice, &mut bob).await;
    assert!(took < DROP_WITHIN, "Alice was still listed after {took:?}");
    eprintln!("a Device that went Hidden from Everyone left another's list after {took:?}");
    assert!(answers(&alice, epoch).await);

    alice.shutdown().await;
    bob.shutdown().await;
}
