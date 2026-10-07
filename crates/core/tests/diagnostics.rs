//! Settings → Diagnostics through the Device API: the debug logging setting, which is kept
//! across a restart, and the export, a zip of the log files and an about file that names this
//! Device by its Fingerprint only.

mod support;

use std::{fs::File, io::Read};

use support::TestDevice;

#[tokio::test]
async fn debug_logging_is_off_until_switched_on_and_stays_as_set_after_a_restart() {
    let mut alice = TestDevice::start("alice").await;
    assert!(!alice.device.debug_logging());

    alice.device.set_debug_logging(true).await.unwrap();
    assert!(alice.device.debug_logging());
    alice.restart().await;
    assert!(alice.device.debug_logging(), "the setting is lost by a restart");

    alice.device.set_debug_logging(false).await.unwrap();
    assert!(!alice.device.debug_logging());
    alice.restart().await;
    assert!(!alice.device.debug_logging());
    alice.shutdown().await;
}

#[tokio::test]
async fn the_export_holds_the_logs_and_an_about_file_with_the_fingerprint_and_no_full_device_id() {
    let mut alice = TestDevice::start("alice").await;
    alice.device.set_debug_logging(true).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let logs = tmp.path().join("logs");
    std::fs::create_dir(&logs).unwrap();
    std::fs::write(logs.join("bhayanakshare.2026-10-06.log"), "yesterday\n").unwrap();
    std::fs::write(logs.join("bhayanakshare.2026-10-07.log"), "today\n").unwrap();
    let dest = tmp.path().join("bhayanakshare-diagnostics-2026-10-07.zip");

    alice.device.export_diagnostics(Some(&logs), &dest, "9.8.7").await.unwrap();

    let mut zip = zip::ZipArchive::new(File::open(&dest).unwrap()).unwrap();
    let mut files: Vec<String> = zip.file_names().map(str::to_owned).collect();
    files.sort();
    assert_eq!(files, ["about.txt", "logs/bhayanakshare.2026-10-06.log", "logs/bhayanakshare.2026-10-07.log"]);
    let mut text = |name: &str| {
        let mut out = String::new();
        zip.by_name(name).unwrap().read_to_string(&mut out).unwrap();
        out
    };
    assert_eq!(text("logs/bhayanakshare.2026-10-07.log"), "today\n");

    let about = text("about.txt");
    let id = alice.device.device_id();
    assert!(about.contains("App version: 9.8.7"), "{about}");
    assert!(about.contains(&format!("Device: {}", id.fingerprint())), "{about}");
    assert!(about.contains("Debug logging: on"), "{about}");
    assert!(about.contains("Visibility: IdHolders"), "{about}");
    assert!(about.contains("Network: Localhost"), "{about}");
    assert!(about.contains(&format!("System: {}", std::env::consts::OS)), "{about}");
    // Not the whole ID, in either spelling, anywhere in the zip.
    let hex: String = id.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
    let z32 = iroh::EndpointId::from_bytes(id.as_bytes()).unwrap().to_z32();
    for spelling in [id.to_string(), id.to_string().to_lowercase(), hex, z32] {
        assert!(!about.to_lowercase().contains(&spelling.to_lowercase()), "{about}");
    }
    alice.shutdown().await;
}

#[tokio::test]
async fn an_export_with_no_logs_is_still_the_about_file() {
    let mut alice = TestDevice::start("alice").await;
    let tmp = tempfile::tempdir().unwrap();

    for logs in [None, Some(tmp.path().join("no-logs-here"))] {
        let dest = tmp.path().join("out.zip");
        alice.device.export_diagnostics(logs.as_deref(), &dest, "1.0.0").await.unwrap();
        let zip = zip::ZipArchive::new(File::open(&dest).unwrap()).unwrap();
        assert_eq!(zip.file_names().collect::<Vec<_>>(), ["about.txt"]);
    }
    alice.shutdown().await;
}

#[tokio::test]
async fn an_export_that_cannot_be_written_says_so() {
    let mut alice = TestDevice::start("alice").await;
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("no-such-folder").join("out.zip");

    let err = alice.device.export_diagnostics(Some(tmp.path()), &dest, "1.0.0").await.unwrap_err();

    assert!(err.to_string().starts_with("writing the diagnostics zip"), "{err}");
    assert!(!err.to_string().contains("no-such-folder"), "{err}");
    alice.shutdown().await;
}
