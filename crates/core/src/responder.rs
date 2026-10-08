//! How a Hidden Device is found on the LAN, by the Devices that hold its Device ID and by nobody
//! else (spec section 3).
//!
//! A Hidden Device sends nothing of its own accord: no announcement, and not even the browse
//! queries of discovery, which is not running (see [`crate::discovery`]). While it is Hidden, a
//! [`Responder`] listens on the mDNS port for one kind of query and ignores everything else: a
//! TXT query whose name is `<label>._bhayanakshare._udp.local.`, where the label is this
//! Device's own blinded label of [`crate::beacon`] for the previous, current or next epoch. Only
//! a Device that holds the ID can work that label out, and on the wire it is no more than a
//! beacon's label is: random-looking, and different every epoch.
//!
//! The answer goes to the mDNS group, as a multicast response does (RFC 6762 section 6), out of
//! each interface discovery uses: not to the asker, since a reply from port 5353 to the asker's
//! own port does not match the query the asker's firewall saw go out, and is dropped by a
//! stateful one, where one that lets mDNS in lets this through. The answer is a TXT record
//! holding the beacon's sealed value, so it opens only with the ID and the epoch, and it is the
//! same size whatever it holds. The seal carries the ports; the asker takes the address from
//! where the answer came from, which is also the address that can reach it. The Device Name is
//! left out: nothing here needs it, and `Hello` carries it once connected. Whoever else hears
//! the answer learns that a host answered the label, and its address.
//!
//! [`LanLookup`] is the asking side: an iroh address lookup service that, when iroh dials an ID
//! it has no address for, sends that query to the group and gives iroh what is answered, taking
//! the answers for the label it asked for and no others.
//!
//! What a listener can do with a query it copied: replay it for as long as the label is one the
//! Responder accepts, which is up to about 30 minutes (the epoch it was made for, and one on each
//! side), and learn that way that the Device is there, and its address. Holding the ID shows as
//! much, by dialling. Nothing is aimed at anyone: the answer goes to the group whatever the
//! query's source address says, so a spoofed query cannot reflect it at a victim, and at most
//! [`MAX_ANSWERS_PER_SECOND`] go out in a second, to anyone. A packet is answered only if it is
//! byte for byte the query for one of the labels, which is why the packets are written and
//! matched by hand here and not with a DNS library: a parser that accepts compression, extra
//! records and the like would answer more than that.
//!
//! IPv4 only, like the interfaces discovery lists: a Hidden Device cannot be found over IPv6.

use std::{
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::Arc,
    time::Duration,
};

use iroh::{
    Endpoint, EndpointId,
    address_lookup::{AddressLookup, EndpointInfo, Error as LookupError, Item},
};
use n0_future::{boxed::BoxStream, task::AbortOnDropHandle};
use socket2::{Domain, Protocol, SockRef, Socket, Type};
use tokio::{net::UdpSocket, time::Instant};

use crate::{
    beacon::{self, Epoch, Index},
    clock::Clock,
    device::direct_addrs,
    discovery::{MdnsError, SERVICE_NAME, UnavailableReason, dialable, port_of},
    identity::DeviceId,
};

const GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const PORT: u16 = 5353;

/// The most a query or an answer can be. Both are well under it; a packet that fills it may have
/// been cut short and is not read.
const MAX_PACKET: usize = 512;

/// The most answers a Responder sends in one second, to anyone.
const MAX_ANSWERS_PER_SECOND: u32 = 10;

/// How long [`LanLookup`] waits for an answer before it asks again, and how many times it asks.
const RETRY: Duration = Duration::from_millis(700);
const ASKS: u32 = 3;

/// The length of a beacon label, which is the first part of the name asked for.
const LABEL_LEN: usize = 32;

/// How long an asker may keep an answer, in seconds. None is kept.
const TTL: u32 = 10;

const TYPE_TXT: [u8; 2] = [0, 16];
const CLASS_IN: [u8; 2] = [0, 1];
/// What a query starts with: no ID and no flags, one question.
const QUERY_HEADER: [u8; 12] = [0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
/// What an answer starts with: no ID, a response that speaks for its name, one question and one
/// answer.
const ANSWER_HEADER: [u8; 12] = [0, 0, 0x84, 0, 0, 1, 0, 1, 0, 0, 0, 0];
/// A name in an answer that points back at the question's.
const NAME_POINTER: [u8; 2] = [0xc0, 12];

/// The question asked for the label: its TXT record, which no browser asks for under our name.
fn question(label: &str) -> Vec<u8> {
    let service = format!("_{SERVICE_NAME}");
    let mut question = Vec::new();
    for part in [label, &service, "_udp", "local"] {
        question.push(part.len() as u8); // none is over 63 bytes
        question.extend_from_slice(part.as_bytes());
    }
    question.push(0);
    question.extend_from_slice(&TYPE_TXT);
    question.extend_from_slice(&CLASS_IN);
    question
}

/// The query for `label`, as sent to the mDNS group.
fn query(label: &str) -> Vec<u8> {
    [&QUERY_HEADER[..], &question(label)].concat()
}

/// The own label (and its epoch) that `packet` asks for, if the packet is the query of one of
/// the labels `index` holds, and nothing else.
fn own_label_in(packet: &[u8], index: &Index) -> Option<(String, Epoch)> {
    let label = std::str::from_utf8(packet.get(13..13 + LABEL_LEN)?).ok()?;
    let (_, epoch) = index.recognise(label)?;
    (packet == query(label)).then(|| (label.to_owned(), epoch))
}

/// The answer to the query for `label`: the sealed `value` of that label's epoch.
fn answer(label: &str, value: &str) -> Vec<u8> {
    let text = format!("{}={value}", beacon::TXT_KEY);
    let mut record = Vec::new();
    record.extend_from_slice(&NAME_POINTER);
    record.extend_from_slice(&TYPE_TXT);
    record.extend_from_slice(&CLASS_IN);
    record.extend_from_slice(&TTL.to_be_bytes());
    let len = u8::try_from(text.len()).expect("a sealed value fits one TXT string");
    record.extend_from_slice(&(u16::from(len) + 1).to_be_bytes());
    record.push(len);
    record.extend_from_slice(text.as_bytes());
    [&ANSWER_HEADER[..], &question(label), &record].concat()
}

/// The sealed value in `packet`, if it is an answer to the query for `label` and of the shape
/// [`answer`] makes, and nothing else.
fn read_answer<'a>(packet: &'a [u8], label: &str) -> Option<&'a str> {
    let question = question(label);
    let rest = packet.strip_prefix(&ANSWER_HEADER[..])?.strip_prefix(&question[..])?;
    let rest = rest.strip_prefix(&NAME_POINTER[..])?.strip_prefix(&TYPE_TXT[..])?;
    let rest = rest.strip_prefix(&CLASS_IN[..])?.get(4..)?; // past the TTL
    let (rdata_len, rdata) = (u16::from_be_bytes(rest.get(..2)?.try_into().ok()?), rest.get(2..)?);
    let (text_len, text) = (*rdata.first()?, rdata.get(1..)?);
    if usize::from(rdata_len) != rdata.len() || usize::from(text_len) != text.len() {
        return None;
    }
    let value = text.strip_prefix(beacon::TXT_KEY.as_bytes())?.strip_prefix(b"=")?;
    std::str::from_utf8(value).ok()
}

/// What a packet from `from` tells about `id`, whose query for `epoch` it should be the answer
/// to: the addresses it can be dialled on. The ports are in the seal and the address is the
/// one the answer came from.
fn addrs_in_answer(
    packet: &[u8],
    from: SocketAddr,
    id: &DeviceId,
    epoch: Epoch,
    loopback_ok: bool,
) -> Option<Vec<SocketAddr>> {
    let label = beacon::label(id, epoch);
    let contents = beacon::open(id, epoch, &label, read_answer(packet, &label)?)?;
    let addr = SocketAddr::new(from.ip(), contents.v4_port);
    dialable(&addr, loopback_ok).then(|| vec![addr])
}

/// Counts answers in one-second windows.
struct Limit {
    since: Instant,
    answered: u32,
}

impl Limit {
    fn new(now: Instant) -> Self {
        Self { since: now, answered: 0 }
    }

    /// Whether another answer may go out at `now`; counts it if so.
    fn allow(&mut self, now: Instant) -> bool {
        if now.saturating_duration_since(self.since) >= Duration::from_secs(1) {
            *self = Self::new(now);
        }
        if self.answered >= MAX_ANSWERS_PER_SECOND {
            return false;
        }
        self.answered += 1;
        true
    }
}

/// The labels a Responder answers: those of its own ID around an epoch, made again when the
/// epoch moves.
struct Labels {
    epoch: Option<Epoch>,
    index: Index,
}

impl Labels {
    fn at(&mut self, own: DeviceId, epoch: Epoch) -> &Index {
        if self.epoch != Some(epoch) {
            *self = Self { epoch: Some(epoch), index: Index::new([own], epoch) };
        }
        &self.index
    }
}

/// A Hidden Device's answers to lookups. Runs until dropped, or until its socket fails.
pub(crate) struct Responder {
    task: AbortOnDropHandle<()>,
}

impl Responder {
    /// Starts answering on `interfaces` (the default one if there are none). Must be called
    /// inside the Tokio runtime. Fails if the mDNS port cannot be shared.
    pub(crate) fn start(
        own: DeviceId,
        endpoint: Endpoint,
        clock: Arc<dyn Clock>,
        interfaces: &[Ipv4Addr],
        loopback_ok: bool,
    ) -> Result<Self, MdnsError> {
        let socket = bind_mdns(interfaces)?;
        let interfaces = or_default(interfaces).to_vec();
        let task = tokio::spawn(respond(own, endpoint, clock, socket, interfaces, loopback_ok));
        Ok(Self { task: AbortOnDropHandle::new(task) })
    }

    /// Whether it has stopped, because its socket failed: nothing is answered any more.
    pub(crate) fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
}

async fn respond(
    own: DeviceId,
    endpoint: Endpoint,
    clock: Arc<dyn Clock>,
    socket: UdpSocket,
    interfaces: Vec<Ipv4Addr>,
    loopback_ok: bool,
) {
    let mut buf = [0u8; MAX_PACKET];
    let mut labels = Labels { epoch: None, index: Index::default() };
    let mut limit = Limit::new(Instant::now());
    loop {
        let len = match socket.recv_from(&mut buf).await {
            Ok((len, _)) => len,
            // Windows reports an earlier send that nobody received as an error here.
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
            Err(e) => {
                tracing::error!("this Device stopped answering lookups: {e}");
                return;
            }
        };
        if len == buf.len() {
            continue;
        }
        let index = labels.at(own, beacon::epoch_of(clock.now()));
        let Some((label, epoch)) = own_label_in(&buf[..len], index) else { continue };
        if !limit.allow(Instant::now()) {
            continue;
        }
        let addrs: Vec<SocketAddr> =
            direct_addrs(&endpoint).into_iter().filter(|a| dialable(a, loopback_ok)).collect();
        let (_, sealed) = beacon::seal(
            &own,
            epoch,
            "",
            port_of(&addrs, SocketAddr::is_ipv4),
            port_of(&addrs, SocketAddr::is_ipv6),
        );
        send_to_group(&socket, &answer(&label, &sealed), &interfaces).await;
    }
}

/// Port 5353, shared with every other mDNS listener on the machine, joined to the group on each
/// of `interfaces`, and hearing what this socket sends too (a Device on this machine is on the
/// LAN as well). A failure says which of the two things discovery needs was missing: the port, or
/// an interface to join the group on.
pub(crate) fn bind_mdns(interfaces: &[Ipv4Addr]) -> Result<UdpSocket, MdnsError> {
    let other = |e| MdnsError::new(UnavailableReason::Other, e);
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).map_err(other)?;
    socket.set_reuse_address(true).map_err(other)?;
    #[cfg(unix)]
    socket.set_reuse_port(true).map_err(other)?;
    socket
        .bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, PORT).into())
        .map_err(|e| MdnsError::new(UnavailableReason::PortInUse, e))?;
    socket.set_multicast_loop_v4(true).map_err(other)?;
    let mut joined = Err(io::Error::other("no interface to listen on"));
    for interface in or_default(interfaces) {
        match socket.join_multicast_v4(&GROUP, interface) {
            Ok(()) => joined = Ok(()),
            Err(e) if joined.is_err() => joined = Err(e),
            Err(_) => {}
        }
    }
    joined.map_err(|e| MdnsError::new(UnavailableReason::NoInterface, e))?;
    socket.set_nonblocking(true).map_err(other)?;
    UdpSocket::from_std(socket.into()).map_err(other)
}

/// Sends `packet` to the mDNS group out of each of `interfaces`. An interface that cannot be
/// sent on is skipped.
async fn send_to_group(socket: &UdpSocket, packet: &[u8], interfaces: &[Ipv4Addr]) {
    for interface in interfaces {
        let sent = match SockRef::from(socket).set_multicast_if_v4(interface) {
            Ok(()) => socket.send_to(packet, (GROUP, PORT)).await.map(|_| ()),
            Err(e) => Err(e),
        };
        if let Err(e) = sent {
            tracing::debug!("could not send to the mDNS group on {interface}: {e}");
        }
    }
}

/// `interfaces`, or the system's choice if there are none.
fn or_default(interfaces: &[Ipv4Addr]) -> &[Ipv4Addr] {
    if interfaces.is_empty() { &[Ipv4Addr::UNSPECIFIED] } else { interfaces }
}

/// The address lookup service that finds a Hidden Device on the LAN: it asks for the ID dialled,
/// and has no answer for any ID that is not Hidden or not on this LAN.
pub(crate) struct LanLookup {
    interfaces: Vec<Ipv4Addr>,
    clock: Arc<dyn Clock>,
    loopback_ok: bool,
}

impl LanLookup {
    pub(crate) fn new(interfaces: Vec<Ipv4Addr>, clock: Arc<dyn Clock>, loopback_ok: bool) -> Self {
        Self { interfaces, clock, loopback_ok }
    }
}

impl std::fmt::Debug for LanLookup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LanLookup").field("interfaces", &self.interfaces).finish_non_exhaustive()
    }
}

impl AddressLookup for LanLookup {
    fn resolve(&self, endpoint_id: EndpointId) -> Option<BoxStream<Result<Item, LookupError>>> {
        let (interfaces, loopback_ok) = (self.interfaces.clone(), self.loopback_ok);
        let (id, epoch) = (DeviceId::from_endpoint_id(endpoint_id), beacon::epoch_of(self.clock.now()));
        let lookup = async move {
            let addrs = ask(&interfaces, &id, epoch, loopback_ok)
                .await
                .map_err(|e| LookupError::from_err_any("lan", e))?;
            Ok(Item::new(EndpointInfo::new(endpoint_id).with_ip_addrs(addrs), "lan", None))
        };
        Some(Box::pin(n0_future::stream::once_future(lookup)))
    }
}

/// Asks the LAN for `id` and returns the addresses its answer gives. Asks [`ASKS`] times, on each
/// of `interfaces`, listening on the group in between, and gives up with an error if nobody
/// answers: no Hidden Device with this ID is on the LAN.
async fn ask(
    interfaces: &[Ipv4Addr],
    id: &DeviceId,
    epoch: Epoch,
    loopback_ok: bool,
) -> io::Result<Vec<SocketAddr>> {
    let query = query(&beacon::label(id, epoch));
    // Bound before the first ask, so the answer to it is not missed.
    let socket = bind_mdns(interfaces)?;
    let mut buf = [0u8; MAX_PACKET];
    for _ in 0..ASKS {
        send_to_group(&socket, &query, or_default(interfaces)).await;
        let until = Instant::now() + RETRY;
        while let Ok(received) = tokio::time::timeout_at(until, socket.recv_from(&mut buf)).await {
            let (len, from) = received?;
            if len < buf.len() {
                if let Some(addrs) = addrs_in_answer(&buf[..len], from, id, epoch, loopback_ok) {
                    return Ok(addrs);
                }
            }
        }
    }
    Err(io::Error::new(io::ErrorKind::TimedOut, "no Hidden Device with this ID answered"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> DeviceId {
        DeviceId::from_endpoint_id(iroh::SecretKey::from_bytes(&[n; 32]).public())
    }

    const LAN: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 7), 5353));

    /// The sealed value Device `n` answers with in `epoch`, and the label it answers for.
    fn sealed(n: u8, epoch: Epoch, port: u16) -> (String, String) {
        beacon::seal(&id(n), epoch, "", port, 0)
    }

    #[test]
    fn a_query_is_a_txt_question_for_the_label_under_our_service() {
        let label = beacon::label(&id(1), 7);
        let packet = query(&label);
        assert_eq!(packet.len(), 76);
        assert_eq!(&packet[..12], QUERY_HEADER);
        let mut name = vec![32];
        name.extend_from_slice(label.as_bytes());
        name.extend_from_slice(b"\x0e_bhayanakshare\x04_udp\x05local\x00");
        assert_eq!(&packet[12..packet.len() - 4], name);
        assert_eq!(&packet[packet.len() - 4..], [0, 16, 0, 1]);
    }

    #[test]
    fn a_device_answers_the_labels_of_its_own_id_around_now() {
        let index = Index::new([id(1)], 10);
        for e in [9, 10, 11] {
            let label = beacon::label(&id(1), e);
            assert_eq!(own_label_in(&query(&label), &index), Some((label, e)), "epoch {e}");
        }
        for e in [8, 12] {
            assert_eq!(own_label_in(&query(&beacon::label(&id(1), e)), &index), None, "epoch {e}");
        }
    }

    #[test]
    fn a_query_for_another_devices_label_or_anything_but_a_label_is_not_answered() {
        let index = Index::new([id(1)], 10);
        assert_eq!(own_label_in(&query(&beacon::label(&id(2), 10)), &index), None, "another Device's");
        // What an onlooker could try instead: the ID itself, in a name of the right length.
        assert_eq!(own_label_in(&query(&"a".repeat(LABEL_LEN)), &index), None);
        assert_eq!(own_label_in(&query(&id(1).to_string().to_lowercase()), &index), None);
        assert_eq!(own_label_in(&[], &index), None);
        assert_eq!(own_label_in(&[0; 200], &index), None);
        // A browser's query for the service, as swarm-discovery sends it.
        let mut browse = QUERY_HEADER.to_vec();
        browse.extend_from_slice(b"\x0e_bhayanakshare\x04_udp\x05local\x00\x00\x0c\x00\x01");
        assert_eq!(own_label_in(&browse, &index), None);
    }

    #[test]
    fn only_the_exact_query_is_answered() {
        let label = beacon::label(&id(1), 10);
        let index = Index::new([id(1)], 10);
        let good = query(&label);
        assert!(own_label_in(&good, &index).is_some());
        // Every byte that is not the label matters, and so does what comes after.
        for i in (0..good.len()).filter(|i| !(13..13 + LABEL_LEN).contains(i)) {
            let mut bad = good.clone();
            bad[i] ^= 1;
            assert_eq!(own_label_in(&bad, &index), None, "byte {i}");
        }
        for len in 0..good.len() {
            assert_eq!(own_label_in(&good[..len], &index), None, "cut at {len}");
        }
        let mut longer = good.clone();
        longer.push(0);
        assert_eq!(own_label_in(&longer, &index), None, "trailing byte");
        // A response is not a query, even one that repeats the question.
        let (_, value) = sealed(1, 10, 4000);
        assert_eq!(own_label_in(&answer(&label, &value), &index), None);
    }

    #[test]
    fn an_answer_gives_back_the_sealed_value_for_its_label_only() {
        let (label, value) = sealed(1, 10, 4000);
        let packet = answer(&label, &value);
        assert_eq!(read_answer(&packet, &label), Some(value.as_str()));
        assert_eq!(read_answer(&packet, &beacon::label(&id(1), 11)), None, "another label");
        assert!(packet.len() < MAX_PACKET && packet.len() < 5 * query(&label).len());
    }

    #[test]
    fn a_damaged_or_foreign_answer_is_not_read_and_never_a_panic() {
        let (label, value) = sealed(1, 10, 4000);
        let good = answer(&label, &value);
        for len in 0..good.len() {
            assert_eq!(read_answer(&good[..len], &label), None, "cut at {len}");
        }
        let mut longer = good.clone();
        longer.push(b'x');
        assert_eq!(read_answer(&longer, &label), None, "trailing byte");
        // Flipping any byte either reads as nothing or as a value that does not open. The TTL is
        // the one thing that is not read: nothing is kept.
        let ttl = 12 + question(&label).len() + 6;
        for i in (0..good.len()).filter(|i| !(ttl..ttl + 4).contains(i)) {
            let mut bad = good.clone();
            bad[i] ^= 1;
            let opened = read_answer(&bad, &label).and_then(|v| beacon::open(&id(1), 10, &label, v));
            assert_eq!(opened, None, "byte {i}");
        }
        assert_eq!(read_answer(&query(&label), &label), None, "a query is not an answer");
        assert_eq!(read_answer(&[0xff; 600], &label), None);
    }

    #[test]
    fn the_asker_learns_the_address_from_where_the_answer_came_and_the_port_from_the_seal() {
        let (label, value) = sealed(1, 10, 4000);
        let packet = answer(&label, &value);
        let addrs = addrs_in_answer(&packet, LAN, &id(1), 10, false);
        assert_eq!(addrs, Some(vec!["192.168.1.7:4000".parse().unwrap()]));
    }

    #[test]
    fn an_answer_that_does_not_open_for_the_id_and_epoch_asked_is_ignored() {
        let (label, value) = sealed(1, 10, 4000);
        let packet = answer(&label, &value);
        assert_eq!(addrs_in_answer(&packet, LAN, &id(2), 10, false), None, "another ID");
        assert_eq!(addrs_in_answer(&packet, LAN, &id(1), 11, false), None, "another epoch");
        // A Device that answers for a label it was not asked for.
        let (other, value) = sealed(2, 10, 4000);
        assert_eq!(addrs_in_answer(&answer(&other, &value), LAN, &id(1), 10, false), None);
        // Nothing to dial: no port in the seal, or an address nobody could dial.
        let (label, none) = sealed(1, 10, 0);
        assert_eq!(addrs_in_answer(&answer(&label, &none), LAN, &id(1), 10, false), None);
        let loopback = SocketAddr::from((Ipv4Addr::LOCALHOST, PORT));
        assert_eq!(addrs_in_answer(&packet, loopback, &id(1), 10, false), None);
        assert!(addrs_in_answer(&packet, loopback, &id(1), 10, true).is_some(), "the test network is loopback");
    }

    #[test]
    fn answers_are_limited_to_so_many_a_second() {
        let start = Instant::now();
        let mut limit = Limit::new(start);
        let granted = (0..100).filter(|_| limit.allow(start)).count();
        assert_eq!(granted, MAX_ANSWERS_PER_SECOND as usize);
        assert!(!limit.allow(start + Duration::from_millis(999)));
        assert!(limit.allow(start + Duration::from_secs(1)), "a new second, a new allowance");
    }

    #[tokio::test]
    async fn an_interface_that_is_not_there_is_not_taken_for_a_port_in_use() {
        // 192.0.2.0/24 (TEST-NET-1) is assigned to nobody, so no interface has it.
        let result = bind_mdns(&[Ipv4Addr::new(192, 0, 2, 1)]);
        let reason = result.err().map(|e| e.reason);
        assert_eq!(reason, Some(UnavailableReason::NoInterface));
    }

    #[tokio::test]
    async fn a_responder_whose_task_has_ended_says_so() {
        let ended = Responder { task: AbortOnDropHandle::new(tokio::spawn(async {})) };
        let running = Responder { task: AbortOnDropHandle::new(tokio::spawn(std::future::pending())) };
        tokio::task::yield_now().await;
        for _ in 0..50 {
            if ended.is_finished() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(ended.is_finished());
        assert!(!running.is_finished());
    }
}
