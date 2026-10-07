//! What a Transfer writes to the log (spec section 8): Devices are named by Fingerprint and
//! never by full Device ID, and no file name, folder name or text is ever logged.
//!
//! The subscriber here is the one the app installs, minus the file: the same filter, at its
//! most talkative (`log_filter(true)`, Debug logging on), writing to memory. It is installed
//! globally, and this is a test binary of its own with one test, because the Devices run on
//! tasks and threads that iroh starts itself: a subscriber scoped to the test's thread
//! (`set_default`) would miss exactly the lines most likely to leak. A global one would also
//! fight any other test in the same process.

mod support;

use std::{
    io,
    sync::{Arc, Mutex},
    time::Duration,
};

use bhayanakshare_core::{
    DeviceAddr, log_filter,
    manifest::{Entry, Manifest},
    protocol::{self, Hello, Message, Offer, read_frame, write_frame},
};
use iroh_blobs::{
    Hash, HashAndFormat,
    protocol::{GetRequest, ObserveRequest, Request},
};
use support::{TestDevice, dial_addr, raw_peer};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

const FILE_NAME: &str = "secret-plan-XYZZY.txt";
const FOLDER_NAME: &str = "private-folder-QWERTY";
const TEXT: &str = "the launch code is PINEAPPLE-7731-ZULU";
/// A folder the Receiver picks to save into, which is the Receiver's own business too.
const SAVE_FOLDER_NAME: &str = "chosen-save-folder-JKLMN";
const INNER_FOLDER_NAME: &str = "inner-folder-MNBVC";
const INNER_FILE_NAME: &str = "inner-file-LKJHG.bin";
const LOCKED_FOLDER_NAME: &str = "locked-folder-ASDFG";
const LOCKED_FILE_NAME: &str = "locked-file-HJKLP.txt";
const CHANGING_FILE_NAME: &str = "changing-file-BNMVC.txt";
const READONLY_FOLDER_NAME: &str = "readonly-save-folder-POIUY";
const READONLY_FILE_NAME: &str = "readonly-file-TGBNH.txt";
const HOSTILE_FILE_NAME: &str = "hostile-file-ZXCVB.txt";

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Captured {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Every way a Device ID can be spelled that something in the stack might print: our own base32,
/// iroh's hex, and the z-base-32 of its pkarr and DNS lookups.
fn spellings(base32: &str, bytes: &[u8; 32]) -> Vec<String> {
    let z32 = iroh::EndpointId::from_bytes(bytes).unwrap().to_z32();
    [base32.to_owned(), base32.to_lowercase(), hex(bytes), hex(bytes).to_uppercase(), z32].into()
}

#[tokio::test]
async fn a_transfers_logs_hold_no_full_device_id_file_name_folder_name_or_text() {
    let captured = Captured::default();
    let writer = captured.clone();
    tracing_subscriber::registry()
        .with(EnvFilter::new(log_filter(true)))
        .with(fmt::layer().with_ansi(false).with_writer(move || writer.clone()))
        .init();

    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let src = tempfile::tempdir().unwrap();

    // A folder with a file in it and one in a folder below, accepted into a folder of Bob's
    // choosing.
    let folder = src.path().join(FOLDER_NAME);
    std::fs::create_dir_all(folder.join(INNER_FOLDER_NAME)).unwrap();
    std::fs::write(folder.join(FILE_NAME), b"the plan").unwrap();
    std::fs::write(folder.join(INNER_FOLDER_NAME).join(INNER_FILE_NAME), vec![7u8; 200_000]).unwrap();
    let id = alice.device.send_file(bob.addr(), &folder).await.unwrap();
    bob.wait_offer().await;
    let chosen = bob.save_dir.join(SAVE_FOLDER_NAME);
    std::fs::create_dir_all(&chosen).unwrap();
    bob.device.accept_into(id, Some(&chosen)).await.unwrap();
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;

    // The same folder again: its names are now taken in the folder it is saved to.
    let id = alice.device.send_file(bob.addr(), &folder).await.unwrap();
    bob.wait_offer().await;
    bob.device.accept_into(id, Some(&chosen)).await.unwrap();
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;

    // Text, accepted, and text, declined.
    let id = alice.device.send_text(bob.addr(), TEXT).await.unwrap();
    bob.wait_offer().await;
    bob.device.accept(id).await.unwrap();
    bob.wait_state(id, "completed").await;
    let id = alice.device.send_text(bob.addr(), TEXT).await.unwrap();
    bob.wait_offer().await;
    bob.device.decline(id).await.unwrap();
    alice.wait_state(id, "declined").await;

    // A file the Sender cannot read: whatever the Sender logs about it holds a path.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let locked = src.path().join(LOCKED_FOLDER_NAME);
        std::fs::create_dir_all(&locked).unwrap();
        let file = locked.join(LOCKED_FILE_NAME);
        std::fs::write(&file, vec![9u8; 100_000]).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
        if let Ok(id) = alice.device.send_file(bob.addr(), &locked).await {
            bob.wait_offer().await;
            let _ = bob.device.accept(id).await;
            alice.wait_for("the locked Transfer to end", |t| t.transfer_id == id && t.state.is_terminal()).await;
        }
    }

    // A file that changes on Alice's side after it was offered: the Sender tells the user which
    // one, and the log must not.
    let changing = src.path().join(CHANGING_FILE_NAME);
    std::fs::write(&changing, b"as offered").unwrap();
    let id = alice.device.send_file(bob.addr(), &changing).await.unwrap();
    bob.wait_offer().await;
    std::fs::write(&changing, b"no longer as offered").unwrap();
    bob.device.accept(id).await.unwrap();
    alice.wait_state(id, "failed").await;

    // A folder Bob cannot write to, chosen to save into: whatever Bob logs about the failure
    // must not hold its path, which is under a folder the user named.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let readonly = bob.save_dir.join(READONLY_FOLDER_NAME);
        std::fs::create_dir_all(&readonly).unwrap();
        std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o555)).unwrap();
        let file = src.path().join(READONLY_FILE_NAME);
        std::fs::write(&file, vec![3u8; 100_000]).unwrap();
        let id = alice.device.send_file(bob.addr(), &file).await.unwrap();
        bob.wait_offer().await;
        if bob.device.accept_into(id, Some(&readonly)).await.is_ok() {
            bob.wait_for("the Transfer to the read-only folder to end", |t| {
                t.transfer_id == id && t.state.is_terminal()
            })
            .await;
        }
        std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    // A stranger asking Alice's provider for content it was never given, and pushing some.
    let stranger = raw_peer().await;
    let blobs = stranger.connect(dial_addr(&alice), iroh_blobs::ALPN).await.unwrap();
    for request in [
        Request::Get(GetRequest::from(HashAndFormat::raw(Hash::new(b"nothing")))),
        Request::Observe(ObserveRequest::new(Hash::new(b"nothing"))),
    ] {
        let (mut send, mut recv) = blobs.open_bi().await.unwrap();
        send.write_all(&postcard::to_allocvec(&request).unwrap()).await.unwrap();
        send.finish().unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(10), recv.read(&mut [0u8; 1])).await;
    }

    // A Device nothing can place, whose ID is all Alice has: the failure to dial it is logged
    // with whatever the network stack says about it.
    let mut carol = TestDevice::start("carol").await;
    let id = alice.device.send_text(DeviceAddr::from(carol.device.device_id()), TEXT).await.unwrap();
    alice.wait_state(id, "failed").await;

    // A hostile Sender whose Offer is refused, naming a file in a way that is not allowed.
    let peer = raw_peer().await;
    let conn = peer.connect(dial_addr(&bob), protocol::ALPN).await.unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, &Message::Hello(Hello::current())).await.unwrap();
    assert!(matches!(read_frame(&mut recv).await.unwrap(), Message::Hello(_)));
    let bad = format!("{FOLDER_NAME}/../{HOSTILE_FILE_NAME}");
    let offer = Offer::new([5; 16], Manifest { entries: vec![Entry::file(bad, 1)] }, 0);
    write_frame(&mut send, &Message::Offer(offer)).await.unwrap();
    assert!(matches!(read_frame(&mut recv).await.unwrap(), Message::InvalidOffer));

    let mut ids = vec![
        (alice.device.device_id().to_string(), *alice.device.device_id().as_bytes()),
        (bob.device.device_id().to_string(), *bob.device.device_id().as_bytes()),
        (carol.device.device_id().to_string(), *carol.device.device_id().as_bytes()),
    ];
    let peer_bytes = *peer.id().as_bytes();
    ids.push((data_encoding::BASE32_NOPAD.encode(&peer_bytes), peer_bytes));
    let stranger_bytes = *stranger.id().as_bytes();
    ids.push((data_encoding::BASE32_NOPAD.encode(&stranger_bytes), stranger_bytes));
    let hostile_fingerprint = {
        let id = &ids[3].0;
        format!("{}-{}", &id[..4], &id[4..8])
    };

    // Stop everything and let the Devices say what they have to say on the way out.
    alice.shutdown().await;
    bob.shutdown().await;
    carol.shutdown().await;
    drop((conn, peer, blobs, stranger));
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Everything that must not appear, as lowercase text to look for.
    let mut forbidden: Vec<String> = ids.iter().flat_map(|(base32, bytes)| spellings(base32, bytes)).collect();
    forbidden.extend(
        [
            FILE_NAME,
            FOLDER_NAME,
            INNER_FOLDER_NAME,
            INNER_FILE_NAME,
            TEXT,
            "PINEAPPLE",
            SAVE_FOLDER_NAME,
            LOCKED_FOLDER_NAME,
            LOCKED_FILE_NAME,
            READONLY_FOLDER_NAME,
            READONLY_FILE_NAME,
            CHANGING_FILE_NAME,
            HOSTILE_FILE_NAME,
        ]
        .map(String::from),
    );
    let forbidden: Vec<String> = forbidden.iter().map(|s| s.to_lowercase()).collect();

    let logs = captured.text();
    let leaking: Vec<&str> = logs
        .lines()
        .filter(|line| {
            let line = line.to_lowercase();
            forbidden.iter().any(|secret| line.contains(secret))
        })
        .collect();
    assert!(leaking.is_empty(), "the log leaks a full Device ID, a name or text in:\n{}", leaking.join("\n"));

    // Not vacuous: the app's own lines are there, debug ones among them (so debug really was on),
    // and they name the hostile Sender by Fingerprint.
    assert!(logs.contains("bhayanakshare_core"), "nothing of ours was logged:\n{logs}");
    assert!(logs.contains("DEBUG bhayanakshare_core::gate: GET refused"), "no debug line in:\n{logs}");
    assert!(logs.contains(&hostile_fingerprint), "no Fingerprint ({hostile_fingerprint}) in:\n{logs}");
}
