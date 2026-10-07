//! The log as the app installs it: to a file in the logs folder, at info level, with debug
//! lines of the app's own crates switched on and off at once, and a panic written to it. One
//! test in a binary of its own, because the subscriber and the panic hook are the process's.

use std::path::Path;

use bhayanakshare_lib::logging::Logging;

fn log_text(dir: &Path) -> String {
    let mut files: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    files.sort();
    files.iter().map(|path| std::fs::read_to_string(path).unwrap()).collect()
}

#[test]
fn the_log_is_a_file_that_follows_the_debug_switch_at_once_and_holds_panics() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("logs");
    let logging = Logging::install(&dir);

    tracing::info!(target: "bhayanakshare_core::sender", "ours at info");
    tracing::debug!(target: "bhayanakshare_core::sender", "ours at debug, switch off");
    tracing::info!(target: "iroh::socket", "dependency at info, switch off");
    tracing::warn!(target: "iroh::socket", "dependency at warn");

    logging.set_debug(true);
    tracing::debug!(target: "bhayanakshare_core::sender", "ours at debug, switch on");
    tracing::info!(target: "iroh::socket", "dependency at info, switch on");
    tracing::debug!(target: "iroh::socket", "dependency at debug, switch on");

    logging.set_debug(false);
    tracing::debug!(target: "bhayanakshare_core::sender", "ours at debug, switched off again");

    let panicked = std::thread::Builder::new()
        .name("worker".into())
        .spawn(|| panic!("a fixed message"))
        .unwrap()
        .join();
    assert!(panicked.is_err());
    logging.flush();

    let log = log_text(&dir);
    for wanted in [
        "ours at info",
        "dependency at warn",
        "ours at debug, switch on",
        "dependency at info, switch on",
        "a fixed message",
        "thread 'worker'",
    ] {
        assert!(log.contains(wanted), "`{wanted}` is not in:\n{log}");
    }
    for unwanted in [
        "switch off",
        "dependency at debug",
        "switched off again",
    ] {
        assert!(!log.contains(unwanted), "`{unwanted}` is in:\n{log}");
    }
    // A line says when, how bad, and where it came from, and has no colour codes in it.
    assert!(log.contains("INFO bhayanakshare_core::sender: ours at info"), "{log}");
    assert!(!log.contains('\u{1b}'), "{log}");
}
