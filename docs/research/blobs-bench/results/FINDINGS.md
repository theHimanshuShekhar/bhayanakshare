# iroh-blobs 0.103 fs-store on Linux: benchmark findings (issue #18)

Harness: `src/main.rs` (`blobbench`), scenarios in `run.sh`, raw logs in `results/*.log`.
Every number below is from those logs; `SYS` lines in each log record load and IO pressure.

## Environment and caveats

- Proxmox **LXC container**: 6 vCPUs of an AMD Ryzen 7 5700G host, 8 GiB RAM, kernel 7.0.14-6-pve.
- Store and source on **ext4 on a loop-mounted raw image** (`/var/lib/vz/images/165/...raw`); the host's
  backing device is not visible from the container. No reflink. `/dev/shm` is tmpfs.
- iroh-blobs 0.103.0, iroh 1.3.0, redb 4.3.0, rustc 1.99.0, release build. Sender and Receiver are two
  processes on 127.0.0.1 (relay off).
- **Heavily shared host.** Load average 13 to 40 and IO pressure (`/proc/pressure/io` some avg10) 1% to
  85% during runs, with other workloads in the same container at 0% idle CPU. Disk-bound numbers vary
  by up to 6x between back-to-back reps. Treat disk throughput as a lower bound and compare ratios only
  within the same run. Chunking overhead was also measured on tmpfs to take the disk out of the picture.
- "Cold cache" is approximate: `posix_fadvise(DONTNEED)` empties the container's cache, but the host
  still caches the loop image. That is why cold 1 GiB imports run at about 1 GiB/s.
- **Scaled down:** a 10 GiB file instead of 50 GB (74 GB free, and writes ran at 17 MiB/s while the
  10 GiB file was generated in 597 s). Kept: 100k small files of 1 to 8 KiB (440 MiB). Everything
  measured streams, so time scales linearly and RSS stays flat with size.

## Numbers

### Big file (10 GiB)

| What | Result | Peak RSS | Conditions |
|---|---|---|---|
| Sender import, TryReference, cold | 58.8 s (174 MiB/s); store 41 MB (outboard only, 0.39%) | 9.5 MiB (26.5 MiB while serving) | IO psi 37-85% |
| Sender import 1 GiB: TryReference vs Copy | 0.92 s vs 30.5 s | 10 MiB | IO psi 60% |
| Hashing, warm cache, 1 GiB | 0.98 s (~1.0 GiB/s) | 10 MiB | load 25 |
| Fetch, single request, ext4 | 19.3 MiB/s (530 s); other single-request runs 31-42 MiB/s | 27 MiB | IO psi 37-85% |
| Fetch, single request, ext4 to tmpfs, 500 MiB (earlier run) | 283 MiB/s | 26 MiB | load 35 |
| Export Copy, same ext4 | 479 s; **disk use 2x** (store 10.04 GiB + save folder 10 GiB) | 27 MiB | IO psi 55-70% |
| Export TryReference, same ext4 | **0.003 s** (rename); store shrinks to 41 MB | 19 MiB | |
| Export 500 MiB to tmpfs (other filesystem): Copy / TryReference | 0.27 s / 0.27 s (TryReference falls back to a copy) | 27 MiB | |

### Many small files (100,000 files, 1-8 KiB, 440 MiB)

| What | Result | Peak RSS |
|---|---|---|
| Sender import + collection | 19.8 s, **5,060 files/s** | **697 MiB** |
| Store size (both sides) | blobs.db 1.08 GB apparent / 636 MiB allocated (files of 16 KiB or less are inlined in redb) | |
| Fetch (warm sender) | 19.3 s, **5,170 files/s**, 23 MiB/s | **720 MiB** |
| Export Copy of 100k files, same ext4 | 3.1 s (32k files/s); tree verified identical | 756 MiB |
| kill -9 at 200 MB, then restart | 209.4 MB of 209.7 MB survived; computing what is missing across 100k children took 5.0 s; finishing took 101 s at 2.4 MiB/s (restarted sender, cold redb, IO psi 30%+) | 874 MiB |

The ~700-870 MiB RSS comes from redb's **default 1 GiB page cache**. iroh-blobs opens redb with
`Database::create` and does not expose the cache size. Big-file transfers stay under 35 MiB.

### Resume, 10 GiB file, stopped at 3.04 GiB

| How it stopped | Survived on restart | Restart cost | Notes |
|---|---|---|---|
| Clean (`shutdown()`) | 3,263,168,512 B (all) | 0.006 s | `shutdown()` took 10.5 s (fsyncs the partial) |
| kill -9, single request | 3,263,070,208 B (all) | 6.6 s re-hash | bitfield rebuilt from outboard |
| kill -9, 64 MiB chunks (mid-chunk) | 3,263,545,344 B (all, including the partial chunk) | 16.6 s re-hash | |
| kill -9, 16 MiB chunks (mid-chunk) | 3,263,266,816 B (all) | 12.3 s re-hash | |

All restarts completed and verified (`complete_after true`). Re-hashing ran at roughly 200-470 MiB/s
here, likely partly from page cache. A 50 GB partial would take an estimated 2-5 min.

### When does kill -9 lose progress? (`kill-window.log`, 1 GiB, ext4)

| Kill at | Single request | 16 MiB chunks | 64 MiB chunks |
|---|---|---|---|
| 8 MiB | lost (0.28 s in) | lost (0.12 s) | lost (0.06 s) |
| 32 MiB | lost (0.36 s) | **lost (0.78 s, 2 chunks finished)** | lost (0.21 s) |
| 128 MiB | lost (0.73 s) | kept 133.9 MB (1.6 s) | kept 134.2 MB (1.05 s) |
| 205 MiB (smoke test) / 3 GiB (above) | kept all (1.2 s / 60 s) | kept all | kept all |

What decides survival is time since the fetch started, about 1 s. Chunk boundaries don't matter.
The fs-store batches metadata writes in one redb write transaction for up to 1 s
(`BatchOptions::max_read_duration`, also used for write batches) before committing it. Until that
commit, a reopened store has no entry for the hash and ignores the files on disk; this is upstream
#254. After it, the entry is Partial and a missing or truncated `.bitfield` is rebuilt by re-hashing.

### Chunked vs single-request fetch

| Setup | single | 64 MiB | 16 MiB | 4 MiB |
|---|---|---|---|---|
| tmpfs, 1 GiB, 3 reps (MiB/s) | 93 / 127 / 128 | 85 / 147 / 166 | 77 / 156 / 172 | 104 / 118 / 156 |
| ext4, 1 GiB, quiet moment (`chunk-trace.log`) | - | 88.8 | 82.4 | - |
| ext4, 1 GiB, 3 reps, IO psi 6-54% (`chunked-disk1g.log`) | 122 / 25 / 63 | 20 / 41 / 48 | 79 / 27 / 25 | - |
| ext4, 10 GiB, IO psi 50-85% | 19.3 (and 31-42 in resume runs) | 26.8 | 37.8 | - |

- **Protocol overhead of chunking is negligible.** On tmpfs, the local-state check between requests
  totals 7-125 ms per GiB, even at 4 MiB. Variation between reps is larger than any chunk-size effect.
- **On disk, each chunk boundary costs one fsync of that chunk.** When a request ends, the blob's
  in-memory entry goes idle and is unloaded. Unloading runs `persist()`, which `sync_all`s data,
  outboard and sizes and writes `.bitfield`. The next request reloads the entry, which truncates the
  bitfield. `chunk-trace.log` shows a bitfield write and truncate at every boundary. Time spent at
  boundaries:
  - Quiet disk: 1.35 s per GiB at 64 MiB (12% of fetch time) and 2.2 s per GiB at 16 MiB (18%).
  - Contended disk: 12-41 s per GiB, up to 80% of wall time. The 10 GiB runs spent 214 s (64 MiB) and
    144 s (16 MiB) at boundaries.

  A single request leaves writes to background writeback instead.

### Store shutdown time (`shutdown.log`, 2 GiB)

`Store::shutdown()` ends by closing redb, which fsyncs `blobs.db`. On ext4 that fsync waits behind
whatever dirty page cache is queued for the filesystem (cgroup `file_dirty + file_writeback` shown):

| Before shutdown | dirty+writeback | shutdown |
|---|---|---|
| Right after a 2 GiB fetch | 539 MiB | 26.2 s |
| After Copy export to same ext4 (reps) | 656 / 228 MiB | 10.5 / 5.9 s |
| After a plain `cp` of 2 GiB (no iroh export) | 357 MiB | 10.9 s |
| After a cross-filesystem TryReference, then Copy export to ext4 (`xfs`) | 500 MiB | 6.6 s |
| Idle store, or export to tmpfs | 0-78 MiB | 0.01-0.46 s |

The **~62 s shutdown in the earlier `xfs.log`** (since overwritten) is the same effect. It was the
step right after a 500 MiB Copy export onto ext4, with the host at load ~35 and IO psi ~55%. The plain
`cp` row shows this is not iroh-specific. A `sync` right after a 2 GiB export took 0.5-8.3 s: iroh-blobs
fsyncs neither exported files nor a blob that has just completed (`into_complete` and export do not
call `sync_all`).

## Recommendations

1. **Chunk size.** Chunking does not deliver the crash safety #6 expects from it (see "Contradicts"
   below), and on a contended disk it costs one fsync stall per chunk.
   - Preferred: one `fetch` per Transfer, relying on kill -9 recovery by re-hash.
   - If bounded requests are kept for other reasons (progress clock, cancelling between requests), use
     **64 MiB or larger, not 16 MiB**. Overhead is about 12% at 64 MiB on a quiet disk and grows when
     the disk is busy. Chunks smaller than 64 MiB only add fsyncs.
2. **Store location.** Put the Receiver's store on the **same filesystem as the save folder** and
   export with **`ExportMode::TryReference`** (rename: 3 ms for 10 GiB, one copy on disk).
   - Copy without reflink doubles disk use and took 479 s for 10 GiB here.
   - Across filesystems, TryReference falls back to a copy **and the store's own `.data` file stays
     behind**: `handle_set` never schedules it for deletion (`xfs.log`).
   - After a reference export, the store points at the user's file (#233 risk).
   - In all cases, delete the per-Transfer store directory right after Saving.
3. **fsync the save folder ourselves** (files, then parent directories) before marking a Transfer
   Completed and deleting the store. iroh-blobs does not, so a power cut after "Completed" could lose
   data. That cost then lands in Saving, where the UI can show it.
4. **`Store::shutdown()` can take tens of seconds** on a busy disk after large writes. Run it with
   progress feedback and a time limit on quit. Kill -9 recovery makes an unclean exit safe, just
   slower on next start.
5. **Show a "Checking downloaded data" state on restart after a crash.** Re-hashing took 7-17 s for
   3 GiB here, an estimated 2-5 min for a 50 GB partial. A clean shutdown avoids it (6 ms).
6. **Many small files.** Budget about 1 GiB RSS per side (redb cache) and about 1.4x payload for the
   store database.
   - 100k files took about 20 s each way.
   - A restarted Sender serving small files from a cold redb was 10x slower under IO pressure. Keeping
     the Sender's store warm, or importing just before serving, helps.
7. **Sender:** keep `ImportMode::TryReference` (33x faster than Copy for 1 GiB; the store holds only
   the outboard).

## Contradicts decisions in earlier issues

- **#6, "Each finished chunk is persisted, so a crash loses at most one chunk, which works around
  upstream #254":**
  - A single-request fetch killed after about 1 s lost nothing (205 MiB and 3 GiB).
  - A chunked fetch killed before about 1 s lost everything, including finished chunks (16 MiB chunks,
    killed at 32 MiB / 0.78 s).
  - The bitfield persisted at a chunk boundary is truncated when the entry is reloaded, so a kill -9
    mid-chunk still triggers a full re-hash.
  - Chunking neither causes nor is needed for crash survival; the ~1 s redb commit window is what
    matters, and it loses at most about 1 s of data.
- **#4, "A hard kill can lose the progress":** on Linux this happens only in the first ~1 s of a fetch.
  The #254 report killed at 250 ms. Its "stopped, then 2 s idle" row is explained by time passing,
  not by the fetch ending.
- **#4, "TryReference ... the store then references the exported file":** true, but across
  filesystems the store also keeps its own copy (leaked until the store directory is deleted).
