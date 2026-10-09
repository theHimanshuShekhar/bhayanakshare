<img src="assets/icon.svg" width="64" height="64" alt="">

# BhayanakShare

A desktop app for sending files directly from one Device to another, AirDrop-style. The design is in [`docs/spec/v1.md`](docs/spec/v1.md), the vocabulary in [`CONTEXT.md`](CONTEXT.md).

Created in [T3 Code](https://t3.codes).

## Layout

| Path | What it is |
|---|---|
| `crates/core` | The Rust core, with no Tauri dependency: Device API, control protocol, blob stores, SQLite persistence. All integration tests live here. |
| `src-tauri` | The Tauri 2 shell: starts a core `Device`, exposes its commands to the UI, forwards its event stream. |
| `ui` | React + TypeScript + Vite front end. `ui/src/bindings.ts` is generated from the Rust types and committed; every UI string is in `ui/src/i18n.ts`. |

The core's public seam is `Device`: it is created from a data folder, a save folder, a key source and an injected clock; commands go in as methods and everything that happens comes back, in order, on one `EventStream`. The shell and every test use only that.

## Install and update

Version 1 runs on **Windows 11 (x64)** and **Linux (x86_64)**. The Linux distros it is built and checked for are Ubuntu 22.04+ and Fedora 39+ (deb and rpm). The AppImage is expected to work on other distros with glibc 2.35+ and WebKitGTK 4.1, but is not checked there. macOS is later: there is no macOS build, and signing, notarisation and the Local Network permission are not part of version 1.

Releases are on the [releases page](https://github.com/theHimanshuShekhar/bhayanakshare/releases/latest). Windows has one installer; Linux has three formats. All are built by Tauri's bundler:

| Format | Install | Updates |
|---|---|---|
| Windows installer (`BhayanakShare_X.Y.Z_x64-setup.exe`; Windows 11) | Run it; see [Installing on Windows](#installing-on-windows) | Itself, after the user agrees |
| AppImage (`BhayanakShare_X.Y.Z_amd64.AppImage`) | `chmod +x` the file and run it | Itself, after the user agrees |
| deb (`BhayanakShare_X.Y.Z_amd64.deb`; Debian, Ubuntu) | `sudo apt install ./BhayanakShare_X.Y.Z_amd64.deb` | Not itself: a notice with a link to the release page |
| rpm (`BhayanakShare-X.Y.Z-1.x86_64.rpm`; Fedora, openSUSE) | `sudo dnf install ./BhayanakShare-X.Y.Z-1.x86_64.rpm` | Not itself: a notice with a link to the release page |

The app looks for a newer release when it starts, and again once 24 hours of clock time have passed since the last successful look (it wakes hourly to check, so a machine that was asleep looks soon after waking, and one that was offline at start tries again within the hour), by fetching `latest.json` from the latest release on GitHub: a plain GET, with no Device ID and no user data. There is no telemetry. When a newer release exists the app shows "Update available (version X)":

- **Windows installer** (intended, once [#58](https://github.com/theHimanshuShekhar/bhayanakshare/issues/58) lands; the shell's updater detects only the AppImage today): the notice has "Install and restart" and works as the AppImage's does: nothing is installed before the user agrees, the signature is checked against the key in the app, and the app restarts the way Quit does. Until then a Windows install is updated by hand, with the installer from the releases page.
- **AppImage** (the `APPIMAGE` variable is set, `src-tauri/src/updates.rs`): the notice has "Install and restart". Nothing is installed before the user presses it, and what is installed is the version the notice showed (if a newer one has been found since, the install is refused and the user checks again). With Transfers in progress the user is asked first, as they stop for the restart. The new AppImage is downloaded, its signature is checked against the key in the app, the file is replaced, and the app restarts the way Quit does, so Transfers save their progress and resume.
- **deb and rpm** (anything on Linux that is not an AppImage): the notice links to the release page; the package is updated by hand.

When a Transfer is refused because this Device's version is older, the notice also has "Update now": it looks for the update and, on an AppImage, installs it and restarts (pressing it is the agreement; the version being installed is shown), and on a package opens the release page (on Windows, the same as an AppImage, once #58 lands). If no update is found, or the check cannot be made (offline), it says so. A failed check or install is logged and otherwise ignored, never a crash.

The Linux packages register the `bhayanakshare://` scheme (the desktop file, `src-tauri/linux/bhayanakshare.desktop`, has the scheme handler and `%u` to receive the link), run no install scripts and never change firewall rules. The Windows installer does not change them either; Windows asks the first time the app listens. See [`docs/firewall.md`](docs/firewall.md) for allowing local discovery yourself on either system.

### Installing on Windows

Download the installer from the releases page and run it. It installs for the current user only, under `%LOCALAPPDATA%`, with no administrator rights and no UAC prompt, and adds BhayanakShare to the Start menu and to Settings, Apps. Windows 11 already has WebView2, which the app needs; the installer downloads it only on a PC that lacks it.

The installer is **not code-signed in version 1**, so Windows warns about it:

- **SmartScreen** shows "Windows protected your PC". Choose **More info**, then **Run anyway**.
- **Smart App Control** is a stricter Windows 11 setting, and a fresh install can have it on. When it is on, it blocks the unsigned installer outright, with no "Run anyway" and no way to allow one app. To install, turn Smart App Control off first (Windows Security, App & browser control, Smart App Control settings). This is a limit of version 1's unsigned build, not something the installer can work around; code signing is a later decision. Microsoft says recent Windows updates let Smart App Control be turned on again afterwards, but not on every PC (see [What is Smart App Control](https://support.microsoft.com/en-us/topic/what-is-smart-app-control-285ea03d-fa88-4d56-882e-6698afdb7003)).

The first time the app listens, Windows asks about its firewall. Only an administrator can allow it, and cancelling leaves block rules that stop the app finding Devices; see [Windows](docs/firewall.md#windows) before answering.

**Uninstalling** (Settings, Apps, Installed apps, BhayanakShare, Uninstall; or the Start menu entry) removes the app and **keeps** two things, so that a reinstall is the same Device with the same Device ID:

- The data folder, `%APPDATA%\dev.bhayanakshare.share` (the Roaming AppData folder plus the app's identifier): the database with settings, Contacts and History, `logs/`, and the fallback `secret.key` if one was ever made. Delete the folder to remove it. The uninstaller has an option to delete the application data too, unticked by default; it removes this folder but not the key.
- The secret key, in Credential Manager. Open Credential Manager (Start menu), **Windows Credentials**, and under Generic Credentials remove the entry named `device-secret-key.bhayanakshare`.

Remove both and the next install is a new Device with a new Device ID. The files received are in the save folder (`Downloads\BhayanakShare` by default), which the uninstaller never touches.

### Cutting a release

1. Bump the version in `src-tauri/tauri.conf.json` and `[workspace.package]` in `Cargo.toml` (the same number in both; the release workflow stops if the tag disagrees with either), and commit it.
2. Tag it `vX.Y.Z` and push the tag.
3. `.github/workflows/release.yml` builds on Ubuntu 22.04 and uploads the AppImage, deb, rpm, their updater signatures and `latest.json` to a **draft** GitHub Release.
4. Publish the draft. The updater reads the latest *published* release, so installed AppImages see the new version only then.

The release workflow is the one that sees the signing key, so its actions are pinned to commits (the tag is in a comment beside each), it caches nothing, and only its job has write permission. Update the pins by hand, checking the new tag.

`.github/workflows/ci.yml` runs `pnpm typecheck` and `pnpm test` on every push and pull request, on Ubuntu and on Windows (without `BHAYANAKSHARE_REQUIRE_MULTICAST`, as multicast may not work on a runner). A test that cannot run on Windows is `#[ignore]`d there with its reason, or prints `SKIPPED: ...` and says what it did not test; the Windows job runs with `RUST_TEST_NOCAPTURE` so that line is in its log.

### The update-signing key

Updates are signed. The **private** key lives only as the repository secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, which only the release workflow reads. It is never committed or logged. The **public** key is `plugins.updater.pubkey` in `src-tauri/tauri.conf.json`, and every installed AppImage carries it.

The key pair was made with `pnpm tauri signer generate`. An update whose signature does not match the public key is refused, and the error is logged.

Losing the private key means no release can be signed for the AppImages already installed: their users must install a new AppImage by hand, signed with a new key, which then carries the new public key. Keep a copy somewhere safe.

A local `pnpm build` makes updater artifacts too (`createUpdaterArtifacts`), so it needs `TAURI_SIGNING_PRIVATE_KEY` set. To build the installers without them, run `pnpm tauri build --config '{"bundle":{"createUpdaterArtifacts":false}}'`.

## First run and settings

The first time a Device runs, one screen is shown instead of the tabs (`ui/src/FirstRunScreen.tsx`): Device Name (the hostname), Visibility ("People who have my ID"), start at login (on) and the save folder (`~/Downloads/BhayanakShare`). "Get started" keeps them as they are, or as the user changed them, and goes to Home. Start at login is switched on (or off) there by an explicit choice, which also marks the shell's one-time default (`background::default_autostart`, for installs that never see the screen) as done, so the two cannot undo each other; if it cannot be switched on, first run still finishes, with a note. A Visibility that could not be read is not overwritten with the default shown. Whether it was done is the `first_run_done` setting in the Device's database, so an install made before the screen existed sees it once.

Every setting lives in the Settings tab, in sections: This Device (Device Name, My ID), Privacy (Visibility, public DHT), Receiving (the save folder), App (start at login, the version and "Check for updates") and then Identity and Diagnostics. A new Device Name, Visibility or public DHT setting takes effect at once, without a restart.

The save folder is the `save_folder` setting. Until it is set it is the folder the Device was configured with (`BHAYANAKSHARE_SAVE_DIR`, else `~/Downloads/BhayanakShare`); once set it wins over that. Changing it applies to the next Offer accepted (also one waiting for an answer, and Auto-accept), and it must be an absolute path that is made if missing and can be written in, else `Device::set_save_folder` fails with `Error::SaveFolder` (a kind the UI words). Only the default folder is made at start: one the user set that is missing (an unmounted drive, say) is left alone and logged, and each Offer's check says so until it is back. The Offer sheet can still choose another folder for one Offer.

## Commands

Requirements: a Rust toolchain, Node 22+ and pnpm. Building the Tauri shell on Linux also needs `pkg-config` and the `webkit2gtk-4.1`, `libsoup-3.0` and GTK 3 development packages (see the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)).

```sh
pnpm install

pnpm typecheck   # cargo check --workspace --all-targets (core + shell), then tsc for the UI
pnpm test        # cargo test --workspace (core + shell), then the UI tests (vitest)
pnpm bindings    # regenerate ui/src/bindings.ts after changing a command or an event type

pnpm dev         # run the desktop app with hot reload
pnpm build       # build installers
```

Plain `cargo check` and `cargo test` at the repository root cover only the core (the workspace's `default-members`), so they need no system libraries. Add `--workspace` to include the shell.

The UI's TypeScript types for commands and events are generated from the Rust types with [specta](https://github.com/specta-rs/tauri-specta). `cargo test --workspace` fails if `ui/src/bindings.ts` is stale; `pnpm bindings` rewrites it.

### Running two instances on one machine

Each instance needs its own data folder (it holds the Device ID) and, to tell them apart, its own save folder:

```sh
# terminal 1: the first instance, and the dev server both instances load
BHAYANAKSHARE_DATA_DIR=/tmp/bhs-a/data BHAYANAKSHARE_SAVE_DIR=/tmp/bhs-a/save pnpm dev
# terminal 2: a second instance, started from the debug binary terminal 1 built
BHAYANAKSHARE_DATA_DIR=/tmp/bhs-b/data BHAYANAKSHARE_SAVE_DIR=/tmp/bhs-b/save target/debug/bhayanakshare
```

An instance given `BHAYANAKSHARE_DATA_DIR` is treated as a separate install made to run beside another: it skips the single-instance check and does not register itself to start at login or as the handler of `bhayanakshare://` links. (Without the variable, a second launch focuses the running window instead and hands it any files named on the command line.)

Copy the Device ID from one instance's "My ID", choose "Send to ID…" in the other, and paste it. Without the variables the data lives in the platform's app data folder and files are saved to `~/Downloads/BhayanakShare` (or the folder chosen in Settings, which takes the place of `BHAYANAKSHARE_SAVE_DIR` once set).

### Where the secret key lives

The Device ID comes from a secret key, which is kept in the OS secret store (Secret Service on Linux, Credential Manager on Windows; entry `bhayanakshare` / `device-secret-key`, which Credential Manager lists as `device-secret-key.bhayanakshare`), or in `secret.key` in the data folder, where there is no secret store. On Linux that file has mode 0600. Windows has no Unix modes: the fallback key file, like the log files, relies on the permissions of the user profile folder, so anyone who can read that folder can read them. `key-location` beside it says which. Where it says `os-store`, a store that is locked or not running stops the app with an error (shown in a dialog): it never makes a new key, so it never gives the Device a new Device ID. The one accepted gap is a first start with nothing recorded: if the data folder was cleared while the Secret Service was not running, the old key in the store cannot be seen, and a new key is made in a file. A `secret.key` left by an older version moves into the store on the next start, and is deleted only after the store has given the same key back. An instance with its own `BHAYANAKSHARE_DATA_DIR` keeps its key in that folder instead, as the store has one entry for all installs of a user.

Settings → Identity exports the key alone, under a password, as a `.bhid` file, and imports one on another install. Contacts and History are neither exported nor touched by an import.

### Privacy and diagnostics

BhayanakShare logs what it does to files in `logs/` in its data folder: one file a day (UTC), the last 7 kept and about 50 MB in all at the most (the oldest files go first; if a single day's file reaches the cap, the rest of that day is not logged), so a week of use at most. A line says when, how serious and what happened, and names another Device only by its Fingerprint, never its whole ID; no file name, folder name or text is ever logged, and neither is a path inside the save folder (one rare line, an incoming store that could not be opened, can still carry the save folder's path in its error, though the store's own files are named by hash and Transfer ID, never by a transferred file's name). Anything shaped like a Device ID (the base32 form, iroh's hex form, the z-base-32 form of its lookups) is also redacted from every line on its way to the file, as are the 64-digit hex hashes of content, and two chatty iroh lookups are silenced, since they print whole IDs. Settings → Diagnostics has a "Debug logging" switch (a bigger log, which follows the same rules) and "Export diagnostics…", which saves a zip of the log files and an `about.txt` (app version, operating system, Visibility, whether debug logging is on, the network, this Device's Fingerprint) wherever the user says. Nothing is sent anywhere: there is no telemetry and no crash reporting, and a crash only leaves a line in the local log (where it happened, and its message if that is a fixed one). Passing the zip on is up to the user. (The one thing the app fetches on its own is the check for a newer release, a plain GET of `latest.json`; see [Install and update](#install-and-update).)

For code: log a Device with `DeviceId::fingerprint()` (never `{id}` on a `DeviceId` or an `EndpointId`), never log a path under a save folder or a source being sent (use the Transfer ID), and never log a `Message` or an `Offer` with `{:?}`, nor an `Error` that has a path in it (`Error::for_log`). `crates/core/tests/log_privacy.rs` runs Transfers, refusals and failures with the app's own filter at debug (`log_filter(true)`) and fails if the log holds a whole Device ID, a name or text. The subscriber is installed by the shell (`src-tauri/src/logging.rs`), the filter, the rolling files, the redaction and the export are in `crates/core/src/logs.rs` (the writer is synchronous, under a lock, not a background thread: lines are small and rare, and a panic's line or an export must find everything written). `RUST_LOG` is not read; a debug build also writes to stderr.

### Share links

"My ID" shows `bhayanakshare://add/<Device ID>?name=<Device Name>` and its QR code. The link is made and read in the UI (`ui/src/shareLink.ts`), which matches the Device ID in it with the same pattern `ui/src/contacts.ts` uses for a pasted ID: a link is accepted wherever a Device ID is pasted, and the Rust commands still take the bare ID. Only the Device ID decides whether a link is valid. The name is a suggestion: it is made fit to be a Contact's name (control characters removed, trimmed, cut to 64 characters, where the core would refuse a name that is not already clean) and dropped if its percent-escapes cannot be read. It fills the name field of Add Contact, never the identity: the Fingerprint check is unchanged. A link opened while another dialog is open waits until that is closed.

The shell registers the scheme with the `deep-link` plugin (`tauri.conf.json`), and on Linux calls `register_all()` at every start so a moved AppImage registers itself again. The UI gets opened links, and the one that started the app, from the plugin's JavaScript API. To try one without a second machine, start the app without `BHAYANAKSHARE_DATA_DIR` and run `xdg-open 'bhayanakshare://add/<Device ID>?name=Test'`.

"Scan QR code…" in Add Contact uses the webcam (`getUserMedia`, decoded by `jsqr`); the QR code is drawn by `uqr`. On Linux, WebKitGTK keeps the camera off, so the shell turns it on for the main window and allows video-only requests.

## Tests

Integration tests drive Devices through the Device API only. `crates/core/tests/support` starts 2 or 3 Devices in one process on localhost, with relays and address lookup off; a test hands one Device another's address (`TestDevice::addr`) and reads each Device's event stream. Every Device gets its own temp folders and a `ManualClock`. Start new integration tests from that harness.

LAN discovery is its own seam, `crates/core/tests/discovery.rs`: Devices started with `TestDevice::start_discovering` use real mDNS multicast on the loopback interface (UDP port 5353, shared with anything else on the machine, so a test looks for one specific Device and never expects an exact list). Hidden Visibility is `crates/core/tests/hidden.rs`, a binary of its own that runs one test at a time: to show that a Hidden Device sends nothing it listens for a few seconds and hears nothing, which means nothing while other tests announce. It uses the same harness and the raw mDNS helpers of `tests/support/multicast.rs` (a sniffer on the group, and queries from a Device that holds no ID; the blinded label is spelled out there, so a change to the design fails the tests). A Device that cannot bind the mDNS port reports it as its discovery status (`Device::discovery_status` and `DiscoveryStatus` events), which Home words as a hint and which recovers when a later try binds (every 15 s); the tests for it hold port 5353 exclusively (`support::multicast::hold_mdns_port`, with `SO_EXCLUSIVEADDRUSE` on Windows, where a socket that shares would otherwise bind over it), and say `SKIPPED` if the port cannot be held (as on the Windows CI runner, where something else shares it, so those two tests run only on Linux there). Each test first probes whether multicast works here; if not it says so on stderr (`--nocapture`) and returns without testing anything. Set `BHAYANAKSHARE_REQUIRE_MULTICAST=1` to make that a failure instead. When Nearby Devices do not show up on a real network, see [`docs/firewall.md`](docs/firewall.md), which the Home screen links to.

The UI's accessibility is tested in `ui/src/a11y.test.tsx` (axe on every screen and dialog), `keyboard.test.tsx` (the main flows with the keyboard alone) and `announce.test.ts` / `announcer.test.tsx` (what is said to a screen reader); [`docs/accessibility.md`](docs/accessibility.md) says how it is built, what is not covered, and the checklist to run by hand before a release.

First run and the save folder, as settings of the Device, are `crates/core/tests/settings.rs`; a rename reaching a Nearby Device without a restart is in `tests/discovery.rs`.

Identity export and import is `crates/core/tests/identity.rs`, with `KeySource::File`. Where the key lives (store, file, marker) is unit-tested in `crates/core/src/keystore.rs` against a fake secret store; no test touches the real one, as it would be the developer's own key.

`TestDevice::restart` shuts a Device down and starts another on the same folders, to test resume after a clean restart. A Device that is killed instead is a separate seam: `crates/core/tests/crash.rs` runs a Receiver in a child process (this test binary again), kills it (SIGKILL; `TerminateProcess` on Windows) part-way through a fetch, and starts a Device on its folders. A restarted Device is told where to find another with `Device::note_address`, as discovery would on a real network.

The log's privacy is `crates/core/tests/log_privacy.rs`, a test binary of its own because it installs a global `tracing` subscriber (a subscriber scoped to the test's thread would miss the tasks iroh starts on its own); Settings → Diagnostics through the Device API is `crates/core/tests/diagnostics.rs`; the rolling files and the zip are unit-tested in `crates/core/src/logs.rs`, and the installed subscriber in `src-tauri/tests/logging.rs` (also its own binary, for the global subscriber and panic hook).

Two tests need more than the harness gives and do not run by default. The public DHT is tested against the real Mainline DHT in `crates/core/src/dht.rs`, which needs the internet: set `BHAYANAKSHARE_TEST_DHT=1` (`BHAYANAKSHARE_TEST_DHT=1 cargo test -p bhayanakshare-core --lib dht`). A Transfer through a rate-limited relay (`crates/core/tests/relay_limit.rs`) runs a relay server of its own, behind the `relay-tests` feature: `cargo test -p bhayanakshare-core --features relay-tests --test relay_limit -- --nocapture`. The rest of reachability (a Contact's last known address used when no lookup works) is in `crates/core/tests/reachability.rs`, on the ordinary harness.
