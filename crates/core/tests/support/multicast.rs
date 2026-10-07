//! Real mDNS multicast on the loopback interface (port 5353, shared with anything else on the
//! machine), for the tests of LAN discovery: the probe that says whether it works here, a
//! sniffer that reads everything sent to the group, and what a Device without any ID can send.

use std::{
    mem::MaybeUninit,
    net::{Ipv4Addr, SocketAddrV4},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use bhayanakshare_core::DeviceId;
use socket2::{Domain, Protocol, Socket, Type};

/// The mDNS group and port, which is where a Device asks for a Hidden one.
pub const MDNS: (Ipv4Addr, u16) = (Ipv4Addr::new(224, 0, 0, 251), 5353);

/// How a DNS name starts that is under our service: the length-prefixed `_bhayanakshare`. Every
/// packet this app sends, announcement, query or answer, has it.
pub const SERVICE: &[u8] = b"\x0e_bhayanakshare";

/// Why multicast on the loopback interface cannot be used here, or `None` if it can. Does what
/// swarm-discovery does: binds port 5353 shared, joins the mDNS group on 127.0.0.1, and sends to
/// it from the loopback interface.
fn multicast_problem() -> Option<String> {
    let probe = || -> std::io::Result<()> {
        let udp = || Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP));

        let mdns_port = udp()?;
        mdns_port.set_reuse_address(true)?;
        mdns_port.set_reuse_port(true)?;
        mdns_port.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 5353).into())?;

        let rx = udp()?;
        rx.set_reuse_address(true)?;
        rx.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0).into())?;
        rx.join_multicast_v4(&MDNS.0, &Ipv4Addr::LOCALHOST)?;
        rx.set_read_timeout(Some(Duration::from_secs(2)))?;
        let port = rx.local_addr()?.as_socket().expect("an IP socket").port();

        let tx = udp()?;
        tx.set_multicast_if_v4(&Ipv4Addr::LOCALHOST)?;
        tx.set_multicast_loop_v4(true)?;
        tx.send_to(b"probe", &SocketAddrV4::new(MDNS.0, port).into())?;
        let mut buf = [MaybeUninit::uninit(); 16];
        rx.recv(&mut buf).map(|_| ())
    };
    probe().err().map(|e| e.to_string())
}

/// `true` if multicast works; otherwise says why the test is not running, or fails if multicast
/// was required.
pub fn multicast_available() -> bool {
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

/// A socket on port 5353, shared, joined to the group on the loopback interface: it hears what
/// anyone sends to the group and can send to it.
pub fn mdns_socket() -> std::io::Result<tokio::net::UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    socket.set_reuse_port(true)?;
    socket.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, MDNS.1).into())?;
    socket.join_multicast_v4(&MDNS.0, &Ipv4Addr::LOCALHOST)?;
    socket.set_multicast_if_v4(&Ipv4Addr::LOCALHOST)?;
    socket.set_multicast_loop_v4(true)?;
    socket.set_nonblocking(true)?;
    tokio::net::UdpSocket::from_std(socket.into())
}

/// The blinded label `id` has in `epoch`: what a beacon is announced under and what a Hidden
/// Device is asked for. This is the design's contract (`beacon::label`), spelled out so that a
/// change to it fails the tests.
pub fn blinded_label(id: DeviceId, epoch: i64) -> String {
    let mut material = id.as_bytes().to_vec();
    material.extend_from_slice(&(epoch as u64).to_be_bytes());
    let key = blake3::derive_key("bhayanakshare 2026-10 beacon label", &material);
    data_encoding::HEXLOWER.encode(&key[..16])
}

/// The query for `label` as it goes on the wire: a TXT question for
/// `<label>._bhayanakshare._udp.local.`.
pub fn dns_query(label: &str) -> Vec<u8> {
    let mut packet = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    for part in [label, "_bhayanakshare", "_udp", "local"] {
        packet.push(part.len() as u8);
        packet.extend_from_slice(part.as_bytes());
    }
    packet.extend_from_slice(&[0, 0, 16, 0, 1]);
    packet
}

/// Whether a DNS packet is a query (its response bit is clear).
pub fn is_query(packet: &[u8]) -> bool {
    packet.get(2).is_some_and(|flags| flags & 0x80 == 0)
}

/// Whether a DNS packet is a response that repeats the question it answers, as the answer of a
/// Hidden Device does and an announcement (which has no question) does not.
pub fn is_answer_to_a_question(packet: &[u8]) -> bool {
    packet.len() > 12 && !is_query(packet) && packet[4..6] == [0, 1]
}

/// Whether `packet` has `needle` in it, ignoring case.
pub fn contains(packet: &[u8], needle: &str) -> bool {
    let needle = needle.to_lowercase().into_bytes();
    packet.to_ascii_lowercase().windows(needle.len()).any(|w| w == needle)
}

/// Sends each packet to the group, as a Device with no ID but the label might, and returns the
/// answers to a question that come back to the group within `wait`: those that carry `label`.
pub async fn ask(label: &str, packets: &[Vec<u8>], wait: Duration) -> Vec<Vec<u8>> {
    let socket = mdns_socket().unwrap();
    for packet in packets {
        socket.send_to(packet, MDNS).await.unwrap();
    }
    let until = tokio::time::Instant::now() + wait;
    let mut answers = Vec::new();
    let mut buf = [0u8; 1500];
    while let Ok(received) = tokio::time::timeout_at(until, socket.recv(&mut buf)).await {
        let packet = &buf[..received.unwrap()];
        if is_answer_to_a_question(packet) && contains(packet, label) {
            answers.push(packet.to_vec());
        }
    }
    answers
}

/// Every packet sent to the group from the time it starts: what anyone on the LAN can read.
pub struct Sniffer {
    packets: Arc<Mutex<Vec<Vec<u8>>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Sniffer {
    pub fn start() -> Self {
        let socket = mdns_socket().unwrap();
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

    /// The packets that have `needle` in them, ignoring case.
    pub fn containing(&self, needle: &str) -> Vec<Vec<u8>> {
        let packets = self.packets.lock().unwrap();
        packets.iter().filter(|p| contains(p, needle)).cloned().collect()
    }

    /// The packets of our service: announcements, queries and answers.
    pub fn of_the_service(&self) -> Vec<Vec<u8>> {
        let packets = self.packets.lock().unwrap();
        let has = |p: &&Vec<u8>| p.windows(SERVICE.len()).any(|w| w == SERVICE);
        packets.iter().filter(has).cloned().collect()
    }

    /// How many packets of any kind were heard.
    pub fn heard(&self) -> usize {
        self.packets.lock().unwrap().len()
    }

    /// Forgets what was heard so far.
    pub fn clear(&self) {
        self.packets.lock().unwrap().clear();
    }
}

impl Drop for Sniffer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
