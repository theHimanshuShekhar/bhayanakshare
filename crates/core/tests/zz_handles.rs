//! TEMP-DEBUG: which files of a stopped Device's data folder are still open.
mod support;

use std::{path::Path, sync::Arc, time::Duration};

use bhayanakshare_core::{Device, Network, SystemClock, SystemFreeSpace};
use support::TestDevice;

fn report(what: &str, dir: &Path) {
    fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(dir, &mut files);
    eprintln!("HANDLES {what}: {} files", files.len());
    for f in files {
        // Rename is refused for a file open without delete sharing, just as a delete is.
        let moved = f.with_extension("probe-moved");
        let r = std::fs::rename(&f, &moved);
        eprintln!("HANDLES   {} -> {:?}", f.strip_prefix(dir).unwrap().display(), r.as_ref().map(|_| "free").map_err(|e| e.to_string()));
        if r.is_ok() {
            std::fs::rename(&moved, &f).unwrap();
        }
    }
}

#[tokio::test]
async fn after_a_transfer_and_shutdown() {
    let mut alice = TestDevice::start("alice").await;
    let mut bob = TestDevice::start("bob").await;
    let src = tempfile::tempdir().unwrap();
    let path = src.path().join("a.txt");
    std::fs::write(&path, b"hello").unwrap();
    let id = alice.device.send_file(bob.addr(), &path).await.unwrap();
    bob.wait_offer().await;
    bob.device.accept(id).await.unwrap();
    bob.wait_state(id, "completed").await;
    alice.wait_state(id, "completed").await;
    alice.shutdown().await;
    bob.shutdown().await;
    report("alice after shutdown", &alice.data_dir);
    report("bob after shutdown", &bob.data_dir);
}

#[tokio::test]
async fn a_device_that_did_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (data, save) = (tmp.path().join("data"), tmp.path().join("save"));
    std::fs::create_dir_all(&save).unwrap();
    let config = support::config(&data, &save, Arc::new(SystemClock), Network::Localhost, Arc::new(SystemFreeSpace));
    let (device, events) = Device::start(config).await.unwrap();
    device.shutdown(Duration::from_secs(30)).await;
    report("idle device after shutdown", &data);
    drop(events);
    drop(device);
    tokio::time::sleep(Duration::from_secs(2)).await;
    report("idle device after drop", &data);
}
