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

Copy the Device ID from one instance's "My ID", choose "Send to ID…" in the other, and paste it. Without the variables the data lives in the platform's app data folder and files are saved to `~/Downloads/BhayanakShare`.

### Where the secret key lives

The Device ID comes from a secret key, which is kept in the OS secret store (Secret Service on Linux, Keychain on macOS, Credential Manager on Windows; entry `bhayanakshare` / `device-secret-key`), or in `secret.key` in the data folder, mode 0600, where there is no secret store. `key-location` beside it says which. Where it says `os-store`, a store that is locked or not running stops the app with an error (shown in a dialog): it never makes a new key, so it never gives the Device a new Device ID. The one accepted gap is a first start with nothing recorded: if the data folder was cleared while the Secret Service was not running, the old key in the store cannot be seen, and a new key is made in a file. A `secret.key` left by an older version moves into the store on the next start, and is deleted only after the store has given the same key back. An instance with its own `BHAYANAKSHARE_DATA_DIR` keeps its key in that folder instead, as the store has one entry for all installs of a user.

Settings → Identity exports the key alone, under a password, as a `.bhid` file, and imports one on another install. Contacts and History are neither exported nor touched by an import.

### Privacy and diagnostics

BhayanakShare logs what it does to files in `logs/` in its data folder: one file a day, the last 7 kept and never more than 50 MB in all, so a week of use at most. A line says when, how serious and what happened, and names another Device only by its Fingerprint, never its whole ID; no file name, folder name or text is ever logged, and neither is a path inside the save folder. Settings → Diagnostics has a "Debug logging" switch (a bigger log, which follows the same rules) and "Export diagnostics…", which saves a zip of the log files and an `about.txt` (app version, operating system, Visibility, whether debug logging is on, the network, this Device's Fingerprint) wherever the user says. Nothing is sent anywhere: there is no telemetry and no crash reporting, and a crash only leaves a line in the local log (where it happened, and its message if that is a fixed one). Passing the zip on is up to the user.

For code: log a Device with `DeviceId::fingerprint()` (never `{id}` on a `DeviceId` or an `EndpointId`), never log a path under a save folder or a source being sent (use the Transfer ID), and never log a `Message` or an `Offer` with `{:?}`. `crates/core/tests/log_privacy.rs` runs Transfers, refusals and failures with the app's own filter at debug (`log_filter(true)`) and fails if the log holds a whole Device ID, a name or text. The subscriber is installed by the shell (`src-tauri/src/logging.rs`), the filter, the rolling files and the export are in `crates/core/src/logs.rs`. `RUST_LOG` is not read; a debug build also writes to stderr.

### Share links

"My ID" shows `bhayanakshare://add/<Device ID>?name=<Device Name>` and its QR code. The link is made and read in the UI (`ui/src/shareLink.ts`), which matches the Device ID in it with the same pattern `ui/src/contacts.ts` uses for a pasted ID: a link is accepted wherever a Device ID is pasted, and the Rust commands still take the bare ID. Only the Device ID decides whether a link is valid. The name is a suggestion: it is made fit to be a Contact's name (control characters removed, trimmed, cut to 64 characters, where the core would refuse a name that is not already clean) and dropped if its percent-escapes cannot be read. It fills the name field of Add Contact, never the identity: the Fingerprint check is unchanged. A link opened while another dialog is open waits until that is closed.

The shell registers the scheme with the `deep-link` plugin (`tauri.conf.json`), and on Linux calls `register_all()` at every start so a moved AppImage registers itself again. The UI gets opened links, and the one that started the app, from the plugin's JavaScript API. To try one without a second machine, start the app without `BHAYANAKSHARE_DATA_DIR` and run `xdg-open 'bhayanakshare://add/<Device ID>?name=Test'`.

"Scan QR code…" in Add Contact uses the webcam (`getUserMedia`, decoded by `jsqr`); the QR code is drawn by `uqr`. On Linux, WebKitGTK keeps the camera off, so the shell turns it on for the main window and allows video-only requests.

## Tests

Integration tests drive Devices through the Device API only. `crates/core/tests/support` starts 2 or 3 Devices in one process on localhost, with relays and address lookup off; a test hands one Device another's address (`TestDevice::addr`) and reads each Device's event stream. Every Device gets its own temp folders and a `ManualClock`. Start new integration tests from that harness.

LAN discovery is its own seam, `crates/core/tests/discovery.rs`: Devices started with `TestDevice::start_discovering` use real mDNS multicast on the loopback interface (UDP port 5353, shared with anything else on the machine, so a test looks for one specific Device and never expects an exact list). Hidden Visibility is `crates/core/tests/hidden.rs`, a binary of its own that runs one test at a time: to show that a Hidden Device sends nothing it listens for a few seconds and hears nothing, which means nothing while other tests announce. It uses the same harness and the raw mDNS helpers of `tests/support/multicast.rs` (a sniffer on the group, and queries from a Device that holds no ID; the blinded label is spelled out there, so a change to the design fails the tests). Each test first probes whether multicast works here; if not it says so on stderr (`--nocapture`) and returns without testing anything. Set `BHAYANAKSHARE_REQUIRE_MULTICAST=1` to make that a failure instead. When Nearby Devices do not show up on a real network, see [`docs/firewall.md`](docs/firewall.md), which the Home screen links to.

Identity export and import is `crates/core/tests/identity.rs`, with `KeySource::File`. Where the key lives (store, file, marker) is unit-tested in `crates/core/src/keystore.rs` against a fake secret store; no test touches the real one, as it would be the developer's own key.

`TestDevice::restart` shuts a Device down and starts another on the same folders, to test resume after a clean restart. A Device that is killed instead is a separate seam: `crates/core/tests/crash.rs` runs a Receiver in a child process (this test binary again), kills it with SIGKILL part-way through a fetch, and starts a Device on its folders. A restarted Device is told where to find another with `Device::note_address`, as discovery would on a real network.

The log's privacy is `crates/core/tests/log_privacy.rs`, a test binary of its own because it installs a global `tracing` subscriber (a subscriber scoped to the test's thread would miss the tasks iroh starts on its own); Settings → Diagnostics through the Device API is `crates/core/tests/diagnostics.rs`; the rolling files and the zip are unit-tested in `crates/core/src/logs.rs`, and the installed subscriber in `src-tauri/tests/logging.rs` (also its own binary, for the global subscriber and panic hook).

Two tests need more than the harness gives and do not run by default. The public DHT is tested against the real Mainline DHT in `crates/core/src/dht.rs`, which needs the internet: set `BHAYANAKSHARE_TEST_DHT=1` (`BHAYANAKSHARE_TEST_DHT=1 cargo test -p bhayanakshare-core --lib dht`). A Transfer through a rate-limited relay (`crates/core/tests/relay_limit.rs`) runs a relay server of its own, behind the `relay-tests` feature: `cargo test -p bhayanakshare-core --features relay-tests --test relay_limit -- --nocapture`. The rest of reachability (a Contact's last known address used when no lookup works) is in `crates/core/tests/reachability.rs`, on the ordinary harness.
