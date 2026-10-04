<img src="assets/icon.svg" width="64" height="64" alt="">

# BhayanakShare

A desktop app for sending files directly from one Device to another, AirDrop-style. The design is in [`docs/spec/v1.md`](docs/spec/v1.md), the vocabulary in [`CONTEXT.md`](CONTEXT.md).

Created in [T3 Code](https://t3.codes).

## Layout

| Path | What it is |
|---|---|
| `crates/core` | The Rust core, with no Tauri dependency: Device API, control protocol, blob stores, SQLite persistence. All integration tests live here. |
| `src-tauri` | The Tauri 2 shell: starts a core `Device`, exposes its commands to the UI, forwards its event stream. |
| `ui` | React + TypeScript + Vite front end (placeholders for now). |

The core's public seam is `Device`: it is created from a data folder, a save folder, a key source and an injected clock; commands go in as methods and everything that happens comes back, in order, on one `EventStream`. The shell and every test use only that.

## Commands

Requirements: a Rust toolchain, Node 22+ and pnpm. Building the Tauri shell on Linux also needs `pkg-config` and the `webkit2gtk-4.1`, `libsoup-3.0` and GTK 3 development packages (see the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)).

```sh
pnpm install

pnpm typecheck   # cargo check --workspace --all-targets (core + shell), then tsc for the UI
pnpm test        # cargo test (core: unit + integration tests), then the UI tests (vitest)

pnpm dev         # run the desktop app with hot reload
pnpm build       # build installers
```

Plain `cargo check` and `cargo test` at the repository root cover only the core (the workspace's `default-members`), so they need no system libraries. Add `--workspace` to include the shell.

## Tests

Integration tests drive Devices through the Device API only. `crates/core/tests/support` starts 2 or 3 Devices in one process on localhost, with relays and address lookup off; a test hands one Device another's address (`TestDevice::addr`) and reads each Device's event stream. Every Device gets its own temp folders and a `ManualClock`. Start new integration tests from that harness.
