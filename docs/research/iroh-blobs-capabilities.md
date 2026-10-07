# What iroh-blobs 0.103 gives us for resume, folders and large files

Research for issue #4. Date: 2026-10-03.

Sources read:
- iroh-blobs tag `v0.103.0` (commit `e82cbdc`, released 2026-06-15; latest tag at time of writing).
- sendme tag `v0.36.0` (commit `ee8d8d6`), which depends on `iroh-blobs = "0.103"` and `iroh = "1.0.0"`.
- docs.rs for iroh-blobs 0.103.0.
- Open issues on both repos.

Links below are permalinks to those tags. `B:` means `https://github.com/n0-computer/iroh-blobs/blob/v0.103.0/`. `S:` means `https://github.com/n0-computer/sendme/blob/v0.36.0/`.

Labels:
- **[verified]**: read in source or docs.
- **[reported]**: stated in an upstream issue that I did not reproduce.
- **[inference]**: my reasoning, not yet tested.

I did not compile or run anything; there is no Rust toolchain on the research host. Every behavioural claim below comes from reading source code.

## TL;DR

- **Resume works with the API we need. It survives a clean restart on both sides but is fragile after a hard kill.** The Receiver calls `store.remote().local(content).missing()` (or `remote().fetch`) and gets back only the missing chunk ranges, including inside a Collection.
  - A partial blob lives in the fs-store as `<hash>.data`, `.obao4` and `.sizes4` files, plus a `.bitfield` file that is written on clean shutdown.
  - After an unclean exit, the bitfield is rebuilt by re-hashing the partial data. Upstream says this takes "many minutes" for giant files.
  - Open upstream issue #254: if the process is killed *during* a `fetch`, a reopened store reports the hash as `NotFound`, so the whole transfer is downloaded again.
- **Folders: a Collection is a flat list of `(String name, Hash)` and nothing else.** It has no directories (so no empty dirs), no symlinks, no permissions, no timestamps, and no name sanitisation.
  - iroh-blobs only exports one blob to one absolute path. Mapping a collection onto a folder tree is the application's job.
  - The collection format is explicitly "subject to change".
- **Large files and many files are a stated design goal.** No size limit is built in; data and outboard live in files and the outboard is about 0.4% of the data. Files of 16 KiB or less are inlined into the redb database.
  - The Sender must hash every file sequentially before a root hash exists.
  - The provider loads the whole HashSeq (32 B per file) into memory when serving collection children.
- **Copies: sendme's pattern is reference on the Sender and copy (or reflink) on the Receiver.**
  - Sender: `ImportMode::TryReference` keeps files larger than 16 KiB in place and stores only the outboard. Files of 16 KiB or less are always copied into the database.
  - Receiver: the partial download lives inside the store directory. `ExportMode::TryReference` moves it out with a rename (no second copy) when the store is on the same filesystem; otherwise it reflinks or copies.
  - On Windows the cross-volume fallback is probably broken: it checks for error 18 (EXDEV), but Windows reports 17.
- **Wire compatibility: no stated guarantee and no version negotiation.** The ALPN has been `/iroh-bytes/4` from 0.35 through 0.103, while the request enum grew new variants.
  - Within the 0.9x/0.10x line the `Get` path looks stable, but nothing promises that.
  - The README says 0.103 is "not yet considered production quality" and recommends 0.35 for production. 0.35 is not compatible with iroh 1.x.

## 1. Resume across restarts on both sides

### How it works (verified)

- **Resume API.** `Remote::local(content) -> LocalInfo` inspects the store; `LocalInfo::missing() -> GetRequest` builds a request for exactly the missing ranges. For a HashSeq it covers both the root and each child. `Remote::fetch(conn, content)` does both steps in one call. The doc comment on `Remote` describes exactly this flow.
  - Source: B:src/api/remote.rs#L51-L65, #L341-L400, #L491-L540. docs.rs: https://docs.rs/iroh-blobs/0.103.0/iroh_blobs/api/remote/struct.Remote.html
- **The protocol supports this directly.** The protocol docs include a worked example titled "Parts of hash sequences" for resuming an interrupted collection download: skip the children you have and give a chunk offset into the partially received child (B:src/protocol.rs, module docs).
  - Range granularity is a 16 KiB chunk group, and the worst-case overhead is about two chunk groups per range.
- **sendme's receiver does exactly this.** It opens `FsStore::load(".sendme-recv-<roothash>")`, runs `db.remote().local(hash_and_format)`, then `execute_get(connection, local.missing())`. To resume, you re-run with the same ticket: the store directory is keyed by the root hash and is removed only after a successful export (S:src/main.rs#L1026-L1074, #L1142).
- **On-disk state of a partial blob.** Once a blob is larger than 16 KiB, it is stored as `data/<hash>.data`, `<hash>.obao4` (outboard), `<hash>.sizes4` and `<hash>.bitfield`, with a `Partial` entry in the redb metadata table (B:src/store/fs/options.rs, B:src/store/fs/bao_file.rs#L135-L200).
  - Partial blobs of 16 KiB or less are kept only in memory and are lost on crash ("Incomplete mem entries are *not* persisted at all", bao_file.rs#L315). That is harmless; they get re-fetched.
- **Clean shutdown versus crash.**
  - `Store::shutdown()`, which also runs on `Router::shutdown()`, syncs the data, outboard and sizes files and then writes a checksummed `.bitfield` (bao_file.rs#L147-L159).
  - If the bitfield is missing or corrupt on load, it is rebuilt by validating the whole partial file against the outboard (`valid_ranges`, bao_file.rs#L161-L190).
  - The module docs warn that without a clean shutdown "operations involving large partial blobs will have a large initial delay on the next startup" and that the last seconds of writes may be lost (B:src/store/fs.rs#L48-L64).
  - DESIGN.md says that for "giant files ... it would take many *minutes* at startup to compute the validity bitfield" (B:DESIGN.md#L126).
- **GC.** GC is off by default (`Options::new` sets `gc: None`, options.rs#L124). If we turn it on, partial downloads must be protected:
  - Option 1: a named tag. `tags().set` writes any `HashAndFormat` and does not check that the hash exists (B:src/store/fs/meta.rs#L624-L638). GC marks a HashSeq tag's children by traversing the root (B:src/store/gc.rs).
  - Option 2: a `blobs().batch()` plus `temp_tag`, which lasts only for the life of the process. This pattern was documented in 0.103.0 (#236), after reports of GC deleting in-flight partial `.data` files (issue #235).
- **The Sender side.** Nothing special is needed for the Sender to resume. Content is addressed by hash, so any provider holding the same root can serve the missing ranges.
  - If the Sender's store persists (with a tag for the Transfer), it can serve again after restart without re-hashing.
  - If the store is lost, re-importing the same unchanged files with the same names in the same order gives the same root hash (sendme sorts names, S:src/main.rs#L461). The cost is re-hashing everything **[inference: follows from content addressing]**.
- **Modified source files are detected.** If a referenced file changes after import, the provider validates while sending (`traverse_ranges_validated`) and aborts. The protocol docs say "Data will be validated both on the provider and getter side". Maintainer comment on sendme #87: "The sender detects that the data is corrupt and stops sending."
  - So a modified file makes the Transfer fail. It does not deliver corrupt data.

### Caveats

- **[reported] iroh-blobs #254 (open, 2026-08-18, against 0.103.0).** "A partial is only recorded when the fetch ends: SIGKILL during remote().fetch loses all progress."
  - When the process is aborted mid-fetch, the `.data`/`.obao4`/`.sizes4` files are on disk, but a reopened store reports `NotFound` and downloads everything again.
  - Stopping the fetch, idling about 2 s and then killing the process was recorded correctly.
  - Reporter's workaround: end and re-issue the fetch periodically.
  - This directly threatens "resume after crash, power loss or OS update reboot". A clean app quit with `shutdown()` is fine.
- **[reported] iroh-blobs #233 (open).** The fs-store can panic with "poisoned storage should not be used" in two ways:
  - `persist()` poisons a `Complete` handle, triggered after a `TryReference` export once the handle is evicted for idleness.
  - `load()` poisons on `NotFound` when an External file has been deleted by the user.
  - Both apply directly to "export, then the user moves or deletes the file".
- **[inference]** Calling `Store::shutdown()` on every exit path matters a lot: Tauri exit, OS logoff or shutdown, and the updater restart.

## 2. Exporting a Collection to a folder tree

### What exists (verified)

- **The Collection format.** It is a HashSeq whose first link points to a metadata blob. That blob is the postcard encoding of `{ header: b"CollectionV0.", names: Vec<String> }`, followed by one link per file.
  - The doc comment says: "Note that the format is subject to change." (B:src/format/collection.rs#L18-L25, #L83-L118)
  - The format is unchanged since 0.35, where the `CollectionV0.` header is also present.
- **Names.** These are arbitrary UTF-8 `String`s; by convention, sendme joins relative path components with `/`. iroh-blobs attaches no meaning to them.
- **Export is per blob only.** `blobs().export(hash, target)` or `export_with_opts(ExportOptions { hash, target, mode })`:
  - The target must be absolute.
  - It calls `fs::create_dir_all(parent)`.
  - A partial entry is refused ("cannot export partial entry").
  - The file is written with `File::create`, `reflink`, `copy` or `rename`.
  - No permissions, timestamps or other metadata are set (B:src/store/fs.rs#L1217-L1330).
  - There is no "export collection" API; sendme loops over `collection.iter()` (S:src/main.rs#L487-L540).
- **Empty directories.** These are not representable. sendme walks the tree with `walkdir` and keeps only `is_file()` entries (S:src/main.rs#L383-L400).
- **Symlinks.**
  - sendme skips them on send ("Skip symlinks", S:src/main.rs#L386-L392).
  - iroh-blobs `add_path` accepts a symlink path and reads through it to the target (`!path.is_file() && !path.is_symlink()` check, B:src/store/fs/import.rs#L456).
  - Nothing in iroh-blobs can recreate a symlink.
- **Permissions and timestamps.** Not carried. sendme #101 ("maintain file permissions") was closed; the maintainer said the metadata blob would need to change and called cross-OS permissions "a can of worms".
  - **[inference]** Exported files get default permissions (umask). Their mtime is either the export time (copy) or the time the store file was last written (rename).
- **Windows-invalid names.** iroh-blobs does no sanitisation. sendme's `get_export_path` splits on `/` and `validate_path_component` only rejects `/` (S:src/main.rs#L313-L319, #L477-L485).
  - **[inference]** sendme as written does not reject these on the receiving side:
    - `..` components (path traversal).
    - `\`, which acts as a separator on Windows.
    - Drive prefixes such as `C:`.
    - Windows-reserved names (`CON`, `NUL`, `COM1`...).
    - The characters `<>:"|?*`, trailing dots and spaces.
    - Names that collide only by case on case-insensitive filesystems.
  - On NTFS, `a:b` would create an alternate data stream. The sending side does reject non-UTF-8 names and components containing `/` or `\` (`canonicalized_path_to_string`, S:src/main.rs#L329-L370).
- **Duplicate content.** Two files with identical bytes share one blob. A bug with `TryReference` export of duplicates (sendme #103) was fixed in iroh-blobs #149 (commit `4d8cade`, which is in v0.103.0). When the same hash is exported again, a reflink or copy is made from the first external path (fs.rs#L1284-L1299).

### Implications (inference)

- We need **our own manifest**. It should carry:
  - relative paths as components rather than a `/`-joined string;
  - entry kind (file, dir, symlink), so empty dirs are included;
  - size, mtime and permission bits (Unix mode and/or Windows read-only/hidden);
  - optional text payloads.
- Options for carrying the manifest:
  - In the Offer, with content as a raw HashSeq of file hashes.
  - As our own metadata blob in slot 0 of a HashSeq, which mirrors Collection without depending on `CollectionV0.`.
  - In either case we stop depending on a format marked "subject to change".
- The **Receiver must sanitise and map names itself**, rejecting traversal and handling Windows-reserved names, illegal characters and case collisions. It also applies timestamps and permissions after export.

## 3. Very large files (50 GB+) and many small files (100k+)

### Verified

- **Design goals.** "Do not limit the size of blobs or collections ... up to terabytes ... A well behaved implementation will not require the entire blob or collection to be in memory at once." Also: "Be efficient when transferring multiple tiny blobs" (B:src/protocol.rs#L19-L34).
- **Hybrid store.** Blobs of 16 KiB or less have their data inlined in redb; larger blobs are files named by hash. Outboards of 16 KiB or less are also inlined (B:src/store/fs/options.rs#L60-L78, B:DESIGN.md).
  - DESIGN.md: "We won't ever have a directory with millions of files, since tiny files never make it to the file system."
  - Both thresholds are configurable through `InlineOptions`.
- **Block size.** `IROH_BLOCK_SIZE` is `BlockSize::from_chunk_log(4)`, i.e. 16 KiB chunk groups (B:src/store/mod.rs#L18). The outboard holds 64 bytes per 16 KiB group, about 0.39% of the data.
  - For a 50 GiB file that is roughly 200 MiB of outboard on each side (computed: `(50 GiB / 16 KiB − 1) × 64 B`).
- **Hashing on import.** `add_path` computes the outboard in one sequential pass with a 1 MiB `BufReader` (B:src/util.rs#L273-L292). For 50 GB the Sender must read and hash the whole file before the root hash is known.
  - With `TryReference` there is no copy. With `Copy` it reflinks or copies to temp first (import.rs#L465-L485).
- **Collection size on the provider.** When a request touches HashSeq children, the provider loads the whole HashSeq root with `get_bytes` ("todo: this assumes ... it is small enough to fit in memory", B:src/provider.rs#L429-L440; open issue #265). For 100k files that is 3.2 MB, which is fine. The receiver's `Collection::load` also reads the whole names blob.
- **One stream for everything.** A whole collection is served on one QUIC stream in hash-seq order, with no per-file round trip (protocol docs, "Responses").
- **Resume bookkeeping.** `local_for_request` calls `observe` once per child to build `missing()` (B:src/api/remote.rs#L435-L488). That is 100k store round-trips before a resumed 100k-file transfer starts **[inference: cost unmeasured]**.
- **Receiver export cost.** With `ExportMode::Copy` a 4 GB file took about 7.5 minutes to "export" after the download finished (sendme #120, open, **[reported]**). That is the cost of copying out of the store.

### Unknowns (need a spike)

- Real throughput for 50 GB+ end to end on Windows, macOS and Linux, including the fsync behaviour of the partial file.
- Time to rebuild the bitfield after a crash at 50 GB.
- Time to hash and import 100k small files on the Sender (sendme parallelises with `num_cpus`, S:src/main.rs#L404-L455).
- Time for `missing()` on a 100k-child collection, and redb size.
- Windows performance: iroh-blobs #253 ("disk-backed blob reads should use `RandomAccessFile`") is open. The reporter later attributed their slowdown to UDP segmentation offload, but the maintainer kept the issue open.

## 4. Disk and memory overhead; copies

### Sender (verified)

- `ImportMode` has two values: `Copy` (the default) and `TryReference`, described as "try to reference the file in place and assume it is unchanged after import" (B:src/api/proto.rs#L623-L645).
- In `import_path_impl` (B:src/store/fs/import.rs#L439-L485):
  - `size <= max_data_inlined` (16 KiB) means the file is **read into memory and inlined in the database, i.e. copied, regardless of mode**.
  - `TryReference` means `DataLocation::External(path)`: no copy, only the outboard is stored.
  - `Copy` means reflink or copy into `temp/`, then rename to `data/<hash>.data`.
- sendme uses `TryReference` (S:src/main.rs#L416) with a throw-away store `.sendme-send-<random>` in the current directory, which it deletes on exit (#L673, #L783).
- **Net Sender overhead** with TryReference: about 0.4% outboard for large files, plus a full copy of every file of 16 KiB or less inside redb. Setting `InlineOptions::NO_INLINE` would avoid that copy, but then every small file gets a referenced entry **[inference from import.rs logic]**.

### Receiver (verified)

- **During download.** Data is written into the store at `data/<hash>.data` (partial, then complete `Owned`). Files of 16 KiB or less go inline into redb.
- **`ExportMode::Copy`** (the default, and what sendme uses): `reflink_or_copy_with_progress` tries `reflink_copy::reflink` first, then falls back to a 1 MiB-buffer copy (B:src/store/fs.rs#L1351-L1380).
  - Without reflink support the export is a **second full copy**, so peak disk is 2× until the store is deleted.
  - Reflink support comes from the `reflink-copy` crate, which covers Linux, macOS and Windows. In practice that means btrfs/XFS, APFS, and ReFS/Dev Drive (the crate's README states the OS coverage; the per-filesystem list is **[inference]**).
- **`ExportMode::TryReference`.** If the data is `Owned`, it is moved with `std::fs::rename` and the entry becomes `External([target])`. If the data is already External, it is reflinked or copied.
  - If rename fails with raw OS error `18` (`ERR_CROSS`), it falls back to reflink or copy; any other error is returned (fs.rs#L1284-L1320).
  - **[inference]** 18 is `EXDEV` on Linux and macOS. Windows reports a cross-volume move as `ERROR_NOT_SAME_DEVICE` (17), so on Windows a TryReference export to a different drive should fail instead of falling back. This needs testing on Windows.
- **Deleting an External entry never deletes the user's file.** Only store-owned data and outboard files are removed (B:src/store/fs/meta.rs#L585-L603).
  - `blobs().delete` is `pub(crate)` (B:src/api/blobs.rs#L166). Public cleanup goes through tags plus GC, or by deleting the whole store directory, which is what sendme does.
- **Upstream has no "move out and forget" export mode.** iroh-blobs #167 is open; the maintainer prefers a future batch API.
- **Memory.** A bitfield is a `ChunkRanges` range set, which is tiny for sequential downloads. The fs-store runs its own multi-threaded tokio runtime. Copy buffers are 1 MiB. The HashSeq is held in memory on the provider. **[inference]** No component scales with blob size in memory, but per-file overhead scales with collection size.

### Implications (inference)

- To get **one copy on the Receiver**, put the Receiver's store (or a per-Transfer store) on the **same volume as the destination folder**, as sendme does with a hidden directory in the target dir, and export with `TryReference` (rename). Alternatively, use `Copy` and accept a reflink or a 2× peak.
- Because of #233 and the Windows error-17 issue, it may be safer to export with `Copy` when reflink is available and do our own `rename` from the store's data file. That second part relies on internal paths. Either way we should then drop the Transfer's tag or store.
- On the Sender, use `TryReference` and keep a persistent store with one named tag per Transfer, so a resume after a Sender restart does not re-hash. Remove the tag when the Transfer ends.

## 5. Wire compatibility between iroh-blobs versions

### Verified

- **ALPN.** `ALPN = b"/iroh-bytes/4"` in v0.35.0, v0.90.0, v0.95.0, v0.99.0 and v0.103.0 (checked with `git grep` at each tag; B:src/protocol.rs#L406).
- **Request enum.** In 0.35 it was `Request { Get(GetRequest) }` with `RangeSpecSeq`. In 0.103 it is `{ Get, Observe, Slot2..Slot7, Push, GetMany }` with `ChunkRangesSeq`. Its first byte is a request-type tag, and unknown types are rejected with "failed to deserialize request type" (B:src/protocol.rs#L410-L490).
- **No compatibility promise.** There is no version field, no negotiation, and no compatibility statement in the README, DESIGN.md or CHANGELOG. The CHANGELOG marks changes **[breaking]** at the Rust API level (iroh upgrades) and says nothing about the wire format.
- **README:** "this version of iroh-blobs is not yet considered production quality. For now, if you need production quality, use iroh-blobs 0.35". 0.35 targets pre-1.0 iroh, so it is not an option with iroh 1.3.
- **Provider authorisation.** The provider can **intercept and reject** connections and individual get requests through `EventMask` with `ConnectMode::Intercept` and `RequestMode::Intercept`/`InterceptLog`, and can report per-blob transfer progress (B:src/provider/events.rs). Push is disabled by default.

### Implications (inference)

- Because the ALPN never changes, a mismatch surfaces as a decode or validation failure at runtime, not as a clean ALPN rejection. ADR 0001 is right that **our Offer must carry an explicit content-protocol version**. We should refuse incompatible peers at Offer time and pin the exact iroh-blobs version (`=0.103.0`).
- We could add our own ALPN alias for blobs (for example `bhayanakshare/blobs/1`) by registering the same `BlobsProtocol` handler under our ALPN. Then a future incompatible iroh-blobs can sit under a new ALPN alongside the old one. **[inference: Router accepts any ALPN-to-handler mapping; not tested]**
- Use the intercept hooks so the Sender only serves the accepted Receiver's EndpointId and that Transfer's hashes. Otherwise any peer that learns a hash can pull it.

## Open questions for the map

1. **Crash-resume robustness.**
   - Do we accept #254 (a hard kill mid-fetch loses progress), work around it by splitting fetches into bounded requests (for example per child, or per N GiB range) so progress commits often, or wait for an upstream fix?
   - Do we need a guaranteed `Store::shutdown()` on every exit path?
2. **Folder metadata.**
   - Which metadata do we preserve: empty dirs, mtime, Unix mode bits, Windows attributes?
   - How do we treat symlinks: skip, follow, or recreate?
   - Where does the manifest live (the Offer, or our own metadata blob instead of `Collection`)?
3. **Name policy on the Receiver.** Rules for Windows-invalid or reserved names, case collisions, traversal, and conflicts with existing files (rename to "file (1)", overwrite, or ask).
4. **Store placement and lifetime.**
   - One global store or one per Transfer?
   - On which volume (needed for rename-based export)?
   - GC on, or delete the store when a Transfer is finished?
   - What happens if the Sender's files change before or during a Transfer (fail, or re-offer)?
5. **Sender hashing latency.** Hashing 50 GB+ or 100k files takes noticeable time before the root hash exists. Does the Offer wait for hashing, or do we send the Offer first and the content hash later?
6. **Version pinning and compatibility UX.** Exact pin, an Offer `content_protocol` version, and possibly a private ALPN for blobs. What do we show when peers are incompatible?
7. **Benchmark spike.** Measure 50 GB and 100k-file transfers on all three OSes, including crash-resume timing and Windows cross-volume export, before committing to the design.
