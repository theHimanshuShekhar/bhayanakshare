//! Updates (spec section 9): a check for a newer release at startup and every 24 hours, and
//! installing it once the user has agreed.
//!
//! The check is a plain GET of `latest.json` on GitHub Releases (the endpoint is in
//! `tauri.conf.json`): no Device ID, no user data. An AppImage can replace itself with the new
//! release, signed with the update key; a deb or rpm install never updates itself, and is only
//! pointed at the release page. Nothing is installed without the UI asking for it.
//!
//! What to do about a release is [`decide`], a pure function. The plugin and the network stay at
//! the edge, in [`check`] and [`install_update`].

use std::{
    ffi::OsStr,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use semver::Version;
use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, Manager, Runtime, State};
use tauri_plugin_updater::{Update, UpdaterExt as _};

use crate::{Relaunch, background};

/// How often the running app looks for a new release.
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// A check that gets no answer in this long has failed, so "Update now" cannot hang.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

/// How this copy of BhayanakShare was installed, which decides how it can be updated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallKind {
    /// Run from an AppImage, which can replace itself.
    AppImage,
    /// Installed by a package manager (deb, rpm), or anything else that is not an AppImage:
    /// updated by the user, from the release page.
    Package,
}

impl InstallKind {
    /// The kind of this run, from the `APPIMAGE` variable an AppImage's launcher sets.
    pub fn current() -> Self {
        Self::detect(std::env::var_os("APPIMAGE").as_deref())
    }

    /// An AppImage has `APPIMAGE` (the path of the file) set, and non-empty.
    fn detect(appimage: Option<&OsStr>) -> Self {
        match appimage {
            Some(path) if !path.is_empty() => Self::AppImage,
            _ => Self::Package,
        }
    }
}

/// What to do about the latest release.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UpdateAction {
    /// Nothing newer, or nothing that could be compared.
    None,
    /// Offer to install `version` and restart: an AppImage.
    Install { version: String },
    /// Offer a link to the release page for `version`: a package.
    OpenPage { version: String },
}

/// What to do when `latest` is the newest release and `current` is what is running. Versions
/// are compared as semver; one that is not a version leaves nothing to do.
pub fn decide(current: &str, latest: &str, kind: InstallKind) -> UpdateAction {
    let (Ok(current), Ok(latest)) = (Version::parse(current), Version::parse(latest)) else {
        return UpdateAction::None;
    };
    if latest <= current {
        return UpdateAction::None;
    }
    let version = latest.to_string();
    match kind {
        InstallKind::AppImage => UpdateAction::Install { version },
        InstallKind::Package => UpdateAction::OpenPage { version },
    }
}

/// The newest release found so far that is newer than this one, and the plugin's handle on it.
#[derive(Default)]
pub struct Updates {
    found: Mutex<Option<(UpdateAction, Update)>>,
    /// Set while an install runs, so that a second click does not start another.
    installing: AtomicBool,
}

impl Updates {
    fn action(&self) -> UpdateAction {
        match &*self.found.lock().unwrap() {
            Some((action, _)) => action.clone(),
            None => UpdateAction::None,
        }
    }
}

/// Looks at `latest.json` once and remembers what it found. A failed check says why, as a
/// message, and leaves what was found before.
async fn check<R: Runtime>(app: &AppHandle<R>) -> Result<UpdateAction, String> {
    let updater = app
        .updater_builder()
        .timeout(CHECK_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;
    let release = updater.check().await.map_err(|e| e.to_string())?;
    let action = match &release {
        Some(update) => decide(&update.current_version, &update.version, InstallKind::current()),
        None => UpdateAction::None,
    };
    let found = match release {
        Some(update) if action != UpdateAction::None => Some((action.clone(), update)),
        _ => None,
    };
    *app.state::<Updates>().found.lock().unwrap() = found;
    Ok(action)
}

/// A check that tells the UI when it finds something. A failure (offline, no release yet, a key
/// that is not a key) is logged and ignored: the next check is in a day.
async fn check_and_announce<R: Runtime>(app: &AppHandle<R>) {
    match check(app).await {
        Ok(UpdateAction::None) => {}
        Ok(action) => {
            tracing::info!("an update is available: {action:?}");
            background::emit(app, background::ShellEvent::UpdateAvailable { action });
        }
        Err(e) => tracing::warn!("could not check for updates: {e}"),
    }
}

/// Checks now, and then every 24 hours for as long as the app runs.
pub fn spawn_checks<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            check_and_announce(&app).await;
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

/// Looks for a newer release now (the "Update now" button). Rejects when the check fails, as it
/// does offline.
#[tauri::command]
#[specta::specta]
pub async fn check_for_update<R: Runtime>(app: AppHandle<R>) -> Result<UpdateAction, String> {
    check(&app).await
}

/// The newer release the last check found, if any: what the UI asks for when it opens, as a
/// check at startup may have finished before it was listening.
#[tauri::command]
#[specta::specta]
pub fn pending_update(updates: State<'_, Updates>) -> UpdateAction {
    updates.action()
}

/// Downloads the release the last check found, replaces this AppImage with it (the plugin checks
/// the signature first), and starts the app again by the way out of quitting, so that the
/// Device saves its Transfers' progress: they resume in the new version. The UI calls this only
/// once the user has agreed. A deb or rpm install is refused: those are never updated in place.
#[tauri::command]
#[specta::specta]
pub async fn install_update<R: Runtime>(app: AppHandle<R>, updates: State<'_, Updates>) -> Result<(), String> {
    if InstallKind::current() != InstallKind::AppImage {
        return Err("this install is updated with its package manager or from the release page".into());
    }
    let update = match &*updates.found.lock().unwrap() {
        Some((_, update)) => update.clone(),
        None => return Err("there is no update to install".into()),
    };
    if updates.installing.swap(true, Ordering::SeqCst) {
        return Err("an update is already being installed".into());
    }
    let installed = update.download_and_install(|_, _| {}, || {}).await;
    if let Err(e) = installed {
        updates.installing.store(false, Ordering::SeqCst);
        tracing::warn!("could not install the update: {e}");
        return Err(e.to_string());
    }
    tracing::info!("installed version {}, restarting", update.version);
    app.state::<Relaunch>().0.store(true, Ordering::SeqCst);
    background::finish_quit(&app).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use tauri::test::{mock_builder, mock_context, noop_assets};

    use super::*;

    fn install(version: &str) -> UpdateAction {
        UpdateAction::Install { version: version.into() }
    }

    fn page(version: &str) -> UpdateAction {
        UpdateAction::OpenPage { version: version.into() }
    }

    #[test]
    fn an_appimage_is_told_apart_by_its_variable() {
        assert_eq!(InstallKind::detect(Some(OsStr::new("/home/me/BhayanakShare.AppImage"))), InstallKind::AppImage);
        // Anything else on Linux is a package: deb, rpm, a build run from a checkout.
        assert_eq!(InstallKind::detect(None), InstallKind::Package);
        assert_eq!(InstallKind::detect(Some(OsStr::new(""))), InstallKind::Package);
    }

    #[test]
    fn a_newer_release_is_installed_by_an_appimage_and_linked_to_for_a_package() {
        assert_eq!(decide("0.1.0", "0.2.0", InstallKind::AppImage), install("0.2.0"));
        assert_eq!(decide("0.1.0", "0.2.0", InstallKind::Package), page("0.2.0"));
        assert_eq!(decide("0.9.0", "0.10.0", InstallKind::AppImage), install("0.10.0"));
        assert_eq!(decide("0.1.0", "1.0.0-beta.1", InstallKind::Package), page("1.0.0-beta.1"));
    }

    #[test]
    fn the_same_or_an_older_release_is_nothing_to_do() {
        for kind in [InstallKind::AppImage, InstallKind::Package] {
            assert_eq!(decide("0.2.0", "0.2.0", kind), UpdateAction::None);
            assert_eq!(decide("0.2.0", "0.1.9", kind), UpdateAction::None);
            // A release candidate comes before its release.
            assert_eq!(decide("1.0.0", "1.0.0-rc.1", kind), UpdateAction::None);
        }
    }

    #[test]
    fn something_that_is_not_a_version_is_nothing_to_do() {
        assert_eq!(decide("0.1.0", "latest", InstallKind::AppImage), UpdateAction::None);
        assert_eq!(decide("", "0.2.0", InstallKind::Package), UpdateAction::None);
    }

    #[test]
    fn nothing_is_pending_until_a_check_finds_something() {
        assert_eq!(Updates::default().action(), UpdateAction::None);
    }

    fn tauri_conf() -> Value {
        serde_json::from_str(include_str!("../tauri.conf.json")).unwrap()
    }

    #[test]
    fn the_release_config_is_what_the_updater_and_the_packages_need() {
        let conf = tauri_conf();
        let updater = &conf["plugins"]["updater"];
        assert_eq!(
            updater["endpoints"],
            serde_json::json!(["https://github.com/theHimanshuShekhar/bhayanakshare/releases/latest/download/latest.json"]),
        );
        assert!(updater["pubkey"].as_str().is_some_and(|key| !key.is_empty()));
        assert_eq!(conf["bundle"]["createUpdaterArtifacts"], true);
        // The deep-link plugin takes the desktop file's scheme handler from here.
        assert_eq!(conf["plugins"]["deep-link"]["desktop"]["schemes"], serde_json::json!(["bhayanakshare"]));
        // The packaged desktop file hands the link over (`%u`); the scheme itself is filled in by
        // the bundler from the config above.
        let desktop = include_str!("../linux/bhayanakshare.desktop");
        assert!(desktop.contains("Exec={{exec}} %u") && desktop.contains("MimeType={{mime_type}}"));
        for kind in ["deb", "rpm"] {
            assert_eq!(conf["bundle"]["linux"][kind]["desktopTemplate"], "linux/bhayanakshare.desktop");
        }
        // Packages never change the firewall: no install or removal scripts at all.
        for kind in ["deb", "rpm"] {
            let package = &conf["bundle"]["linux"][kind];
            for script in ["preInstallScript", "postInstallScript", "preRemoveScript", "postRemoveScript"] {
                assert!(package.get(script).is_none(), "{kind} has a {script}");
            }
        }
    }

    #[test]
    fn an_update_key_that_is_not_a_key_does_not_stop_the_updater_from_starting() {
        // A build whose config has a broken key must still start, and only fail to install.
        let mut updater = tauri_conf()["plugins"]["updater"].clone();
        updater["pubkey"] = "not a key".into();
        let mut context = mock_context(noop_assets());
        context.config_mut().plugins.0.insert("updater".into(), updater);
        let app = mock_builder()
            .plugin(tauri_plugin_updater::Builder::new().build())
            .build(context)
            .expect("the app starts");
        // Making an updater needs no valid key; only installing something does.
        assert!(app.handle().updater().is_ok());
    }
}
