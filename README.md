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

An instance given `BHAYANAKSHARE_DATA_DIR` is treated as a separate install made to run beside another: it skips the single-instance check and does not register itself to start at login. (Without the variable, a second launch focuses the running window instead and hands it any files named on the command line.)

Copy the Device ID from one instance's "My ID", choose "Send to ID…" in the other, and paste it. Without the variables the data lives in the platform's app data folder and files are saved to `~/Downloads/BhayanakShare`.

## Tests

Integration tests drive Devices through the Device API only. `crates/core/tests/support` starts 2 or 3 Devices in one process on localhost, with relays and address lookup off; a test hands one Device another's address (`TestDevice::addr`) and reads each Device's event stream. Every Device gets its own temp folders and a `ManualClock`. Start new integration tests from that harness.

LAN discovery is its own seam, `crates/core/tests/discovery.rs`: Devices started with `TestDevice::start_discovering` use real mDNS multicast on the loopback interface (UDP port 5353, shared with anything else on the machine, so a test looks for one specific Device and never expects an exact list). Each test first probes whether multicast works here; if not it says so on stderr (`--nocapture`) and returns without testing anything. Set `BHAYANAKSHARE_REQUIRE_MULTICAST=1` to make that a failure instead. When Nearby Devices do not show up on a real network, see [`docs/firewall.md`](docs/firewall.md), which the Home screen links to.

`TestDevice::restart` shuts a Device down and starts another on the same folders, to test resume after a clean restart. A Device that is killed instead is a separate seam: `crates/core/tests/crash.rs` runs a Receiver in a child process (this test binary again), kills it with SIGKILL part-way through a fetch, and starts a Device on its folders. A restarted Device is told where to find another with `Device::note_address`, as discovery would on a real network.
