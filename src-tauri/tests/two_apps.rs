//! Two shells in one process, driven the way the UI drives them: commands through Tauri's IPC
//! and events from the `device-event` stream. Stands in for launching two windows, which
//! needs a display. The Offer itself is started through the core's `Device` because the
//! `send_file` command dials by Device ID alone, and a localhost-only test network has no
//! way to resolve one.

use std::{
    sync::{Arc, mpsc},
    time::Duration,
};

use bhayanakshare_core::{Device, DeviceConfig, KeySource, Network, SystemClock, SystemFreeSpace};
use bhayanakshare_lib::{specta_builder, start_device};
use serde_json::{Value, json};
use tauri::{
    App, Listener, Manager, WebviewWindow,
    ipc::{CallbackFn, InvokeBody},
    test::{INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder, mock_context, noop_assets},
    webview::InvokeRequest,
};
use tempfile::TempDir;

const TIMEOUT: Duration = Duration::from_secs(30);

struct Shell {
    app: App<MockRuntime>,
    window: WebviewWindow<MockRuntime>,
    events: mpsc::Receiver<Value>,
    save_dir: std::path::PathBuf,
    _tmp: TempDir,
}

fn start_shell() -> Shell {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().join("data");
    let save_dir = tmp.path().join("save");
    let config = DeviceConfig {
        key_source: KeySource::File(data_dir.join("secret.key")),
        data_dir,
        save_dir: save_dir.clone(),
        clock: Arc::new(SystemClock),
        network: Network::Localhost,
        free_space: Arc::new(SystemFreeSpace),
    };

    // What `run` does in its `setup` hook, which only fires once an event loop runs.
    let builder = specta_builder::<MockRuntime>();
    let app = mock_builder()
        .invoke_handler(builder.invoke_handler())
        .build(mock_context(noop_assets()))
        .unwrap();
    builder.mount_events(&app);
    start_device(&app, config).unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default()).build().unwrap();

    let (tx, events) = mpsc::channel();
    app.listen("device-event", move |e| {
        tx.send(serde_json::from_str(e.payload()).unwrap()).unwrap();
    });
    Shell { app, window, events, save_dir, _tmp: tmp }
}

impl Shell {
    fn invoke(&self, cmd: &str, args: Value) -> Result<Value, Value> {
        let request = InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "tauri://localhost".parse().unwrap(),
            body: InvokeBody::Json(args),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        };
        get_ipc_response(&self.window, request).map(|body| body.deserialize().unwrap())
    }

    fn device(&self) -> tauri::State<'_, Device> {
        self.app.state::<Device>()
    }

    /// The next event matching `pred`, skipping the ones before it.
    fn wait_event(&self, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        loop {
            match self.events.recv_timeout(TIMEOUT) {
                Ok(event) if pred(&event) => return event,
                Ok(_) => {}
                Err(_) => panic!("timed out waiting for {what}"),
            }
        }
    }
}

fn is_transfer(state: &'static str) -> impl Fn(&Value) -> bool {
    move |e| e["type"] == "transfer" && e["state"]["kind"] == state
}

#[test]
fn a_file_goes_from_one_shell_to_another_through_commands_and_events() {
    let alice = start_shell();
    let bob = start_shell();

    // Bob's UI is not listening yet: the Offer must wait for it, not be lost.
    let src = tempfile::tempdir().unwrap();
    let path = src.path().join("note.txt");
    let bytes = "hello from the other app\n".repeat(50_000);
    std::fs::write(&path, &bytes).unwrap();
    let to = bob.device().addr();
    tauri::async_runtime::block_on(alice.device().send_file(to, &path)).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert!(bob.events.try_recv().is_err(), "an event reached a UI that was not ready");

    // Each UI says it is ready, then sees everything from the start, in order.
    for shell in [&alice, &bob] {
        shell.invoke("events_ready", json!({})).unwrap();
    }
    let alice_id = alice.invoke("my_id", json!({})).unwrap();
    assert_eq!(alice_id["id"].as_str().unwrap().len(), 52);
    assert_eq!(alice_id["fingerprint"].as_str().unwrap().len(), 9);

    let offer = bob.wait_event("the Offer", is_transfer("offered"));
    assert_eq!(offer["seq"], 0);
    assert_eq!(offer["role"], "receiver");
    assert_eq!(offer["peer"], alice_id["id"]);
    assert_eq!((offer["name"].as_str(), offer["size"].as_u64()), (Some("note.txt"), Some(bytes.len() as u64)));

    let save_folder = bob.invoke("save_folder", json!({})).unwrap();
    assert_eq!(save_folder, json!(bob.save_dir.to_string_lossy()));

    bob.invoke("accept_offer", json!({ "transferId": offer["transfer_id"] })).unwrap();

    let progress = bob.wait_event("progress", |e| e["type"] == "progress");
    assert_eq!(progress["transfer_id"], offer["transfer_id"]);
    assert_eq!(progress["total"].as_u64(), Some(bytes.len() as u64));
    let done = bob.wait_event("completion", is_transfer("completed"));
    let saved = done["state"]["saved_to"].as_str().unwrap();
    assert_eq!(std::fs::read_to_string(saved).unwrap(), bytes);
    assert!(std::path::Path::new(saved).starts_with(&bob.save_dir));

    alice.wait_event("the Sender's progress", |e| e["type"] == "progress");
    let sent = alice.wait_event("the Sender's completion", is_transfer("completed"));
    assert_eq!(sent["role"], "sender");

    for shell in [&alice, &bob] {
        tauri::async_runtime::block_on(shell.device().shutdown());
    }
}

#[test]
fn a_declined_offer_and_bad_input_are_reported_to_the_ui() {
    let alice = start_shell();
    let bob = start_shell();
    for shell in [&alice, &bob] {
        shell.invoke("events_ready", json!({})).unwrap();
    }
    let src = tempfile::tempdir().unwrap();
    let path = src.path().join("a.txt");
    std::fs::write(&path, "x").unwrap();
    tauri::async_runtime::block_on(alice.device().send_file(bob.device().addr(), &path)).unwrap();

    let offer = bob.wait_event("the Offer", is_transfer("offered"));
    bob.invoke("decline_offer", json!({ "transferId": offer["transfer_id"] })).unwrap();
    bob.wait_event("the decline", is_transfer("declined"));
    alice.wait_event("the Sender hearing it", is_transfer("declined"));
    assert_eq!(std::fs::read_dir(&bob.save_dir).unwrap().count(), 0);

    // Answering again, a malformed Transfer ID and a malformed Device ID are all refused.
    let again = bob.invoke("decline_offer", json!({ "transferId": offer["transfer_id"] }));
    assert!(again.unwrap_err().as_str().unwrap().contains("no pending Offer"));
    assert!(bob.invoke("accept_offer", json!({ "transferId": "nope" })).is_err());
    let bad = alice.invoke("send_file", json!({ "to": "not an id", "path": "/tmp/x" }));
    assert!(bad.unwrap_err().as_str().unwrap().contains("52 characters"));

    for shell in [&alice, &bob] {
        tauri::async_runtime::block_on(shell.device().shutdown());
    }
}
