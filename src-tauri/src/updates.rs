//! Updates (spec section 9): a check for a newer release at startup and about daily, and
//! installing it once the user has agreed.
//!
//! The check is a plain GET of `latest.json` on GitHub Releases (the endpoint is in
//! `tauri.conf.json`): no Device ID, no user data. An AppImage can replace itself with the new
//! release, signed with the update key; a deb or rpm install never updates itself, and is only
//! pointed at the release page. Nothing is installed without the UI asking for it, for the
//! version the user was shown.
//!
//! What to do about a release is [`decide`], when to look is [`due`], and whether an install may
//! start is [`may_install`]: pure functions. The plugin and the network stay at the edge, in
//! [`check`] and [`install_update`].

use std::{
    ffi::OsStr,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime},
};

use semver::Version;
use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, Manager, Runtime, State};
use tauri_plugin_updater::{Update, UpdaterExt as _};

use crate::{Relaunch, background};

/// How long after one successful check the next is due.
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// How long after a failed check (the network was not up yet) the next is due.
const RETRY_AFTER: Duration = Duration::from_secs(60 * 60);
/// How often the checking task wakes to see whether a check is due. A sleep of a day would
/// stop while the machine is suspended; waking hourly and looking at the wall clock does not.
const WAKE_EVERY: Duration = Duration::from_secs(60 * 60);
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

impl UpdateAction {
    /// The version this offers, if it offers anything.
    fn version(&self) -> Option<&str> {
        match self {
            Self::None => None,
            Self::Install { version } | Self::OpenPage { version } => Some(version),
        }
    }
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

/// Whether it is time to look for a release: a day of wall-clock time after the last successful
/// check (or there has been none), and an hour after the last failed one. The wall clock, not a
/// timer, because a timer does not run while the machine sleeps. A clock that went back counts
/// as time passed: better a check too many than none.
pub fn due(last_success: Option<SystemTime>, last_failure: Option<SystemTime>, now: SystemTime) -> bool {
    let since = |then: SystemTime| now.duration_since(then).unwrap_or(Duration::MAX);
    last_success.is_none_or(|at| since(at) >= CHECK_EVERY) && last_failure.is_none_or(|at| since(at) >= RETRY_AFTER)
}

/// Why an update was not installed, in the terms the UI words (`update.error.*` in `i18n.ts`).
/// Deliberately no message: the plugin's are English, and can name the AppImage's path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum UpdateError {
    /// This install is a package, which is never updated in place.
    NotAppImage,
    /// No newer release has been found.
    NonePending,
    /// The release found is no longer the version the user agreed to: a newer check replaced it.
    VersionChanged,
    /// Another install is running.
    AlreadyInstalling,
    /// The app is already quitting.
    Quitting,
    /// The download failed, as it does offline.
    DownloadFailed,
    /// The download was not signed by the update key, so it was not installed.
    SignatureInvalid,
    /// Replacing the AppImage failed.
    InstallFailed,
}

impl From<&tauri_plugin_updater::Error> for UpdateError {
    fn from(e: &tauri_plugin_updater::Error) -> Self {
        use tauri_plugin_updater::Error as E;
        match e {
            E::Reqwest(_) | E::Network(_) | E::Http(_) => Self::DownloadFailed,
            E::Minisign(_)
            | E::Base64(_)
            | E::SignatureUtf8(_)
            | E::SignedVersionMismatch { .. }
            | E::MissingSignedVersion => Self::SignatureInvalid,
            _ => Self::InstallFailed,
        }
    }
}

/// Whether installing `agreed`, the version the user was shown, may start: only an AppImage, not
/// while quitting, and only if it is still the release that was found.
fn may_install(kind: InstallKind, quitting: bool, found: Option<&str>, agreed: &str) -> Result<(), UpdateError> {
    if kind != InstallKind::AppImage {
        return Err(UpdateError::NotAppImage);
    }
    if quitting {
        return Err(UpdateError::Quitting);
    }
    match found {
        None => Err(UpdateError::NonePending),
        Some(version) if version != agreed => Err(UpdateError::VersionChanged),
        Some(_) => Ok(()),
    }
}

/// The newest release found so far that is newer than this one, and the plugin's handle on it.
#[derive(Default)]
pub struct Updates {
    found: Mutex<Option<(UpdateAction, Update)>>,
    /// Set while an install runs, and once one has finished, until the app restarts.
    installing: AtomicBool,
}

impl Updates {
    fn action(&self) -> UpdateAction {
        match &*self.found.lock().unwrap() {
            Some((action, _)) => action.clone(),
            None => UpdateAction::None,
        }
    }

    /// What the last check found: a release newer than this one, or nothing.
    fn remember(&self, found: Option<(UpdateAction, Update)>) {
        *self.found.lock().unwrap() = found;
    }

    /// Takes the one place for an install, if `agreed` may be installed now (see [`may_install`]),
    /// and hands back the release to install. [`Updates::install_failed`] gives the place back.
    fn begin_install(&self, kind: InstallKind, quitting: bool, agreed: &str) -> Result<Update, UpdateError> {
        let found = self.found.lock().unwrap();
        may_install(kind, quitting, found.as_ref().and_then(|(action, _)| action.version()), agreed)?;
        let (_, update) = found.as_ref().ok_or(UpdateError::NonePending)?;
        if self.installing.swap(true, Ordering::SeqCst) {
            return Err(UpdateError::AlreadyInstalling);
        }
        Ok(update.clone())
    }

    fn install_failed(&self) {
        self.installing.store(false, Ordering::SeqCst);
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
    app.state::<Updates>().remember(found);
    Ok(action)
}

/// A check that tells the UI when it finds something; whether it could be made. A failure
/// (offline, no release yet) is logged and ignored: the next check is in an hour.
async fn check_and_announce<R: Runtime>(app: &AppHandle<R>) -> bool {
    match check(app).await {
        Ok(UpdateAction::None) => true,
        Ok(action) => {
            tracing::info!("an update is available: {action:?}");
            background::emit(app, background::ShellEvent::UpdateAvailable { action });
            true
        }
        Err(e) => {
            tracing::warn!("could not check for updates: {e}");
            false
        }
    }
}

/// Checks now, and then whenever one is [`due`], for as long as the app runs.
pub fn spawn_checks<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let (mut last_success, mut last_failure) = (None, None);
        loop {
            let now = SystemTime::now();
            if due(last_success, last_failure, now) {
                if check_and_announce(&app).await {
                    last_success = Some(now);
                } else {
                    last_failure = Some(now);
                }
            }
            tokio::time::sleep(WAKE_EVERY).await;
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

/// Downloads `version`, the release the user agreed to install, replaces this AppImage with it
/// (the plugin checks the signature first), and starts the app again by the way out of quitting,
/// so that the Device saves its Transfers' progress: they resume in the new version. The UI
/// calls this only once the user has agreed (and, with Transfers in progress, been told they
/// stop for the restart). Refused if that is no longer the release found, if the app is
/// quitting, and for a deb or rpm install, which is never updated in place.
#[tauri::command]
#[specta::specta]
pub async fn install_update<R: Runtime>(
    app: AppHandle<R>,
    updates: State<'_, Updates>,
    version: String,
) -> Result<(), UpdateError> {
    let update = updates.begin_install(InstallKind::current(), background::is_quitting(&app), &version)?;
    if let Err(e) = update.download_and_install(|_, _| {}, || {}).await {
        // The kind only: the plugin's message can name the AppImage's path, which is in the
        // home folder, and the log's redaction does not cover paths.
        let kind = UpdateError::from(&e);
        tracing::warn!("could not install the update: {kind:?}");
        updates.install_failed();
        return Err(kind);
    }
    tracing::info!("installed version {version}");
    if background::is_quitting(&app) {
        // Quit was chosen meanwhile: it is not to start the app again; the new version runs at
        // the next start.
        return Ok(());
    }
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
        let updates = Updates::default();
        assert_eq!(updates.action(), UpdateAction::None);
        assert!(matches!(
            updates.begin_install(InstallKind::AppImage, false, "0.2.0"),
            Err(UpdateError::NonePending)
        ));
    }

    #[test]
    fn only_the_version_that_was_found_is_installed_by_an_appimage_that_is_not_quitting() {
        assert_eq!(may_install(InstallKind::AppImage, false, Some("0.2.0"), "0.2.0"), Ok(()));
        // A newer check replaced the release the user was shown.
        assert_eq!(
            may_install(InstallKind::AppImage, false, Some("0.3.0"), "0.2.0"),
            Err(UpdateError::VersionChanged)
        );
        assert_eq!(may_install(InstallKind::AppImage, false, None, "0.2.0"), Err(UpdateError::NonePending));
        assert_eq!(may_install(InstallKind::AppImage, true, Some("0.2.0"), "0.2.0"), Err(UpdateError::Quitting));
        // A package is never updated in place, whatever else is true.
        assert_eq!(may_install(InstallKind::Package, false, Some("0.2.0"), "0.2.0"), Err(UpdateError::NotAppImage));
    }

    #[test]
    fn plugin_errors_become_kinds_the_ui_words_without_their_messages() {
        use tauri_plugin_updater::Error as E;
        assert_eq!(UpdateError::from(&E::Network("a path /home/me/x".into())), UpdateError::DownloadFailed);
        assert_eq!(UpdateError::from(&E::SignatureUtf8("x".into())), UpdateError::SignatureInvalid);
        assert_eq!(UpdateError::from(&E::MissingSignedVersion), UpdateError::SignatureInvalid);
        assert_eq!(UpdateError::from(&E::BinaryNotFoundInArchive), UpdateError::InstallFailed);
        assert_eq!(UpdateError::from(&E::TempDirNotOnSameMountPoint), UpdateError::InstallFailed);
    }

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    const HOUR: u64 = 60 * 60;
    const DAY: u64 = 24 * HOUR;

    #[test]
    fn the_first_check_is_due_at_once() {
        assert!(due(None, None, at(1_000)));
    }

    #[test]
    fn after_a_successful_check_the_next_is_due_a_day_later_by_the_wall_clock() {
        let done = at(1_000);
        assert!(!due(Some(done), None, at(1_000 + DAY - 1)));
        assert!(due(Some(done), None, at(1_000 + DAY)));
        // Asleep for three days: the first look after waking is due, however long the timer slept.
        assert!(due(Some(done), None, at(1_000 + 3 * DAY)));
    }

    #[test]
    fn after_a_failed_check_the_next_is_due_an_hour_later() {
        let failed = at(5_000);
        assert!(!due(None, Some(failed), at(5_000 + HOUR - 1)));
        assert!(due(None, Some(failed), at(5_000 + HOUR)));
        // A failure ends the day's wait but is no reason to look sooner than an hour: a day
        // after a success, with a failure just now, it waits.
        assert!(!due(Some(at(0)), Some(at(DAY + 100)), at(DAY + 200)));
        assert!(due(Some(at(0)), Some(at(DAY + 100)), at(DAY + 100 + HOUR)));
    }

    #[test]
    fn a_failure_before_a_success_does_not_hold_up_the_day() {
        // Failed at 10, succeeded at 20: only the success counts.
        assert!(!due(Some(at(20)), Some(at(10)), at(20 + HOUR)));
        assert!(due(Some(at(20)), Some(at(10)), at(20 + DAY)));
    }

    #[test]
    fn a_clock_that_went_back_counts_as_time_passed() {
        assert!(due(Some(at(10 * DAY)), None, at(DAY)));
        assert!(due(None, Some(at(10 * DAY)), at(DAY)));
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
