//! Crash tests: a Receiver Device in a child process is killed with SIGKILL part-way through
//! a fetch, and a fresh Device on the same folders carries on from what the dead one had
//! stored. The child is this test binary run again for `child_receiver`, with the folder it
//! is to use in `BHS_CRASH_CHILD_DIR`.
//!
//! What a kill -9 loses is decided by iroh-blobs' store, which commits what it has received
//! in batches about a second apart (research/bench, `results/FINDINGS.md`). After the first
//! second the data survives and a restart only re-hashes it; a kill in the first second loses
//! that second's data, which the Transfer then fetches again. Both are accepted.

mod support;

use std::{
    io::{BufRead, BufReader, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

use bhayanakshare_core::{
    Device, DeviceAddr, DeviceId, EventKind, Network, SystemClock, SystemFreeSpace, TransferId,
    TransferState,
};
use support::TestDevice;
use tempfile::TempDir;
use tokio::sync::mpsc;

const CHILD_DIR: &str = "BHS_CRASH_CHILD_DIR";
const INCOMING: &str = ".bhayanakshare-incoming";
/// Big enough that the fetch is still running a few seconds after it starts.
const SIZE: u64 = 512 << 20;
/// For a kill right at the start, which needs no time to pass.
const SMALL: u64 = 64 << 20;
/// How long the child's fetch must have run before the kill, to be past the store's first
/// batch of writes.
const PAST_THE_FIRST_SECOND: Duration = Duration::from_millis(1_800);
const WAIT: Duration = Duration::from_secs(60);

/// The Receiver in the child process: accepts the first Offer it gets and prints what it
/// sees, one line each. When the test binary runs normally this does nothing.
#[tokio::test]
async fn child_receiver() {
    let Some(dir) = std::env::var_os(CHILD_DIR).map(PathBuf::from) else { return };
    let config = support::config(
        &dir.join("data"),
        &dir.join("save"),
        Arc::new(SystemClock),
        Network::Localhost,
        Arc::new(SystemFreeSpace),
    );
    let (device, mut events) = Device::start(config).await.unwrap();
    let addr = device.addr();
    // The test harness has left its own unfinished line on the output.
    println!();
    println!("ADDR {} {}", addr.id, addr.direct[0]);
    while let Some(event) = events.next().await {
        match event.kind {
            EventKind::Transfer(t) if t.state == TransferState::Offered => {
                device.accept(t.transfer_id).await.unwrap();
            }
            EventKind::Progress(p) => println!("PROGRESS {}", p.bytes),
            _ => {}
        }
    }
}

/// The child process, killed (and reaped) when this is dropped, so that no failing test
/// leaves it running.
struct ChildReceiver {
    child: Child,
    lines: mpsc::UnboundedReceiver<String>,
}

impl ChildReceiver {
    fn start(dir: &Path) -> Self {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "child_receiver", "--nocapture", "--test-threads=1"])
            .env(CHILD_DIR, dir)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let out = BufReader::new(child.stdout.take().unwrap());
        let (tx, lines) = mpsc::unbounded_channel();
        std::thread::spawn(move || {
            for line in out.lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self { child, lines }
    }

    /// The next line the child prints with this `tag`, as the words after it.
    async fn line(&mut self, tag: &str) -> Vec<String> {
        tokio::time::timeout(WAIT, async {
            while let Some(line) = self.lines.recv().await {
                if let Some(rest) = line.strip_prefix(tag).and_then(|r| r.strip_prefix(' ')) {
                    return rest.split(' ').map(str::to_owned).collect();
                }
            }
            panic!("the child exited without printing {tag}");
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {tag} from the child"))
    }

    /// SIGKILL: the process gets no chance to run any shutdown code.
    fn kill(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}

impl Drop for ChildReceiver {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn big_file(len: u64) -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("movie.bin");
    let mut file = std::fs::File::create(&path).unwrap();
    file.set_len(len).unwrap();
    for (i, at) in [0, len / 7, len / 3, len / 2, len - 100].into_iter().enumerate() {
        file.seek(SeekFrom::Start(at)).unwrap();
        file.write_all(format!("mark {i} at {at}").as_bytes()).unwrap();
    }
    (dir, path)
}

/// What a crash left behind and what the next Device made of it.
struct Crashed {
    /// The last progress the child reported before it was killed.
    reported: u64,
    /// The first progress the new Device reported: how much it found in the store.
    found: u64,
}

/// Alice offers a big file to a Receiver in a child process, which is killed with SIGKILL
/// once `kill_when` says so; then a new Device on the child's folders finishes the Transfer.
async fn crash_and_resume(size: u64, kill_when: impl Fn(Instant, u64) -> bool) -> Crashed {
    let mut alice = TestDevice::start("alice").await;
    let tmp = tempfile::tempdir().unwrap();
    let mut child = ChildReceiver::start(tmp.path());
    let [id, addr] = child.line("ADDR").await.try_into().unwrap();
    let bob = DeviceAddr {
        id: id.parse::<DeviceId>().unwrap(),
        direct: vec![addr.parse().unwrap()],
    };
    let (_src, path) = big_file(size);
    let transfer: TransferId = alice.device.send_file(bob, &path).await.unwrap();

    let mut first = None;
    let reported = loop {
        let [bytes] = child.line("PROGRESS").await.try_into().unwrap();
        let bytes: u64 = bytes.parse().unwrap();
        let began = *first.get_or_insert_with(Instant::now);
        assert!(bytes < size, "the Transfer finished before the kill; make the file bigger");
        if kill_when(began, bytes) {
            break bytes;
        }
    };
    child.kill();
    assert!(
        std::fs::read_dir(tmp.path().join("save").join(INCOMING)).unwrap().next().is_some(),
        "the killed Receiver left its incoming store behind"
    );

    // A new Device on the same folders: the Receiver redials Alice and finishes.
    let mut bob = TestDevice::start_in("bob", tmp, SystemFreeSpace).await;
    bob.device.note_address(alice.addr());
    bob.wait_state(transfer, "reconnecting").await;
    bob.wait_state(transfer, "transferring").await;
    bob.wait_state(transfer, "completed").await;
    alice.wait_state(transfer, "completed").await;

    let saved = std::fs::read(bob.save_dir.join("movie.bin")).unwrap();
    let sent = std::fs::read(&path).unwrap();
    assert!(saved == sent, "the file that arrived differs from the one sent");
    let found = bob.progress(transfer).first().expect("a report when the fetch resumed").bytes;
    alice.shutdown().await;
    bob.shutdown().await;
    assert_eq!(support::list_dir(&bob.save_dir.join(INCOMING)), Vec::<String>::new());
    Crashed { reported, found }
}

#[tokio::test]
async fn a_receiver_killed_mid_fetch_resumes_without_fetching_verified_data_again() {
    let kill = |began: Instant, bytes| bytes > 0 && began.elapsed() >= PAST_THE_FIRST_SECOND;
    let crashed = crash_and_resume(SIZE, kill).await;

    // What the child had reported was received and written before the kill, and the store
    // keeps it: the new Device finds it again by re-hashing, instead of fetching it again.
    // (The child may have reported a little that was still in flight.)
    eprintln!("reported before the kill: {}, found after: {}", crashed.reported, crashed.found);
    assert!(
        crashed.found * 10 >= crashed.reported * 9,
        "found {} bytes in the store after a kill at {}",
        crashed.found,
        crashed.reported
    );
    assert!(crashed.found < SIZE);
}

#[tokio::test]
async fn a_receiver_killed_in_the_first_second_still_finishes_but_may_have_lost_that_second() {
    // The accepted loss window: the store has not committed yet, so a restart may find
    // nothing in it (iroh-blobs #254), and then the Transfer fetches it all again. All this
    // can assert is that it still completes, with the right bytes (`crash_and_resume`); what
    // the store kept is not defined, so it is only reported.
    let crashed = crash_and_resume(SMALL, |_, bytes| bytes > 0).await;

    eprintln!("reported before the kill: {}, found after: {}", crashed.reported, crashed.found);
}
