//! Staying reachable in the background (spec section 7): the tray, closing to the tray, a
//! second launch, start at login and quitting safely.
//!
//! Linux sends no click events for a tray icon, so everything the tray offers is in its menu.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use bhayanakshare_core::{Device, TransferId, Visibility};
use serde::Serialize;
use specta::Type;
use tauri::{
    AppHandle, Manager, Runtime, Window, WindowEvent,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::TrayIconBuilder,
};
use tauri_plugin_deep_link::DeepLinkExt as _;
use tauri_plugin_dialog::DialogExt as _;
use tauri_specta::Event as _;

use crate::{Logging, QUIT_DEADLINE, autostart::Autostart, notice, updates::UpdateAction};

/// Passed by the login entry, so that starting at login leaves the window closed.
pub const BACKGROUND_FLAG: &str = "--background";

/// Set once the user has been told, the first time the window was closed, that the app keeps
/// running in the tray.
const TRAY_NOTICE_SETTING: &str = "tray_notice_shown";
/// Set once start at login has been switched on by default, so that switching it off sticks.
pub(crate) const AUTOSTART_SETTING: &str = "autostart_defaulted";

/// What the shell tells the UI that is not a Device event.
#[derive(Clone, Serialize, Type, tauri_specta::Event)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ShellEvent {
    /// Files to send, from a second launch or the tray's "Send files…"; the user still has to
    /// say to whom.
    SendFiles { paths: Vec<String> },
    /// A notification about this incoming Offer was clicked.
    OpenOffer { transfer_id: TransferId },
    /// Quit was chosen while `active` Transfers are in progress; the UI asks, and answers with
    /// the `quit_app` command.
    ConfirmQuit { active: u32 },
    /// Shutdown has begun: the Device is saving its progress, which can take a while.
    Quitting,
    /// A check found a release newer than this one; `action` is what to offer for it.
    UpdateAvailable { action: UpdateAction },
}

/// Set once the user has confirmed quitting.
#[derive(Default)]
pub struct Quitting(AtomicBool);

/// Whether the user has confirmed quitting, so that the app is on its way out.
pub fn is_quitting<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.state::<Quitting>().0.load(Ordering::SeqCst)
}

/// Glue between the tray's Visibility choices and the Visibility the Device holds.
struct TrayVisibility<R: Runtime>(Vec<(Visibility, CheckMenuItem<R>)>);

const VISIBILITY_CHOICES: [(Visibility, &str, &str); 3] = [
    (Visibility::Everyone, "visibility-everyone", "Everyone"),
    (Visibility::IdHolders, "visibility-id-holders", "People who have my ID"),
    (Visibility::Hidden, "visibility-hidden", "Hidden"),
];

/// Brings the main window forward, from the tray, a notification or a second launch.
pub fn show_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// A second launch: focus the window and pass on any files it was given.
pub fn second_launch<R: Runtime>(app: &AppHandle<R>, argv: &[String], cwd: &str) {
    show_main(app);
    let paths = files_in(argv, Path::new(cwd));
    if !paths.is_empty() {
        emit(app, ShellEvent::SendFiles { paths });
    }
}

/// The existing files among a launch's arguments (the first is the program), made absolute
/// against the folder it was launched from. Flags are not files.
fn files_in(argv: &[String], cwd: &Path) -> Vec<String> {
    argv.iter()
        .skip(1)
        .filter(|arg| !arg.starts_with("--"))
        .map(|arg| cwd.join(arg))
        .filter(|path| path.exists())
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

/// Brings the window forward whenever a `bhayanakshare://` link is opened (where that does not
/// start a second launch, as on macOS). The UI reads the links itself, the one that started the
/// app included.
pub fn show_on_link<R: Runtime>(app: &AppHandle<R>) {
    let handle = app.clone();
    app.deep_link().on_open_url(move |_| show_main(&handle));
}

/// Registers the `bhayanakshare://` scheme with the desktop at every start, so that an AppImage
/// that was moved, or never installed, still gets its links. The bundles of the other platforms
/// register it when they are installed: the Windows installer does so per user, under HKCU, and
/// a portable copy of the exe is left unregistered on purpose, as it would take the links from
/// the installed one.
#[cfg(target_os = "linux")]
pub fn register_links<R: Runtime>(app: &AppHandle<R>) {
    if let Err(e) = app.deep_link().register_all() {
        tracing::warn!("could not register the bhayanakshare:// links: {e}");
    }
}

pub(crate) fn emit<R: Runtime>(app: &AppHandle<R>, event: ShellEvent) {
    if let Err(e) = event.emit(app) {
        tracing::warn!("could not tell the UI: {e}");
    }
}

/// Tells the UI an Offer's notification was clicked, after bringing the window forward.
pub fn open_offer<R: Runtime>(app: &AppHandle<R>, transfer_id: TransferId) {
    show_main(app);
    emit(app, ShellEvent::OpenOffer { transfer_id });
}

/// Closing the main window hides it; the app keeps running to receive files. The first time,
/// say so.
pub fn on_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    let WindowEvent::CloseRequested { api, .. } = event else { return };
    if window.label() != "main" || window.state::<Quitting>().0.load(Ordering::SeqCst) {
        return;
    }
    api.prevent_close();
    let _ = window.hide();
    let app = window.app_handle().clone();
    tauri::async_runtime::spawn(async move {
        let Some(device) = app.try_state::<Device>() else { return };
        if matches!(device.setting(TRAY_NOTICE_SETTING).await, Ok(None)) {
            let _ = device.set_setting(TRAY_NOTICE_SETTING, "1").await;
            notice::show(&app, notice::TRAY_TITLE, notice::TRAY_BODY, None);
        }
    });
}

/// Builds the tray icon and its menu: Open, Send files…, Visibility, Quit.
pub fn build_tray<R: Runtime>(app: &AppHandle<R>, visibility: Visibility) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open", true, None::<&str>)?;
    let send = MenuItem::with_id(app, "send", "Send files…", true, None::<&str>)?;
    let mut choices = Vec::new();
    for (value, id, label) in VISIBILITY_CHOICES {
        choices.push((value, CheckMenuItem::with_id(app, id, label, true, value == visibility, None::<&str>)?));
    }
    let items: Vec<&dyn tauri::menu::IsMenuItem<R>> =
        choices.iter().map(|(_, item)| item as &dyn tauri::menu::IsMenuItem<R>).collect();
    let visibility_menu = Submenu::with_items(app, "Visibility", true, &items)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&open, &send, &visibility_menu, &PredefinedMenuItem::separator(app)?, &quit],
    )?;
    app.manage(TrayVisibility(choices));

    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("BhayanakShare")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main(app),
            "send" => pick_files(app),
            "quit" => request_quit(app),
            id => {
                if let Some((value, _, _)) = VISIBILITY_CHOICES.iter().find(|(_, c, _)| *c == id) {
                    choose_visibility(app, *value);
                }
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// The tray's "Send files…": ask for files, then let the UI ask to whom.
fn pick_files<R: Runtime>(app: &AppHandle<R>) {
    show_main(app);
    let handle = app.clone();
    app.dialog().file().pick_files(move |picked| {
        let paths: Vec<String> = picked
            .unwrap_or_default()
            .into_iter()
            .filter_map(|file| file.into_path().ok())
            .map(|path: PathBuf| path.to_string_lossy().into_owned())
            .collect();
        if !paths.is_empty() {
            emit(&handle, ShellEvent::SendFiles { paths });
        }
    });
}

/// A Visibility chosen in the tray menu.
fn choose_visibility<R: Runtime>(app: &AppHandle<R>, visibility: Visibility) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(device) = app.try_state::<Device>() else { return };
        if let Err(e) = device.set_visibility(visibility).await {
            tracing::warn!("could not change Visibility from the tray: {e}");
        }
        // Whatever the outcome, show what the Device now has: ticking an item toggles it
        // on its own.
        sync_visibility(&app, device.visibility().await);
    });
}

/// Marks the tray's Visibility menu to match `visibility`.
pub fn sync_visibility<R: Runtime>(app: &AppHandle<R>, visibility: Visibility) {
    if let Some(tray) = app.try_state::<TrayVisibility<R>>() {
        for (value, item) in &tray.0 {
            let _ = item.set_checked(*value == visibility);
        }
    }
}

/// Switches start at login on, once, the first time this Device runs: it is the default, and
/// switching it off afterwards sticks.
pub fn default_autostart<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(device) = app.try_state::<Device>() else { return };
        if !matches!(device.setting(AUTOSTART_SETTING).await, Ok(None)) {
            return;
        }
        let Some(autostart) = app.try_state::<Autostart>() else { return };
        match autostart.enable() {
            Ok(()) => {
                let _ = device.set_setting(AUTOSTART_SETTING, "1").await;
            }
            Err(e) => tracing::warn!("could not switch on start at login: {e}"),
        }
    });
}

/// How many Transfers are in progress: the ones a clean shutdown leaves to resume.
pub async fn in_progress(device: &Device) -> usize {
    match device.transfers().await {
        Ok(all) => all.iter().filter(|t| t.state.is_in_progress()).count(),
        Err(e) => {
            tracing::warn!("could not count the Transfers in progress: {e}");
            0
        }
    }
}

/// Quit was chosen. With Transfers in progress the UI is asked to confirm first; otherwise the
/// Device shuts down at once.
pub fn request_quit<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let active = match app.try_state::<Device>() {
            Some(device) => in_progress(&device).await,
            None => 0,
        };
        if active == 0 {
            finish_quit(&app).await;
        } else {
            show_main(&app);
            emit(&app, ShellEvent::ConfirmQuit { active: active as u32 });
        }
    });
}

/// Marks the app as on its way out and tells the UI, so that it says "Saving progress…".
/// False if that had already been done, by whoever is quitting.
pub fn begin_quit<R: Runtime>(app: &AppHandle<R>) -> bool {
    if app.state::<Quitting>().0.swap(true, Ordering::SeqCst) {
        return false;
    }
    emit(app, ShellEvent::Quitting);
    true
}

/// Shuts the Device down so that its Transfers resume on the next start, for at most
/// [`QUIT_DEADLINE`] (after which the process may end anyway: the next start re-checks), and
/// writes out the log. Safe to repeat: the exit does it again.
pub async fn save_and_flush<R: Runtime>(app: &AppHandle<R>) {
    if let Some(device) = app.try_state::<Device>() {
        device.shutdown(QUIT_DEADLINE).await;
    }
    if let Some(logging) = app.try_state::<Logging>() {
        logging.flush();
    }
}

/// Saves the Transfers' progress (see [`save_and_flush`]), then exits. The window stays up
/// meanwhile, saying so.
pub async fn finish_quit<R: Runtime>(app: &AppHandle<R>) {
    if !begin_quit(app) {
        return;
    }
    save_and_flush(app).await;
    app.exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_launch_passes_on_the_files_it_names_made_absolute_and_nothing_else() {
        let dir = std::env::temp_dir().join(format!("bhs-launch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        std::fs::write(dir.join("b.txt"), "b").unwrap();

        let found = files_in(
            &args(&["/usr/bin/bhayanakshare", "--background", "a.txt", "gone.txt", "b.txt"]),
            &dir,
        );
        let expected = [dir.join("a.txt"), dir.join("b.txt")].map(|p| p.to_string_lossy().into_owned());
        assert_eq!(found, expected);

        // An absolute argument stays as it is, whatever the folder.
        let absolute = dir.join("a.txt").to_string_lossy().into_owned();
        assert_eq!(files_in(&args(&["bhayanakshare", &absolute]), Path::new("/nowhere")), [absolute]);
        assert!(files_in(&args(&["bhayanakshare"]), &dir).is_empty());
        // A link is for the deep-link plugin, not a file to send.
        let link = format!("bhayanakshare://add/{}", "A".repeat(52));
        assert!(files_in(&args(&["bhayanakshare", &link]), &dir).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What a second launch from the desktop's link handler hands over, on Windows: the exe's
    /// path first, then the link as the only argument. The single instance plugin gives these
    /// arguments to the deep-link plugin before `second_launch` runs.
    #[test]
    fn a_link_handed_over_by_a_second_launch_reaches_the_listeners_and_is_not_a_file() {
        use std::sync::{Arc, Mutex};

        use tauri::test::{mock_builder, mock_context, noop_assets};

        let link = format!("bhayanakshare://add/{}?name=Test%20PC", "A".repeat(52));
        let mut context = mock_context(noop_assets());
        context.config_mut().plugins.0.insert(
            "deep-link".into(),
            serde_json::json!({ "desktop": { "schemes": ["bhayanakshare"] } }),
        );
        let app = mock_builder().plugin(tauri_plugin_deep_link::init()).build(context).unwrap();
        let handle = app.handle();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        handle.deep_link().on_open_url(move |event| {
            sink.lock().unwrap().extend(event.urls().iter().map(|url| url.to_string()));
        });
        show_on_link(handle);

        let exe = r"C:\Users\Test User\AppData\Local\BhayanakShare\bhayanakshare.exe";
        // A launch with other arguments, such as files to send, is not a link.
        handle.deep_link().handle_cli_arguments([exe, "--background", "a.txt"].iter());
        assert!(seen.lock().unwrap().is_empty());

        let argv = args(&[exe, &link]);
        handle.deep_link().handle_cli_arguments(argv.iter());
        second_launch(handle, &argv, r"C:\Users\Test User");
        assert_eq!(*seen.lock().unwrap(), [link.clone()]);
        let current = handle.deep_link().get_current().unwrap().unwrap();
        assert_eq!(current.iter().map(|url| url.to_string()).collect::<Vec<_>>(), [link]);
    }
}
