//! The desktop shell: a thin layer that starts a core [`Device`], exposes its commands to the
//! UI and forwards its event stream. All behaviour lives in `bhayanakshare-core`.
//!
//! Commands map one to one onto `Device` methods and the UI hears exactly the core's events;
//! the TypeScript for both is generated from the Rust types into `ui/src/bindings.ts`
//! (`pnpm bindings`; a test fails when the file is stale).

mod background;
mod notice;

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use bhayanakshare_core::{
    BatchId, Contact, Device, DeviceAddr, DeviceConfig, DeviceId, Event, HistoryEntry, HistoryQuery,
    KeySource, Network, Role, SpaceCheck, SystemClock, SystemFreeSpace, TransferId, Visibility,
};
use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, Manager, Runtime, State};
use tauri_plugin_autostart::AutoLaunchManager;
use tauri_specta::{Builder, ErrorHandlingMode, Event as _, collect_commands, collect_events};

/// Overrides where this install keeps its data (Device ID, database, blobs). Set it to
/// different folders to run several instances on one machine.
const DATA_DIR_VAR: &str = "BHAYANAKSHARE_DATA_DIR";
/// Overrides the folder accepted files are saved to.
const SAVE_DIR_VAR: &str = "BHAYANAKSHARE_SAVE_DIR";

/// How long quitting waits for the Device to save its Transfers' progress.
pub(crate) const QUIT_DEADLINE: Duration = Duration::from_secs(30);

/// Every event the Device emits, in order, as one UI event.
#[derive(Clone, Serialize, Type, tauri_specta::Event)]
#[serde(transparent)]
pub struct DeviceEvent(pub Event);

#[derive(Serialize, Type)]
pub struct MyId {
    /// 52-character base32 Device ID.
    id: String,
    /// First 8 characters, `XXXX-XXXX`.
    fingerprint: String,
}

/// The folder accepted files are saved to (shown on the Offer sheet).
struct SaveFolder(PathBuf);

/// Holds the Device's events back until the UI is listening, so an Offer that arrives while
/// the window is still loading is not lost; after that it passes events straight through.
struct EventGate(Mutex<GateState>);

struct GateState {
    open: bool,
    held: Vec<Event>,
    deliver: Box<dyn Fn(Event) + Send>,
}

impl EventGate {
    fn new(deliver: impl Fn(Event) + Send + 'static) -> Self {
        Self(Mutex::new(GateState { open: false, held: Vec::new(), deliver: Box::new(deliver) }))
    }

    fn send(&self, event: Event) {
        let mut gate = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if gate.open {
            (gate.deliver)(event);
        } else {
            gate.held.push(event);
        }
    }

    /// Delivers what was held, in order, and everything after it directly.
    fn open(&self) {
        let mut gate = self.0.lock().unwrap_or_else(|e| e.into_inner());
        gate.open = true;
        for event in std::mem::take(&mut gate.held) {
            (gate.deliver)(event);
        }
    }
}

#[tauri::command]
#[specta::specta]
fn my_id(device: State<'_, Device>) -> MyId {
    let id = device.device_id();
    MyId { id: id.to_string(), fingerprint: id.fingerprint() }
}

#[tauri::command]
#[specta::specta]
fn save_folder(folder: State<'_, SaveFolder>) -> String {
    folder.0.to_string_lossy().into_owned()
}

/// Offers the files and folders at `paths` to the Device with the pasted Device ID `to`, as
/// one Transfer; resolves to the Transfer ID.
#[tauri::command]
#[specta::specta]
async fn send_files(
    device: State<'_, Device>,
    to: String,
    paths: Vec<String>,
) -> Result<String, String> {
    let to: DeviceId = to.trim().parse().map_err(|e| format!("{e}"))?;
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    let id = device.send(to, &paths).await.map_err(|e| e.to_string())?;
    Ok(id.to_string())
}

/// Offers the files and folders at `paths` to every Device in `to` (pasted or chosen Device
/// IDs) at once, as a Batch of one Transfer each; resolves to the Batch ID.
#[tauri::command]
#[specta::specta]
async fn send_batch(
    device: State<'_, Device>,
    to: Vec<String>,
    paths: Vec<String>,
) -> Result<String, String> {
    let to = to.iter().map(|id| parse_id(id).map(DeviceAddr::from)).collect::<Result<Vec<_>, _>>()?;
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    let sent = device.send_batch(&to, &paths).await.map_err(|e| e.to_string())?;
    Ok(sent.id.to_string())
}

/// Offers `text` to the Device with the pasted Device ID `to`; resolves to the Transfer ID.
#[tauri::command]
#[specta::specta]
async fn send_text(device: State<'_, Device>, to: String, text: String) -> Result<String, String> {
    let id = device.send_text(parse_id(&to)?, &text).await.map_err(|e| e.to_string())?;
    Ok(id.to_string())
}

/// Offers `text` to every Device in `to` at once, as a Batch of one Transfer each; resolves to
/// the Batch ID.
#[tauri::command]
#[specta::specta]
async fn send_text_batch(
    device: State<'_, Device>,
    to: Vec<String>,
    text: String,
) -> Result<String, String> {
    let to = to.iter().map(|id| parse_id(id).map(DeviceAddr::from)).collect::<Result<Vec<_>, _>>()?;
    let sent = device.send_text_batch(&to, &text).await.map_err(|e| e.to_string())?;
    Ok(sent.id.to_string())
}

/// Stops every Transfer of a Batch that is still running.
#[tauri::command]
#[specta::specta]
async fn cancel_batch(device: State<'_, Device>, batch_id: BatchId) -> Result<(), String> {
    device.cancel_batch(batch_id).await.map_err(|e| e.to_string())
}

/// Sends a Failed Transfer of a Batch again, with a new Offer; resolves to the new Transfer ID.
#[tauri::command]
#[specta::specta]
async fn retry_transfer(device: State<'_, Device>, transfer_id: TransferId) -> Result<String, String> {
    let id = device.retry(transfer_id).await.map_err(|e| e.to_string())?;
    Ok(id.to_string())
}

/// Whether a pending Offer fits in `folder` (the save folder when absent).
#[tauri::command]
#[specta::specta]
async fn check_offer(
    device: State<'_, Device>,
    transfer_id: TransferId,
    folder: Option<String>,
) -> Result<SpaceCheck, String> {
    let folder = folder.map(PathBuf::from);
    device.check_offer(transfer_id, folder.as_deref()).await.map_err(|e| e.to_string())
}

/// Accepts a pending Offer into `folder` for this Offer only (the save folder when absent).
#[tauri::command]
#[specta::specta]
async fn accept_offer(
    device: State<'_, Device>,
    transfer_id: TransferId,
    folder: Option<String>,
) -> Result<(), String> {
    let folder = folder.map(PathBuf::from);
    device.accept_into(transfer_id, folder.as_deref()).await.map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
async fn decline_offer(device: State<'_, Device>, transfer_id: TransferId) -> Result<(), String> {
    device.decline(transfer_id).await.map_err(|e| e.to_string())
}

/// Stops a Transfer, on either side, until it starts saving.
#[tauri::command]
#[specta::specta]
async fn cancel_transfer(device: State<'_, Device>, transfer_id: TransferId) -> Result<(), String> {
    device.cancel(transfer_id).await.map_err(|e| e.to_string())
}

/// Sends an expired Offer again; resolves to the new Transfer ID.
#[tauri::command]
#[specta::specta]
async fn resend_transfer(
    device: State<'_, Device>,
    transfer_id: TransferId,
) -> Result<String, String> {
    let id = device.resend(transfer_id).await.map_err(|e| e.to_string())?;
    Ok(id.to_string())
}

/// This Device's name, as other Devices see it.
#[tauri::command]
#[specta::specta]
async fn device_name(device: State<'_, Device>) -> Result<String, String> {
    Ok(device.device_name().await)
}

/// Renames this Device; resolves to the name as stored (trimmed, shortened if too long).
#[tauri::command]
#[specta::specta]
async fn set_device_name(device: State<'_, Device>, name: String) -> Result<String, String> {
    device.set_device_name(&name).await.map_err(|e| e.to_string())
}

/// Who can see this Device as a Nearby Device.
#[tauri::command]
#[specta::specta]
async fn visibility(device: State<'_, Device>) -> Result<Visibility, String> {
    Ok(device.visibility().await)
}

/// Changes who can see this Device as a Nearby Device; it takes effect at once.
#[tauri::command]
#[specta::specta]
async fn set_visibility<R: Runtime>(
    app: AppHandle<R>,
    device: State<'_, Device>,
    visibility: Visibility,
) -> Result<(), String> {
    device.set_visibility(visibility).await.map_err(|e| e.to_string())?;
    background::sync_visibility(&app, visibility);
    Ok(())
}

/// Whether this Device starts when the user logs in.
#[tauri::command]
#[specta::specta]
fn autostart_enabled(autostart: State<'_, AutoLaunchManager>) -> Result<bool, String> {
    autostart.is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
fn set_autostart(autostart: State<'_, AutoLaunchManager>, on: bool) -> Result<(), String> {
    if on { autostart.enable() } else { autostart.disable() }.map_err(|e| e.to_string())
}

/// The user confirmed quitting while Transfers are in progress: save their progress and exit.
#[tauri::command]
#[specta::specta]
async fn quit_app<R: Runtime>(app: AppHandle<R>) {
    background::finish_quit(&app).await;
}

fn parse_id(id: &str) -> Result<DeviceId, String> {
    id.trim().parse().map_err(|e| format!("{e}"))
}

/// Every Contact, in the order they were added.
#[tauri::command]
#[specta::specta]
async fn contacts(device: State<'_, Device>) -> Result<Vec<Contact>, String> {
    device.contacts().await.map_err(|e| e.to_string())
}

/// Saves the Device with the pasted Device ID as a Contact. `device_name` is the name to show
/// until the Contact is given a Nickname. The UI asks the user to check the Fingerprint first.
#[tauri::command]
#[specta::specta]
async fn add_contact(
    device: State<'_, Device>,
    id: String,
    device_name: Option<String>,
) -> Result<Contact, String> {
    let id = parse_id(&id)?;
    device.add_contact(id, device_name.as_deref()).await.map_err(|e| e.to_string())
}

/// Sets a Contact's Nickname; absent or empty goes back to its Device Name.
#[tauri::command]
#[specta::specta]
async fn set_nickname(
    device: State<'_, Device>,
    id: String,
    nickname: Option<String>,
) -> Result<Contact, String> {
    let id = parse_id(&id)?;
    device.set_nickname(id, nickname.as_deref()).await.map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
async fn set_auto_accept(
    device: State<'_, Device>,
    id: String,
    on: bool,
) -> Result<Contact, String> {
    let id = parse_id(&id)?;
    device.set_auto_accept(id, on).await.map_err(|e| e.to_string())
}

/// Forgets a Contact; its Transfer records stay.
#[tauri::command]
#[specta::specta]
async fn remove_contact(device: State<'_, Device>, id: String) -> Result<(), String> {
    let id = parse_id(&id)?;
    device.remove_contact(id).await.map_err(|e| e.to_string())
}

/// Transfer History, newest first, narrowed by whichever are given: the other Device's ID
/// (`peer`), the role this Device played (`direction`) and a search of the item names.
#[tauri::command]
#[specta::specta]
async fn history(
    device: State<'_, Device>,
    peer: Option<String>,
    direction: Option<Role>,
    search: Option<String>,
) -> Result<Vec<HistoryEntry>, String> {
    let device_id = peer.as_deref().map(parse_id).transpose()?;
    let query = HistoryQuery { device: device_id, direction, search };
    device.history(&query).await.map_err(|e| e.to_string())
}

/// Deletes one Transfer that has ended from History.
#[tauri::command]
#[specta::specta]
async fn delete_history_transfer(device: State<'_, Device>, transfer_id: TransferId) -> Result<(), String> {
    device.delete_history_transfer(transfer_id).await.map_err(|e| e.to_string())
}

/// Deletes the Transfers of a Batch that have ended from History; resolves to how many.
#[tauri::command]
#[specta::specta]
async fn delete_history_batch(device: State<'_, Device>, batch_id: BatchId) -> Result<u64, String> {
    device.delete_history_batch(batch_id).await.map_err(|e| e.to_string())
}

/// Clears History of every Transfer that has ended; resolves to how many.
#[tauri::command]
#[specta::specta]
async fn clear_history(device: State<'_, Device>) -> Result<u64, String> {
    device.clear_history().await.map_err(|e| e.to_string())
}

/// Not a Device command: the UI calls it once it is listening for `DeviceEvent`s, and
/// receives everything the Device emitted before that, in order.
#[tauri::command]
#[specta::specta]
fn events_ready(gate: State<'_, EventGate>) {
    gate.open();
}

/// The commands and events the UI sees. Also the source of `ui/src/bindings.ts`.
pub fn specta_builder<R: Runtime>() -> Builder<R> {
    Builder::<R>::new()
        .commands(collect_commands![
            my_id,
            save_folder,
            send_files,
            send_batch,
            send_text,
            send_text_batch,
            cancel_batch,
            retry_transfer,
            check_offer,
            accept_offer,
            decline_offer,
            cancel_transfer,
            resend_transfer,
            device_name,
            set_device_name,
            visibility,
            set_visibility::<tauri::Wry>,
            autostart_enabled,
            set_autostart,
            quit_app::<tauri::Wry>,
            contacts,
            add_contact,
            set_nickname,
            set_auto_accept,
            remove_contact,
            history,
            delete_history_transfer,
            delete_history_batch,
            clear_history,
            events_ready
        ])
        .events(collect_events![DeviceEvent, background::ShellEvent])
        // A rejected command is a rejected promise, not a wrapped result.
        .error_handling(ErrorHandlingMode::Throw)
        // Sizes and timestamps travel as JSON numbers; they stay far below 2^53.
        .dangerously_cast_bigints_to_number()
}

/// Starts the Device, makes it available to the commands and forwards its event stream to
/// the UI.
pub fn start_device<R: Runtime>(
    app: &impl Manager<R>,
    config: DeviceConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    // Shown to the user as the Device resolves it.
    app.manage(SaveFolder(std::path::absolute(&config.save_dir)?));
    let (device, mut events) = tauri::async_runtime::block_on(Device::start(config))?;
    app.manage(device);

    let handle = app.app_handle().clone();
    app.manage(EventGate::new({
        let handle = handle.clone();
        move |event| {
            if let Err(e) = DeviceEvent(event).emit(&handle) {
                tracing::warn!("could not forward a Device event: {e}");
            }
        }
    }));
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.next().await {
            handle.state::<EventGate>().send(event.clone());
            notice::notify(&handle, &event).await;
        }
    });
    Ok(())
}

/// Where this install keeps its data and saves received files.
fn default_config<R: Runtime>(app: &impl Manager<R>) -> Result<DeviceConfig, tauri::Error> {
    let data_dir = match std::env::var_os(DATA_DIR_VAR) {
        Some(dir) => PathBuf::from(dir),
        None => app.path().app_data_dir()?,
    };
    let save_dir = match std::env::var_os(SAVE_DIR_VAR) {
        Some(dir) => PathBuf::from(dir),
        None => app.path().download_dir()?.join("BhayanakShare"),
    };
    Ok(DeviceConfig {
        key_source: KeySource::File(data_dir.join("secret.key")),
        data_dir,
        save_dir,
        clock: Arc::new(SystemClock),
        network: Network::Internet,
        free_space: Arc::new(SystemFreeSpace),
    })
}

/// WebKitGTK leaves the camera off, and refuses it when asked, unless the app says otherwise.
/// It is only asked for (by "Scan QR code…") when the user chooses to scan one.
#[cfg(target_os = "linux")]
fn allow_camera<R: Runtime>(app: &AppHandle<R>) {
    use webkit2gtk::{
        PermissionRequestExt, SettingsExt, UserMediaPermissionRequest,
        UserMediaPermissionRequestExt, WebViewExt, glib::prelude::Cast,
    };
    let Some(window) = app.get_webview_window("main") else { return };
    let set = window.with_webview(|webview| {
        let view = webview.inner();
        if let Some(settings) = view.settings() {
            settings.set_enable_media_stream(true);
        }
        view.connect_permission_request(|_, request| {
            match request.downcast_ref::<UserMediaPermissionRequest>() {
                Some(media) if media.is_for_video_device() && !media.is_for_audio_device() => {
                    request.allow();
                    true
                }
                _ => false,
            }
        });
    });
    if let Err(e) = set {
        tracing::warn!("could not allow the camera: {e}");
    }
}

pub fn run() {
    let builder = specta_builder();
    // An instance with its own data folder is a separate install, made to run beside another
    // one (see the README), so it neither takes part in single instance nor registers itself
    // to start at login.
    let separate = std::env::var_os(DATA_DIR_VAR).is_some();
    let mut app = tauri::Builder::default();
    if !separate {
        // Registered first, as the plugin asks: a second launch ends in its setup, handing
        // its arguments to the running instance.
        app = app.plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            background::second_launch(app, &argv, &cwd)
        }));
    }
    app = app
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_autostart::Builder::new().arg(background::BACKGROUND_FLAG).build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init());
    #[cfg(not(target_os = "linux"))]
    {
        app = app.plugin(tauri_plugin_notification::init());
    }
    app.on_window_event(|window, event| background::on_window_event(window, event))
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            app.manage(background::Quitting::default());
            app.manage(notice::Notifier::default());
            start_device(app, default_config(app)?)?;

            let handle = app.handle();
            let visibility = tauri::async_runtime::block_on(app.state::<Device>().visibility());
            if let Err(e) = background::build_tray(handle, visibility) {
                tracing::warn!("could not create the tray icon: {e}");
            }
            background::show_on_link(handle);
            if !separate {
                background::default_autostart(handle);
                #[cfg(target_os = "linux")]
                background::register_links(handle);
            }
            #[cfg(target_os = "linux")]
            allow_camera(handle);
            // Starting at login leaves the window closed, in the tray.
            if !std::env::args().any(|arg| arg == background::BACKGROUND_FLAG) {
                background::show_main(handle);
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building BhayanakShare")
        .run(|app, event| {
            // Let the Device close its stores cleanly so a restart does not re-hash them. A
            // busy disk can make that slow (spec section 7: "Saving progress…" for up to 30
            // s); after that the process exits anyway, and the next start re-checks.
            if let tauri::RunEvent::Exit = event {
                if let Some(device) = app.try_state::<Device>() {
                    tauri::async_runtime::block_on(device.shutdown(QUIT_DEADLINE));
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use specta_typescript::Typescript;

    use super::*;

    use bhayanakshare_core::EventKind;

    const BINDINGS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ui/src/bindings.ts");

    fn progress(seq: u64) -> Event {
        let transfer_id = TransferId::from_bytes([1; 16]);
        Event {
            seq,
            at: 0,
            kind: EventKind::Progress(bhayanakshare_core::ProgressEvent {
                transfer_id,
                bytes: seq,
                total: 10,
            }),
        }
    }

    #[test]
    fn the_gate_holds_events_until_opened_then_keeps_order() {
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let gate = EventGate::new({
            let delivered = delivered.clone();
            move |e: Event| delivered.lock().unwrap().push(e.seq)
        });

        gate.send(progress(0));
        gate.send(progress(1));
        assert!(delivered.lock().unwrap().is_empty());

        gate.open();
        gate.send(progress(2));
        assert_eq!(*delivered.lock().unwrap(), [0, 1, 2]);

        gate.open(); // a reloaded window asks again: nothing is delivered twice
        assert_eq!(*delivered.lock().unwrap(), [0, 1, 2]);
    }

    fn generate(to: &Path) {
        specta_builder::<tauri::Wry>().export(Typescript::default(), to).unwrap();
    }

    /// Run by `pnpm bindings`: rewrites the generated UI types.
    #[test]
    #[ignore = "writes ui/src/bindings.ts; run through `pnpm bindings`"]
    fn export_ui_bindings() {
        generate(Path::new(BINDINGS));
    }

    #[test]
    fn ui_bindings_match_the_rust_types() {
        let fresh = std::env::temp_dir().join(format!("bindings-{}.ts", std::process::id()));
        generate(&fresh);
        let fresh_text = std::fs::read_to_string(&fresh).unwrap();
        let _ = std::fs::remove_file(&fresh);

        let committed = std::fs::read_to_string(BINDINGS).unwrap_or_default();
        assert!(
            committed == fresh_text,
            "ui/src/bindings.ts is out of date; run `pnpm bindings` and commit the result"
        );
    }
}
