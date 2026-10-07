# Research notes

The research behind the decisions in [`docs/spec/v1.md`](../spec/v1.md), kept as it was written (2026-10-03 to 2026-10-04). Versions and claims are as of then; the spec and the code are what is current.

- [`contacts-only-visibility.md`](contacts-only-visibility.md): whether a Device can be visible to Contacts only on the LAN ([#3](https://github.com/theHimanshuShekhar/bhayanakshare/issues/3)). Behind Visibility, the blinded beacon and the Hidden responder.
- [`iroh-blobs-capabilities.md`](iroh-blobs-capabilities.md): what iroh-blobs 0.103 gives for resume, folders and large files ([#4](https://github.com/theHimanshuShekhar/bhayanakshare/issues/4)).
- [`n0-infrastructure.md`](n0-infrastructure.md): what the app depends on from n0's relays, DNS and the DHT ([#5](https://github.com/theHimanshuShekhar/bhayanakshare/issues/5)).
- [`blobs-bench/`](blobs-bench/): a benchmark of the iroh-blobs 0.103 fs-store on Linux ([#18](https://github.com/theHimanshuShekhar/bhayanakshare/issues/18)). [`results/FINDINGS.md`](blobs-bench/results/FINDINGS.md) has the findings and `results/*.log` the raw runs. It is a crate of its own, outside the app's workspace: `cargo build --release` in that folder, then `./run.sh <scenario>`. Its `Cargo.lock` was not kept, so a rebuild resolves dependencies afresh.
