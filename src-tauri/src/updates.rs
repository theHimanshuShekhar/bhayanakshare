//! Updates (spec section 9): a check for a newer release at startup and about daily, and
//! installing it once the user has agreed.
//!
//! The check is a plain GET of `latest.json` on GitHub Releases (the endpoint is in
//! `tauri.conf.json`): no Device ID, no user data. An AppImage can replace itself with the new
//! release, signed with the update key, and so can the per-user Windows installer (it runs the
//! new installer, which restarts the app); a deb or rpm install never updates itself, and is only
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
use tauri::{
    AppHandle, Manager, Runtime, State,
    utils::{config::BundleType, platform::bundle_type},
};
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
    /// Installed by the Windows (NSIS) installer, which can run the next one. An exe copied out
    /// of an installation keeps the installer's mark in the binary, so it is this too, and its
    /// update goes to the installed copy, not to the copy.
    WindowsInstaller,
    /// Run from an AppImage, which can replace itself.
    AppImage,
    /// Installed by a package manager (deb, rpm), or anything else that cannot update itself: a
    /// build the bundler did not patch (run from a checkout), an AppImage's binary run from its
    /// extracted files, or a bundle this app does not build. Updated by the user, from the
    /// release page.
    Package,
}

impl InstallKind {
    /// The kind of this run: the bundle type the bundler wrote into the binary, and for an
    /// AppImage also the `APPIMAGE` variable its launcher sets.
    pub fn current() -> Self {
        Self::from_bundle(bundle_type(), std::env::var_os("APPIMAGE").as_deref())
    }

    /// Only a bundle that can update itself says so; no bundle type is a package, so that a
    /// build that cannot be recognised is never replaced. The AppImage's binary is the same
    /// whether it is run from the file or from the files extracted from it, and only the file
    /// can be replaced, so it also needs `appimage` (the path of the file) set, and non-empty.
    fn from_bundle(bundle: Option<BundleType>, appimage: Option<&OsStr>) -> Self {
        match bundle {
            Some(BundleType::Nsis) => Self::WindowsInstaller,
            Some(BundleType::AppImage) if appimage.is_some_and(|path| !path.is_empty()) => Self::AppImage,
            _ => Self::Package,
        }
    }

    /// Whether this install replaces itself once the user agrees, rather than pointing at the
    /// release page.
    fn updates_itself(self) -> bool {
        self != Self::Package
    }
}

/// What to do about the latest release.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UpdateAction {
    /// Nothing newer, or nothing that could be compared.
    None,
    /// Offer to install `version` and restart: the Windows installer, or an AppImage.
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
    if kind.updates_itself() {
        UpdateAction::Install { version }
    } else {
        UpdateAction::OpenPage { version }
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
    /// This install is a package (or unrecognised), which is never updated in place.
    NotSelfUpdating,
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
    /// Replacing the AppImage, or starting the Windows installer, failed.
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

/// Whether installing `agreed`, the version the user was shown, may start: only the Windows
/// installer and an AppImage, not while quitting, and only if it is still the release that was
/// found.
fn may_install(kind: InstallKind, quitting: bool, found: Option<&str>, agreed: &str) -> Result<(), UpdateError> {
    if !kind.updates_itself() {
        return Err(UpdateError::NotSelfUpdating);
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

/// What the updater runs just before it starts the Windows installer and ends the process itself
/// (`std::process::exit`, so neither the `RunEvent::Exit` handler nor anything after
/// `Update::install` runs): the rest of quitting. [`run_install`] has already claimed it (the UI
/// says "Saving progress…"); this saves the Transfers' progress, for at most
/// [`QUIT_DEADLINE`](crate::QUIT_DEADLINE), writes out the log, and then does what the updater
/// would have done itself (`cleanup_before_exit`: the tray icon and the windows). It replaces the
/// updater's own hook, so it must.
///
/// The updater calls it only on Windows. It blocks until that is done, so it must not be called
/// from a thread of the async runtime ([`run_install`] runs the install on a blocking one).
fn before_exit<R: Runtime>(app: &AppHandle<R>) -> impl Fn() + Send + Sync + 'static {
    let app = app.clone();
    move || {
        tauri::async_runtime::block_on(background::save_and_flush(&app));
        app.cleanup_before_exit();
    }
}

/// Looks at `latest.json` once and remembers what it found. A failed check says why, as a
/// message, and leaves what was found before.
async fn check<R: Runtime>(app: &AppHandle<R>) -> Result<UpdateAction, String> {
    let updater = app
        .updater_builder()
        .timeout(CHECK_TIMEOUT)
        // The release found keeps these, which are for Windows only (the builder ignores them
        // elsewhere). The hook is what quitting does; the NSIS installer is told in passive mode
        // (`tauri.conf.json`) to start the app when it is done (`/R`), with no arguments, as
        // the AppImage's restart does, so that a start at login (`--background`) does not
        // bring the app back with its window closed, and the link or file it was started with
        // is not opened again. The updater's own restart would pass them on.
        .on_before_exit(before_exit(app))
        .restart_after_install(false)
        .installer_arg("/R")
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

/// Logs why an install failed, by its kind only: the plugin's message can name the AppImage's
/// path, which is in the home folder, and the log's redaction does not cover paths.
fn log_failure(e: &tauri_plugin_updater::Error) -> UpdateError {
    let error = UpdateError::from(e);
    tracing::warn!("could not install the update: {error:?}");
    error
}

/// Runs `install` on a blocking thread: on Windows the before-exit hook waits for the Device to
/// shut down, which needs the async runtime's threads free, and the install does not return at
/// all when it works, as the updater ends the process.
///
/// That is why Windows claims quitting first, so that the installer, which starts the app again,
/// is not run for a Quit that was chosen meanwhile (that is refused with `Quitting`). From then
/// on the app is leaving, so an install that fails starts the app again, rather than leave it
/// saying "Saving progress…" with a Device that may be shut down. Other installs just give back
/// the place for an install. (`exit` is how the app is left, so that a test need not leave.)
async fn run_install<R: Runtime>(
    app: &AppHandle<R>,
    updates: &Updates,
    kind: InstallKind,
    install: impl FnOnce() -> tauri_plugin_updater::Result<()> + Send + 'static,
    exit: impl FnOnce(),
) -> Result<(), UpdateError> {
    let leaves = kind == InstallKind::WindowsInstaller;
    if leaves && !background::begin_quit(app) {
        updates.install_failed();
        return Err(UpdateError::Quitting);
    }
    let error = match tauri::async_runtime::spawn_blocking(install).await {
        Ok(Ok(())) => return Ok(()),
        Ok(Err(e)) => log_failure(&e),
        Err(_) => UpdateError::InstallFailed,
    };
    if leaves {
        app.state::<Relaunch>().0.store(true, Ordering::SeqCst);
        exit();
    } else {
        updates.install_failed();
    }
    Err(error)
}

/// Downloads `version`, the release the user agreed to install (the plugin checks the
/// signature), and installs it. An AppImage is replaced and the app starts again by the way out
/// of quitting, so that the Device saves its Transfers' progress: they resume in the new
/// version. On Windows the updater runs the installer and ends the process itself, after the
/// before-exit hook has done what quitting does; the installer starts the new version. The UI
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
    let kind = InstallKind::current();
    let update = updates.begin_install(kind, background::is_quitting(&app), &version)?;
    let bytes = match update.download(|_, _| {}, || {}).await {
        Ok(bytes) => bytes,
        Err(e) => {
            updates.install_failed();
            return Err(log_failure(&e));
        }
    };
    tracing::info!("installing version {version}");
    run_install(&app, &updates, kind, move || update.install(bytes), || app.exit(0)).await?;
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
    use bhayanakshare_core::Device;
    use serde_json::Value;
    use tauri::test::{mock_builder, mock_context, noop_assets};

    use super::*;

    fn install(version: &str) -> UpdateAction {
        UpdateAction::Install { version: version.into() }
    }

    fn page(version: &str) -> UpdateAction {
        UpdateAction::OpenPage { version: version.into() }
    }

    const KINDS: [InstallKind; 3] = [InstallKind::WindowsInstaller, InstallKind::AppImage, InstallKind::Package];

    fn appimage_file() -> Option<&'static OsStr> {
        Some(OsStr::new("/home/me/BhayanakShare.AppImage"))
    }

    #[test]
    fn the_install_kind_is_the_bundle_the_app_was_built_as() {
        let kind = |bundle, appimage| InstallKind::from_bundle(Some(bundle), appimage);
        assert_eq!(kind(BundleType::Nsis, None), InstallKind::WindowsInstaller);
        assert_eq!(kind(BundleType::AppImage, appimage_file()), InstallKind::AppImage);
        assert_eq!(kind(BundleType::Deb, None), InstallKind::Package);
        assert_eq!(kind(BundleType::Rpm, None), InstallKind::Package);
        // The launcher's variable does not make a package an AppImage.
        assert_eq!(kind(BundleType::Deb, appimage_file()), InstallKind::Package);
    }

    #[test]
    fn an_appimages_binary_run_from_its_extracted_files_is_a_package() {
        // The binary inside `squashfs-root` has the AppImage mark but no file to replace.
        for appimage in [None, Some(OsStr::new(""))] {
            assert_eq!(InstallKind::from_bundle(Some(BundleType::AppImage), appimage), InstallKind::Package);
        }
    }

    #[test]
    fn a_build_that_is_no_known_bundle_is_a_package_and_never_installs_itself() {
        // A dev build or a test run has no bundle type (nothing patched the binary), and neither
        // does a bundle whose patching failed: all are only pointed at the release page.
        assert_eq!(bundle_type(), None);
        assert_eq!(InstallKind::from_bundle(None, None), InstallKind::Package);
        assert_eq!(InstallKind::from_bundle(None, appimage_file()), InstallKind::Package);
        assert_eq!(InstallKind::current(), InstallKind::Package);
        // Bundles that are not built, or not by this shell's installer: the same.
        for other in [BundleType::Msi, BundleType::App, BundleType::Dmg] {
            assert_eq!(InstallKind::from_bundle(Some(other), appimage_file()), InstallKind::Package);
        }
    }

    #[test]
    fn only_a_windows_installer_and_an_appimage_update_themselves() {
        assert!(InstallKind::WindowsInstaller.updates_itself());
        assert!(InstallKind::AppImage.updates_itself());
        assert!(!InstallKind::Package.updates_itself());
    }

    #[test]
    fn a_newer_release_is_installed_by_a_windows_installer_and_an_appimage_and_linked_to_for_a_package() {
        for kind in KINDS.into_iter().filter(|kind| kind.updates_itself()) {
            assert_eq!(decide("0.1.0", "0.2.0", kind), install("0.2.0"));
            assert_eq!(decide("0.9.0", "0.10.0", kind), install("0.10.0"));
        }
        assert_eq!(decide("0.1.0", "0.2.0", InstallKind::Package), page("0.2.0"));
        assert_eq!(decide("0.1.0", "1.0.0-beta.1", InstallKind::Package), page("1.0.0-beta.1"));
    }

    #[test]
    fn the_same_or_an_older_release_is_nothing_to_do() {
        for kind in KINDS {
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
    fn only_the_version_that_was_found_is_installed_by_a_self_updating_install_that_is_not_quitting() {
        for kind in KINDS.into_iter().filter(|kind| kind.updates_itself()) {
            assert_eq!(may_install(kind, false, Some("0.2.0"), "0.2.0"), Ok(()));
            // A newer check replaced the release the user was shown.
            assert_eq!(may_install(kind, false, Some("0.3.0"), "0.2.0"), Err(UpdateError::VersionChanged));
            assert_eq!(may_install(kind, false, None, "0.2.0"), Err(UpdateError::NonePending));
            assert_eq!(may_install(kind, true, Some("0.2.0"), "0.2.0"), Err(UpdateError::Quitting));
        }
        // A package is never updated in place, whatever else is true.
        assert_eq!(may_install(InstallKind::Package, false, Some("0.2.0"), "0.2.0"), Err(UpdateError::NotSelfUpdating));
        assert_eq!(may_install(InstallKind::Package, true, None, "0.2.0"), Err(UpdateError::NotSelfUpdating));
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
    fn nothing_is_pending_until_a_check_finds_something_for_a_windows_installer_too() {
        let updates = Updates::default();
        assert!(matches!(
            updates.begin_install(InstallKind::WindowsInstaller, false, "0.2.0"),
            Err(UpdateError::NonePending)
        ));
        assert!(matches!(
            updates.begin_install(InstallKind::Package, false, "0.2.0"),
            Err(UpdateError::NotSelfUpdating)
        ));
    }

    /// A mock app with a Device on temp folders, and the states the shell sets up.
    fn app_with_device(tmp: &std::path::Path) -> tauri::App<tauri::test::MockRuntime> {
        use std::sync::Arc;

        use bhayanakshare_core::{DeviceConfig, KeySource, Network, SystemClock, SystemFreeSpace};

        let config = DeviceConfig {
            key_source: KeySource::File(tmp.join("data").join("secret.key")),
            data_dir: tmp.join("data"),
            save_dir: tmp.join("save"),
            clock: Arc::new(SystemClock),
            network: Network::Localhost,
            free_space: Arc::new(SystemFreeSpace),
        };
        let app = tauri::test::mock_app();
        // As `setup` does, so that events can be emitted.
        crate::specta_builder().mount_events(&app);
        app.manage(background::Quitting::default());
        app.manage(Updates::default());
        app.manage(Relaunch::default());
        crate::start_device(&app, config).unwrap();
        app
    }

    /// What the UI hears of the shell's events, as the JSON of each payload.
    fn listen_to_shell_events(app: &tauri::App<tauri::test::MockRuntime>) -> std::sync::Arc<Mutex<Vec<String>>> {
        use tauri::Listener as _;
        use tauri_specta::Event as _;

        let heard = std::sync::Arc::new(Mutex::new(Vec::new()));
        let seen = heard.clone();
        app.listen_any(background::ShellEvent::NAME, move |event| seen.lock().unwrap().push(event.payload().to_owned()));
        heard
    }

    const QUITTING: &str = r#"{"type":"quitting"}"#;

    fn installer_failed() -> tauri_plugin_updater::Result<()> {
        Err(tauri_plugin_updater::Error::BinaryNotFoundInArchive)
    }

    #[test]
    fn the_windows_install_saves_progress_in_the_before_exit_hook_before_the_updater_ends_the_process() {
        use std::io::Write as _;

        use bhayanakshare_core::{Error, LogFiles, Visibility};

        let tmp = tempfile::tempdir().unwrap();
        let app = app_with_device(tmp.path());
        let logs = tmp.path().join("data").join("logs");
        let files = std::sync::Arc::new(LogFiles::open(&logs).unwrap());
        app.manage(crate::Logging::new(Some(files.clone()), |_| Ok(())));
        let device = app.state::<Device>();
        let handle = app.handle();
        let heard = listen_to_shell_events(&app);

        // Running the Device: a command is answered.
        assert!(tauri::async_runtime::block_on(device.set_visibility(Visibility::Everyone)).is_ok());
        (&*files).write_all(b"installing version 0.2.0\n").unwrap();

        // The updater calls the hook from inside the install, which `run_install` runs on a
        // blocking thread, and then ends the process: here the install fails after the hook, so
        // that the test can look at what the hook did.
        let hook = before_exit(handle);
        let updates = app.state::<Updates>();
        let left = AtomicBool::new(false);
        let result = tauri::async_runtime::block_on(run_install(
            handle,
            &updates,
            InstallKind::WindowsInstaller,
            move || {
                hook();
                installer_failed()
            },
            || left.store(true, Ordering::SeqCst),
        ));

        // The quit was claimed once, before the install: the UI says "Saving progress…" and the
        // hook does not say it again.
        assert!(background::is_quitting(handle));
        assert_eq!(*heard.lock().unwrap(), [QUITTING]);
        // The Device has been shut down, and the log is on disk.
        let after = tauri::async_runtime::block_on(device.set_visibility(Visibility::Hidden));
        assert!(matches!(after, Err(Error::ShuttingDown)), "{after:?}");
        let log = std::fs::read_dir(&logs).unwrap().next().unwrap().unwrap().path();
        assert_eq!(std::fs::read_to_string(log).unwrap(), "installing version 0.2.0\n");
        // The installer did not start (here), so the app starts again instead of staying up
        // without a Device.
        assert_eq!(result, Err(UpdateError::InstallFailed));
        assert!(app.state::<Relaunch>().0.load(Ordering::SeqCst));
        assert!(left.load(Ordering::SeqCst), "the app did not leave to start again");
    }

    #[test]
    fn a_quit_chosen_before_the_install_stops_the_windows_installer_from_starting() {
        let tmp = tempfile::tempdir().unwrap();
        let app = app_with_device(tmp.path());
        let handle = app.handle();
        let heard = listen_to_shell_events(&app);
        let updates = app.state::<Updates>();
        updates.installing.store(true, Ordering::SeqCst);
        assert!(background::begin_quit(handle));

        let started = std::sync::Arc::new(AtomicBool::new(false));
        let result = tauri::async_runtime::block_on(run_install(
            handle,
            &updates,
            InstallKind::WindowsInstaller,
            {
                let started = started.clone();
                move || {
                    started.store(true, Ordering::SeqCst);
                    Ok(())
                }
            },
            || panic!("the app left for an install that did not start"),
        ));

        assert_eq!(result, Err(UpdateError::Quitting));
        assert!(!started.load(Ordering::SeqCst), "the installer was started");
        // Quit goes on as it was: no second "Saving progress…", and no restart.
        assert_eq!(*heard.lock().unwrap(), [QUITTING]);
        assert!(!app.state::<Relaunch>().0.load(Ordering::SeqCst));
    }

    #[test]
    fn a_failed_appimage_install_gives_back_its_place_and_leaves_the_app_running() {
        let tmp = tempfile::tempdir().unwrap();
        let app = app_with_device(tmp.path());
        let handle = app.handle();
        let updates = app.state::<Updates>();
        updates.installing.store(true, Ordering::SeqCst);

        let result = tauri::async_runtime::block_on(run_install(handle, &updates, InstallKind::AppImage, installer_failed, || {
            panic!("an AppImage install that failed does not leave")
        }));

        assert_eq!(result, Err(UpdateError::InstallFailed));
        assert!(!updates.installing.load(Ordering::SeqCst));
        assert!(!background::is_quitting(handle));
        assert!(!app.state::<Relaunch>().0.load(Ordering::SeqCst));
        // An AppImage's install that works is left to the caller, which restarts by quitting.
        let installed = tauri::async_runtime::block_on(run_install(handle, &updates, InstallKind::AppImage, || Ok(()), || unreachable!()));
        assert_eq!(installed, Ok(()));
        assert!(!background::is_quitting(handle));
    }

    #[test]
    fn the_before_exit_hook_is_safe_when_quit_was_already_chosen_and_says_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let app = app_with_device(tmp.path());
        let handle = app.handle();
        let heard = listen_to_shell_events(&app);
        // Quit is in progress and the Device is already shut down: the hook is not stopped by
        // that, nor does it need a log.
        assert!(background::begin_quit(handle));
        tauri::async_runtime::block_on(app.state::<Device>().shutdown(Duration::from_secs(5)));
        before_exit(handle)();
        assert_eq!(*heard.lock().unwrap(), [QUITTING], "only the quit's own event");
    }

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
        // The Windows installer runs with a progress bar and no questions (the plugin's default,
        // written down): per user, so there is no UAC prompt either.
        assert_eq!(updater["windows"]["installMode"], "passive");
        assert_eq!(conf["bundle"]["createUpdaterArtifacts"], true);
        // The licence the packages ship, relative to this folder, is there.
        let license = conf["bundle"]["licenseFile"].as_str().unwrap();
        assert!(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(license).is_file());
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
    fn linux_builds_the_appimage_deb_and_rpm() {
        // Each platform names its own targets in its own file; the base has none, so no platform
        // gets the MSI or anything else by default.
        let linux: Value = serde_json::from_str(include_str!("../tauri.linux.conf.json")).unwrap();
        assert_eq!(linux["bundle"]["targets"], serde_json::json!(["appimage", "deb", "rpm"]));
        assert!(tauri_conf()["bundle"].get("targets").is_none());
    }

    fn tauri_windows_conf() -> Value {
        serde_json::from_str(include_str!("../tauri.windows.conf.json")).unwrap()
    }

    #[test]
    fn windows_builds_a_per_user_nsis_installer_and_nothing_else() {
        let bundle = &tauri_windows_conf()["bundle"];
        // Not the MSI: Tauri's is per-machine, and it asks for administrator rights.
        assert_eq!(bundle["targets"], serde_json::json!(["nsis"]));
        let nsis = &bundle["windows"]["nsis"];
        // Per user: under %LOCALAPPDATA%, no UAC prompt, the scheme and the uninstall entry in HKCU.
        assert_eq!(nsis["installMode"], "currentUser");
        assert_eq!(bundle["windows"]["webviewInstallMode"], serde_json::json!({"type": "downloadBootstrapper"}));
        assert!(bundle["windows"].get("wix").is_none());
        // The stock installer script only: no template of our own and no hooks, which are where a
        // firewall rule or any other install step would go.
        for key in ["template", "installerHooks"] {
            assert!(nsis.get(key).is_none(), "the NSIS config has a {key}");
        }
        // The installer is not code-signed in version 1, and nothing is run to sign it.
        assert!(bundle["windows"].get("signCommand").is_none() && bundle["windows"].get("certificateThumbprint").is_none());
    }

    #[test]
    fn neither_config_touches_the_firewall_and_both_platforms_make_updater_artifacts() {
        for (name, text) in [
            ("tauri.conf.json", include_str!("../tauri.conf.json")),
            ("tauri.windows.conf.json", include_str!("../tauri.windows.conf.json")),
        ] {
            let text = text.to_lowercase();
            assert!(!text.contains("firewall") && !text.contains("netsh"), "{name} touches the firewall");
        }
        // Updater artifacts stay on for both platforms: Windows does not turn them off.
        assert!(tauri_windows_conf()["bundle"].get("createUpdaterArtifacts").is_none());
        assert_eq!(tauri_conf()["bundle"]["createUpdaterArtifacts"], true);
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
