//! Benchmark harness for iroh-blobs 0.103 fs-store on iroh 1.3.
//!
//! Subcommands (flags are `--key value`):
//!   gen-file  --path P --size-mib N          random file
//!   gen-files --dir D --count N --min B --max B   many random small files (flat dir)
//!   send --store S --path P --kind file|dir --import copy|ref --ticket-out T
//!        imports, writes a BlobTicket to T, serves until SIGTERM/SIGINT
//!   recv --store S --ticket T [--export D] [--export-mode copy|ref]
//!        [--stop-after BYTES --stop-mode clean|kill] [--chunk-mib N]
//!        --chunk-mib fetches a raw blob as sequential ranged requests of N MiB
//!
//! Every measurement is printed as `METRIC <name> <value>` on stdout.

use std::{
    collections::HashMap,
    io::Write,
    path::PathBuf,
    str::FromStr,
    time::Instant,
};

use anyhow::{Context, Result, bail};
use futures::{StreamExt, TryStreamExt, stream};
use iroh::{Endpoint, RelayMode, endpoint::presets, protocol::Router};
use iroh_blobs::{
    BlobFormat, BlobsProtocol, HashAndFormat,
    api::{
        blobs::{AddPathOptions, ExportMode, ExportOptions, ImportMode},
        remote::{GetProgress, GetProgressItem},
    },
    format::collection::Collection,
    protocol::{ChunkRanges, ChunkRangesExt, GetRequest},
    store::fs::FsStore,
    ticket::BlobTicket,
};

fn metric(name: &str, value: impl std::fmt::Display) {
    println!("METRIC {name} {value}");
    std::io::stdout().flush().ok();
}

/// Peak and current resident set size in KiB, from /proc/self/status.
fn rss_kib() -> (u64, u64) {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let get = |k: &str| {
        s.lines()
            .find(|l| l.starts_with(k))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    };
    (get("VmHWM:"), get("VmRSS:"))
}

/// Dirty + writeback page cache (MiB): data an fsync may have to wait behind. Reads the
/// cgroup v2 memory.stat (inside LXC, /proc/meminfo is virtualised and reports 0).
fn report_dirty(phase: &str) {
    let stat = std::fs::read_to_string("/sys/fs/cgroup/memory.stat").unwrap_or_default();
    let get = |k: &str| -> u64 {
        stat.lines()
            .find_map(|l| l.strip_prefix(k)?.strip_prefix(' ')?.trim().parse().ok())
            .unwrap_or(0)
    };
    metric(&format!("{phase}.dirty_mib"), (get("file_dirty") + get("file_writeback")) >> 20);
}

fn report_rss(phase: &str) {
    let (hwm, rss) = rss_kib();
    metric(&format!("{phase}.peak_rss_mib"), format!("{:.1}", hwm as f64 / 1024.0));
    metric(&format!("{phase}.rss_mib"), format!("{:.1}", rss as f64 / 1024.0));
}

struct Args(HashMap<String, String>);

impl Args {
    fn parse(raw: &[String]) -> Result<Self> {
        let mut m = HashMap::new();
        let mut it = raw.iter();
        while let Some(k) = it.next() {
            let k = k.strip_prefix("--").context("expected --flag")?;
            let v = it.next().context("missing flag value")?;
            m.insert(k.to_string(), v.clone());
        }
        Ok(Self(m))
    }
    fn get(&self, k: &str) -> Result<&str> {
        self.0.get(k).map(|s| s.as_str()).with_context(|| format!("--{k} required"))
    }
    fn opt(&self, k: &str) -> Option<&str> {
        self.0.get(k).map(|s| s.as_str())
    }
    fn num<T: FromStr>(&self, k: &str) -> Result<T>
    where
        T::Err: std::error::Error + Send + Sync + 'static,
    {
        Ok(self.get(k)?.parse()?)
    }
}

/// xorshift64* - fast, incompressible enough for a transfer benchmark.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn fill(&mut self, buf: &mut [u8]) {
        for c in buf.chunks_mut(8) {
            let v = self.next().to_le_bytes();
            c.copy_from_slice(&v[..c.len()]);
        }
    }
}

fn gen_file(a: &Args) -> Result<()> {
    let path = PathBuf::from(a.get("path")?);
    let size: u64 = a.num::<u64>("size-mib")? << 20;
    let mut f = std::io::BufWriter::with_capacity(8 << 20, std::fs::File::create(&path)?);
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut buf = vec![0u8; 4 << 20];
    let t = Instant::now();
    let mut done = 0u64;
    while done < size {
        rng.fill(&mut buf);
        let n = buf.len().min((size - done) as usize);
        f.write_all(&buf[..n])?;
        done += n as u64;
    }
    f.into_inner()?.sync_all()?;
    metric("gen.secs", format!("{:.2}", t.elapsed().as_secs_f64()));
    Ok(())
}

fn gen_files(a: &Args) -> Result<()> {
    let dir = PathBuf::from(a.get("dir")?);
    let count: usize = a.num("count")?;
    let min: usize = a.num("min")?;
    let max: usize = a.num("max")?;
    std::fs::create_dir_all(&dir)?;
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    let mut buf = vec![0u8; max];
    let mut total = 0u64;
    for i in 0..count {
        let len = min + (rng.next() as usize % (max - min + 1));
        rng.fill(&mut buf[..len]);
        std::fs::write(dir.join(format!("f{i:06}.bin")), &buf[..len])?;
        total += len as u64;
    }
    metric("gen.files", count);
    metric("gen.bytes", total);
    Ok(())
}

async fn endpoint(alpns: Vec<Vec<u8>>) -> Result<Endpoint> {
    Ok(Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_ip_transports()
        .bind_addr("127.0.0.1:0")?
        .alpns(alpns)
        .bind()
        .await?)
}

async fn send(a: &Args) -> Result<()> {
    let store_dir = PathBuf::from(a.get("store")?);
    let path = std::path::absolute(a.get("path")?)?;
    let mode = match a.get("import")? {
        "copy" => ImportMode::Copy,
        "ref" => ImportMode::TryReference,
        m => bail!("bad import mode {m}"),
    };
    let t = Instant::now();
    let store = FsStore::load(&store_dir).await?;
    metric("send.store_load_secs", format!("{:.3}", t.elapsed().as_secs_f64()));

    // Keep temp tags alive until the root is protected by a named tag.
    let mut keep = Vec::new();
    let t = Instant::now();
    // A restarted Sender reuses its named tag instead of re-hashing (unless --reimport yes).
    let existing = store.tags().get("bench-root").await?;
    metric("send.reused_tag", existing.is_some() && a.opt("reimport").is_none());
    let content = match a.get("kind")? {
        _ if existing.is_some() && a.opt("reimport").is_none() => existing.unwrap().into(),
        "file" => {
            let tt = store
                .blobs()
                .add_path_with_opts(AddPathOptions { path, format: BlobFormat::Raw, mode })
                .temp_tag()
                .await?;
            let hf = tt.hash_and_format();
            keep.push(tt);
            hf
        }
        "dir" => {
            let mut names: Vec<_> = std::fs::read_dir(&path)?
                .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
                .collect::<std::io::Result<_>>()?;
            names.sort();
            metric("send.files", names.len());
            let store2 = store.clone();
            let items: Vec<(String, iroh_blobs::api::TempTag)> = stream::iter(names)
                .map(|name| {
                    let store = store2.clone();
                    let p = path.join(&name);
                    async move {
                        let tt = store
                            .blobs()
                            .add_path_with_opts(AddPathOptions {
                                path: p,
                                format: BlobFormat::Raw,
                                mode,
                            })
                            .temp_tag()
                            .await?;
                        anyhow::Ok((name, tt))
                    }
                })
                .buffered(64)
                .try_collect()
                .await?;
            metric("send.import_children_secs", format!("{:.3}", t.elapsed().as_secs_f64()));
            let coll: Collection =
                items.iter().map(|(n, tt)| (n.clone(), tt.hash())).collect();
            let tt = coll.store(&store).await?;
            let hf = tt.hash_and_format();
            keep.extend(items.into_iter().map(|(_, tt)| tt));
            keep.push(tt);
            hf
        }
        k => bail!("bad kind {k}"),
    };
    store.tags().set("bench-root", content).await?;
    store.sync_db().await?;
    metric("send.import_secs", format!("{:.3}", t.elapsed().as_secs_f64()));
    report_rss("send.after_import");

    let ep = endpoint(vec![iroh_blobs::ALPN.to_vec()]).await?;
    let ticket = BlobTicket::new(ep.addr(), content.hash, content.format);
    std::fs::write(a.get("ticket-out")?, ticket.to_string())?;
    eprintln!("send: serving {} ({:?})", content.hash, content.format);

    let blobs = BlobsProtocol::new(&store, None);
    let router = Router::builder(ep).accept(iroh_blobs::ALPN, blobs).spawn();
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        _ = term.recv() => {},
        _ = tokio::signal::ctrl_c() => {},
    }
    report_rss("send.final");
    // Router shutdown calls BlobsProtocol::shutdown, which shuts the store down.
    drop(keep);
    router.shutdown().await?;
    Ok(())
}

/// Progress across one or more fetch requests; byte counts are cumulative over requests.
#[derive(Default)]
struct FetchState {
    t_start: Option<Instant>,
    stop_after: Option<u64>,
    first_byte_seen: bool,
    payload: u64,
    other: u64,
    last: u64,
    requests: u64,
}

impl FetchState {
    /// Drive one request to completion. Returns true if stopped at `stop_after`.
    async fn drain(&mut self, progress: GetProgress) -> Result<bool> {
        let mut s = std::pin::pin!(progress.stream());
        while let Some(item) = s.next().await {
            match item {
                GetProgressItem::Progress(n) => {
                    let total = self.payload + n;
                    if !self.first_byte_seen && n > 0 {
                        self.first_byte_seen = true;
                        let fb = self.t_start.map_or(0.0, |t| t.elapsed().as_secs_f64());
                        metric("recv.first_payload_byte_since_start_secs", format!("{fb:.3}"));
                    }
                    self.last = total;
                    if self.stop_after.is_some_and(|limit| total >= limit) {
                        return Ok(true);
                    }
                }
                GetProgressItem::Done(stats) => {
                    self.payload += stats.payload_bytes_read;
                    self.other += stats.other_bytes_read;
                    self.last = self.payload;
                }
                GetProgressItem::Error(e) => bail!("fetch failed: {e:?}"),
            }
        }
        Ok(false)
    }
}

async fn recv(a: &Args) -> Result<()> {
    let t_start = Instant::now();
    let ticket: BlobTicket = std::fs::read_to_string(a.get("ticket")?)?.trim().parse()?;
    let content = HashAndFormat { hash: ticket.hash(), format: ticket.format() };
    let stop_after: Option<u64> = a.opt("stop-after").map(|s| s.parse()).transpose()?;
    let stop_mode = a.opt("stop-mode").unwrap_or("clean").to_string();

    let t = Instant::now();
    let store = FsStore::load(a.get("store")?).await?;
    metric("recv.store_load_secs", format!("{:.3}", t.elapsed().as_secs_f64()));

    // Protect partial and complete data from GC (GC is off by default, but be explicit).
    store.tags().set("bench-root", content).await?;

    let t = Instant::now();
    let local = store.remote().local(content).await?;
    metric("recv.local_info_secs", format!("{:.3}", t.elapsed().as_secs_f64()));
    metric("recv.local_bytes_before", local.local_bytes());
    metric("recv.complete_before", local.is_complete());

    let ep = endpoint(vec![]).await?;
    let t = Instant::now();
    let conn = ep.connect(ticket.addr().clone(), iroh_blobs::ALPN).await?;
    metric("recv.connect_secs", format!("{:.3}", t.elapsed().as_secs_f64()));

    let t_fetch = Instant::now();
    let mut fs = FetchState { t_start: Some(t_start), stop_after, ..Default::default() };
    // --chunk-mib N: fetch a Raw blob as a sequence of bounded ranged requests (what the
    // Receiver does to work around iroh-blobs #254), each asking only for missing ranges.
    let chunk: Option<u64> = a.opt("chunk-mib").map(|s| s.parse::<u64>()).transpose()?.map(|m| m << 20);
    let stopped = match chunk {
        None => fs.drain(store.remote().fetch(conn.clone(), content)).await?,
        Some(chunk) => {
            if content.format != BlobFormat::Raw {
                bail!("--chunk-mib only supports raw blobs");
            }
            metric("recv.chunk_mib", chunk >> 20);
            let mut start = 0u64;
            let mut size: Option<u64> = None;
            let mut stopped = false;
            let (mut local_secs, mut get_secs) = (0f64, 0f64);
            while size.is_none_or(|s| start < s) {
                let req =
                    GetRequest::blob_ranges(content.hash, ChunkRanges::bytes(start..start + chunk));
                let t0 = Instant::now();
                let local = store.remote().local_for_request(req).await?;
                let t1 = Instant::now();
                if !local.is_complete() {
                    fs.requests += 1;
                    if fs.drain(store.remote().execute_get(conn.clone(), local.missing())).await? {
                        stopped = true;
                        break;
                    }
                }
                let t2 = Instant::now();
                if size.is_none() {
                    // Size the sender reported in the first response (or known locally).
                    size = Some(store.blobs().observe(content.hash).await?.size());
                }
                local_secs += (t1 - t0).as_secs_f64();
                get_secs += (t2 - t1).as_secs_f64();
                if std::env::var_os("CHUNK_TRACE").is_some() {
                    eprintln!(
                        "chunk {start}: local {:.3}s get {:.3}s observe {:.3}s",
                        (t1 - t0).as_secs_f64(),
                        (t2 - t1).as_secs_f64(),
                        t2.elapsed().as_secs_f64()
                    );
                }
                start += chunk;
            }
            metric("recv.chunk_requests", fs.requests);
            // Time spent checking local state between requests vs inside requests.
            metric("recv.chunk_local_check_secs_total", format!("{local_secs:.3}"));
            metric("recv.chunk_get_secs_total", format!("{get_secs:.3}"));
            stopped
        }
    };
    if !stopped {
        let secs = t_fetch.elapsed().as_secs_f64();
        metric("recv.fetch_secs", format!("{secs:.3}"));
        metric("recv.payload_bytes_read", fs.payload);
        metric("recv.other_bytes_read", fs.other);
        metric("recv.throughput_mib_s", format!("{:.1}", fs.payload as f64 / (1 << 20) as f64 / secs));
    }
    if stopped {
        let last = fs.last;
        metric("recv.stopped_at_bytes", last);
        metric("recv.stopped_after_secs", format!("{:.3}", t_fetch.elapsed().as_secs_f64()));
        report_rss("recv.at_stop");
        if stop_mode == "kill" {
            eprintln!("recv: SIGKILL self at {last} bytes");
            unsafe { libc::kill(libc::getpid(), libc::SIGKILL) };
            unreachable!();
        }
        conn.close(0u32.into(), b"stop");
        let t = Instant::now();
        store.shutdown().await?;
        metric("recv.shutdown_secs", format!("{:.3}", t.elapsed().as_secs_f64()));
        return Ok(());
    }
    conn.close(0u32.into(), b"done");
    report_rss("recv.after_fetch");
    let local = store.remote().local(content).await?;
    metric("recv.complete_after", local.is_complete());

    if let Some(dest) = a.opt("export") {
        let dest = std::path::absolute(dest)?;
        let mode = match a.opt("export-mode").unwrap_or("copy") {
            "copy" => ExportMode::Copy,
            "ref" => ExportMode::TryReference,
            m => bail!("bad export mode {m}"),
        };
        std::fs::create_dir_all(&dest)?;
        let t = Instant::now();
        match content.format {
            BlobFormat::Raw => {
                let target = dest.join("file.bin");
                store
                    .blobs()
                    .export_with_opts(ExportOptions { hash: content.hash, mode, target })
                    .finish()
                    .await?;
            }
            BlobFormat::HashSeq => {
                let coll = Collection::load(content.hash, store.as_ref()).await?;
                metric("recv.collection_len", coll.len());
                let jobs: Vec<_> = coll.iter().cloned().collect();
                stream::iter(jobs)
                    .map(|(name, hash)| {
                        let store = store.clone();
                        let target = dest.join(&name);
                        async move {
                            store
                                .blobs()
                                .export_with_opts(ExportOptions { hash, mode, target })
                                .finish()
                                .await?;
                            anyhow::Ok(())
                        }
                    })
                    .buffer_unordered(64)
                    .try_collect::<()>()
                    .await?;
            }
        }
        metric("recv.export_secs", format!("{:.3}", t.elapsed().as_secs_f64()));
        report_rss("recv.after_export");
    }
    report_dirty("recv.before_shutdown");
    let t = Instant::now();
    store.shutdown().await?;
    metric("recv.shutdown_secs", format!("{:.3}", t.elapsed().as_secs_f64()));
    metric("recv.total_secs", format!("{:.3}", t_start.elapsed().as_secs_f64()));
    ep.close().await;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, rest) = raw.split_first().context("subcommand required")?;
    let a = Args::parse(rest)?;
    match cmd.as_str() {
        "gen-file" => gen_file(&a),
        "gen-files" => gen_files(&a),
        "send" => send(&a).await,
        "recv" => recv(&a).await,
        c => bail!("unknown subcommand {c}"),
    }
}

