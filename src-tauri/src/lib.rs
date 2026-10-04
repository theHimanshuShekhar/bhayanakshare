//! The desktop shell: a thin layer that starts a core [`Device`], exposes its commands to the
//! UI and forwards its event stream. All behaviour lives in `bhayanakshare-core`.

use std::{path::PathBuf, sync::Arc};

use bhayanakshare_core::{
    Device, DeviceConfig, DeviceId, KeySource, Network, SystemClock, TransferId,
};
use serde::Serialize;
use tauri::{Emitter, Manager, State};

/// The event the UI listens to; its payload is a core `Event`.
const DEVICE_EVENT: &str = "device-event";

#[derive(Serialize)]
struct MyId {
    id: String,
    fingerprint: String,
}

#[tauri::command]
fn my_id(device: State<'_, Device>) -> MyId {
    let id = device.device_id();
    MyId { id: id.to_string(), fingerprint: id.fingerprint() }
}

/// Offers the file at `path` to the Device with the pasted Device ID `to`.
#[tauri::command]
async fn send_file(device: State<'_, Device>, to: String, path: String) -> Result<String, String> {
    let to: DeviceId = to.trim().parse().map_err(|e| format!("{e}"))?;
    let id = device.send_file(to, &PathBuf::from(path)).await.map_err(|e| e.to_string())?;
    Ok(id.to_string())
}

#[tauri::command]
async fn accept_offer(device: State<'_, Device>, transfer_id: String) -> Result<(), String> {
    device.accept(parse_transfer_id(&transfer_id)?).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn decline_offer(device: State<'_, Device>, transfer_id: String) -> Result<(), String> {
    device.decline(parse_transfer_id(&transfer_id)?).await.map_err(|e| e.to_string())
}

fn parse_transfer_id(text: &str) -> Result<TransferId, String> {
    text.parse().map_err(|()| "not a Transfer ID".to_owned())
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let save_dir = app.path().download_dir()?.join("BhayanakShare");
            let config = DeviceConfig {
                key_source: KeySource::File(data_dir.join("secret.key")),
                data_dir,
                save_dir,
                clock: Arc::new(SystemClock),
                network: Network::Internet,
            };
            let (device, mut events) =
                tauri::async_runtime::block_on(Device::start(config))?;
            app.manage(device);

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                while let Some(event) = events.next().await {
                    if let Err(e) = handle.emit(DEVICE_EVENT, &event) {
                        tracing::warn!("could not forward a Device event: {e}");
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![my_id, send_file, accept_offer, decline_offer])
        .build(tauri::generate_context!())
        .expect("error while building BhayanakShare")
        .run(|app, event| {
            // Let the Device close its stores cleanly so a restart does not re-hash them.
            if let tauri::RunEvent::Exit = event {
                if let Some(device) = app.try_state::<Device>() {
                    tauri::async_runtime::block_on(device.shutdown());
                }
            }
        });
}
