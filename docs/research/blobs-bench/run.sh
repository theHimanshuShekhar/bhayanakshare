#!/usr/bin/env bash
# Benchmark scenarios for iroh-blobs 0.103 fs-store. Usage: ./run.sh <scenario>...
# Scenarios: big chunked chunked-tmpfs chunked-disk1g small resume-clean resume-kill resume-chunk-kill resume-small-kill
#            kill-window chunk-trace xfs copyimport warmhash shutdown
# Generated data goes to ./data (gitignored), logs to ./results.
set -euo pipefail
cd "$(dirname "$0")"
B=$PWD/target/release/blobbench
D=$PWD/data
R=$PWD/results
SHM=/dev/shm/blobbench
mkdir -p "$D" "$R"

LOG=/dev/null
log() { echo "$@" | tee -a "$LOG"; }
run() { "$@" 2>&1 | tee -a "$LOG"; }
sys() {
  log "SYS $(date -Is) load=$(cut -d' ' -f1-3 /proc/loadavg | tr ' ' ,) io_psi=$(awk '/some/{print $2}' /proc/pressure/io) mem_avail_mib=$(awk '/MemAvailable/{print int($2/1024)}' /proc/meminfo)"
}
# allocated (du -sk) and apparent (du -sb) sizes
sizes() { for p in "$@"; do log "SIZE $p alloc_kib=$(du -sk "$p" | cut -f1) apparent_bytes=$(du -sb "$p" | cut -f1)"; done; }
store_files() { log "STOREFILES $1: $( (ls -l "$1"/blobs.db; ls -l "$1"/data | head -20) 2>/dev/null | awk 'NF>5{print $5, $NF}' | tr '\n' ';')"; }
# Evict a file or tree from the page cache (posix_fadvise DONTNEED; no root needed).
dropcache() {
  python3 - "$@" <<'EOF'
import os, sys
for root in sys.argv[1:]:
    paths = [root] if os.path.isfile(root) else [os.path.join(d, f) for d, _, fs in os.walk(root) for f in fs]
    for p in paths:
        fd = os.open(p, os.O_RDONLY)
        os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
        os.close(fd)
EOF
}

SENDER_PID=
start_sender() { # store path kind import
  rm -f "$1.ticket"
  "$B" send --store "$1" --path "$2" --kind "$3" --import "$4" --ticket-out "$1.ticket" >>"$LOG" 2>&1 &
  SENDER_PID=$!
  until [ -s "$1.ticket" ]; do kill -0 $SENDER_PID || { log "sender died"; exit 1; }; sleep 0.5; done
  grep -E 'send\.' "$LOG" | tail -6
}
stop_sender() { kill -TERM "$SENDER_PID"; wait "$SENDER_PID" || true; grep 'send.final' "$LOG" | tail -2; }

scenario_big() {
  LOG=$R/big.log; : >"$LOG"
  local S=$D/big
  [ -f "$S/src.bin" ] || "$B" gen-file --path "$S/src.bin" --size-mib 10240
  rm -rf "$S/sstore" "$S/rstore" "$S/out-copy" "$S/out-ref"
  sizes "$S/src.bin"; dropcache "$S/src.bin"; sys
  log "## sender import (TryReference, cold cache) + serve"
  start_sender "$S/sstore" "$S/src.bin" file ref
  sizes "$S/sstore"; store_files "$S/sstore"; sys
  log "## receiver fetch + export Copy"
  run "$B" recv --store "$S/rstore" --ticket "$S/sstore.ticket" --export "$S/out-copy" --export-mode copy
  sys; sizes "$S/rstore" "$S/out-copy"; store_files "$S/rstore"
  log "## verify head/tail 256 MiB"
  cmp -n 268435456 "$S/src.bin" "$S/out-copy/file.bin" && log "VERIFY head ok"
  cmp -i 10468982784 "$S/src.bin" "$S/out-copy/file.bin" && log "VERIFY tail ok"
  rm -rf "$S/out-copy"
  log "## receiver export TryReference (already complete, no fetch)"
  run "$B" recv --store "$S/rstore" --ticket "$S/sstore.ticket" --export "$S/out-ref" --export-mode ref
  sys; sizes "$S/rstore" "$S/out-ref"; store_files "$S/rstore"
  stop_sender; sizes "$S/sstore"
  rm -rf "$S/rstore" "$S/out-ref"
}

scenario_small() {
  LOG=$R/small.log; : >"$LOG"
  local S=$D/small
  [ -d "$S/src" ] || run "$B" gen-files --dir "$S/src" --count 100000 --min 1024 --max 8192
  rm -rf "$S/sstore" "$S/rstore" "$S/out"
  sizes "$S/src"; dropcache "$S/src"; sys
  log "## sender import 100k files (TryReference, cold cache) + collection"
  start_sender "$S/sstore" "$S/src" dir ref
  sizes "$S/sstore"; store_files "$S/sstore"; sys
  log "## receiver fetch + export Copy"
  run "$B" recv --store "$S/rstore" --ticket "$S/sstore.ticket" --export "$S/out" --export-mode copy
  sys; sizes "$S/rstore" "$S/out"; store_files "$S/rstore"
  log "COUNT out files=$(find "$S/out" -type f | wc -l)"
  diff -rq "$S/src" "$S/out" >/dev/null && log "VERIFY tree identical"
  stop_sender
  rm -rf "$S/rstore" "$S/out"
}

# Single-request vs chunked (ranged) fetch. chunk_fetch_reps <dir> <src> <reps> <chunk-mib|none>...
chunk_fetch_reps() {
  local S=$1 src=$2 reps=$3; shift 3
  start_sender "$S/sstore" "$src" file ref
  for rep in $(seq "$reps"); do
    for c in "$@"; do
      rm -rf "$S/rstore-chunk"; dropcache "$src"; sys
      log "## rep $rep chunk=$c"
      if [ "$c" = none ]; then
        run "$B" recv --store "$S/rstore-chunk" --ticket "$S/sstore.ticket"
      else
        run "$B" recv --store "$S/rstore-chunk" --ticket "$S/sstore.ticket" --chunk-mib "$c"
      fi
    done
  done
  sys; stop_sender
  rm -rf "$S/rstore-chunk"
}
# On disk, 10 GiB file (single-request baseline is in big.log). REPS / CHUNKS override.
scenario_chunked() {
  LOG=$R/chunked.log; : >"$LOG"
  local S=$D/big
  [ -f "$S/src.bin" ] || "$B" gen-file --path "$S/src.bin" --size-mib 10240
  chunk_fetch_reps "$S" "$S/src.bin" "${REPS:-1}" ${CHUNKS:-64 16}
}
# Same comparison with source and both stores on tmpfs (1 GiB): isolates per-request
# protocol/store overhead from disk contention.
scenario_chunked-tmpfs() {
  LOG=$R/chunked-tmpfs.log; : >"$LOG"
  local S=$SHM/ct
  rm -rf "$S"; mkdir -p "$S"
  log "FS $(findmnt -no FSTYPE -T "$S")"
  run "$B" gen-file --path "$S/src.bin" --size-mib 1024
  chunk_fetch_reps "$S" "$S/src.bin" "${REPS:-3}" ${CHUNKS:-none 64 16 4}
  rm -rf "$S"
}

# Same comparison on disk with a 1 GiB file (short enough to repeat between host load spikes).
scenario_chunked-disk1g() {
  LOG=$R/chunked-disk1g.log; : >"$LOG"
  local S=$D/cd
  rm -rf "$S"; mkdir -p "$S"
  run "$B" gen-file --path "$S/src.bin" --size-mib 1024
  chunk_fetch_reps "$S" "$S/src.bin" "${REPS:-3}" ${CHUNKS:-none 64 16}
  rm -rf "$S"
}

# Resume on the 10 GiB file: stop receiver at STOP bytes, restart, let it finish.
# CHUNK (MiB) set => both runs fetch in ranged requests of that size.
resume_big() { # mode(clean|kill) [chunk-mib]
  local mode=$1 chunk=${2:-} S=$D/big STOP=${STOP:-$((3 * 1024 * 1024 * 1024 + 40 * 1024 * 1024))}
  local copt=() name=$mode
  [ -n "$chunk" ] && { copt=(--chunk-mib "$chunk"); name=$mode-c$chunk; }
  [ -f "$S/src.bin" ] || "$B" gen-file --path "$S/src.bin" --size-mib 10240
  rm -rf "$S/rstore-$name"
  sys
  start_sender "$S/sstore" "$S/src.bin" file ref
  log "## run 1 ($name): stop at $STOP bytes ($mode) ${copt[*]}"
  run "$B" recv --store "$S/rstore-$name" --ticket "$S/sstore.ticket" --stop-after "$STOP" --stop-mode "$mode" "${copt[@]}" || log "EXIT run1 $?"
  sizes "$S/rstore-$name"; store_files "$S/rstore-$name"; sys
  log "## run 2 ($name): restart and finish"
  run "$B" recv --store "$S/rstore-$name" --ticket "$S/sstore.ticket" "${copt[@]}" || log "EXIT run2 $?"
  sizes "$S/rstore-$name"; store_files "$S/rstore-$name"; sys
  stop_sender
  rm -rf "$S/rstore-$name"
}
scenario_resume-clean() { LOG=$R/resume-clean.log; : >"$LOG"; resume_big clean; }
scenario_resume-kill() { LOG=$R/resume-kill.log; : >"$LOG"; resume_big kill; }
scenario_resume-chunk-kill() {
  LOG=$R/resume-chunk-kill.log; : >"$LOG"
  resume_big kill 64; resume_big kill 16
}

# How early must a kill -9 land to lose progress (upstream #254)? Kill at a few small
# offsets, unchunked and chunked, then reopen and read how many bytes the store reports.
scenario_kill-window() {
  LOG=$R/kill-window.log; : >"$LOG"
  local S=$D/kw
  rm -rf "$S"; mkdir -p "$S"
  run "$B" gen-file --path "$S/src.bin" --size-mib 1024
  start_sender "$S/sstore" "$S/src.bin" file ref
  for c in none 16 64; do
    for mib in 8 32 128; do
      rm -rf "$S/r"; sys
      local copt=(); [ $c != none ] && copt=(--chunk-mib $c)
      log "## kill at ${mib} MiB, chunk=$c"
      run "$B" recv --store "$S/r" --ticket "$S/sstore.ticket" --stop-after $((mib << 20)) --stop-mode kill "${copt[@]}" | grep -E "stopped" || true
      log "## reopen"
      run "$B" recv --store "$S/r" --ticket "$S/sstore.ticket" --stop-after 1 --stop-mode clean "${copt[@]}" | grep -E "local_bytes_before|local_info_secs" || true
    done
  done
  stop_sender
  rm -rf "$S"
}

# Is the partial persisted (bitfield written => fsync) between chunks? 1 GiB on disk,
# per-chunk timing from CHUNK_TRACE plus a poller logging every .bitfield rewrite.
scenario_chunk-trace() {
  LOG=$R/chunk-trace.log; : >"$LOG"
  local S=$D/ctr
  rm -rf "$S"; mkdir -p "$S"
  run "$B" gen-file --path "$S/src.bin" --size-mib 1024
  start_sender "$S/sstore" "$S/src.bin" file ref
  for c in 64 16; do
    rm -rf "$S/r"; sys; log "## chunk=$c"
    local before; before=$(grep -c BITFIELD_WRITE "$LOG" || true)
    ( last=; while :; do
        cur=$(stat -c '%y' "$S"/r/data/*.bitfield 2>/dev/null | head -1) || true
        [ -n "$cur" ] && [ "$cur" != "$last" ] && { echo "BITFIELD_WRITE at $(date +%T.%N | cut -c1-12) size=$(stat -c %s "$S"/r/data/*.bitfield | head -1)" >>"$LOG"; last=$cur; }
        sleep 0.02
      done ) & local poll=$!
    CHUNK_TRACE=1 run "$B" recv --store "$S/r" --ticket "$S/sstore.ticket" --chunk-mib $c
    kill $poll; wait $poll 2>/dev/null || true
    log "BITFIELD_WRITES chunk=$c count=$(( $(grep -c BITFIELD_WRITE "$LOG") - before ))"
  done
  stop_sender
  rm -rf "$S"
}

# kill -9 midway through the 100k-file collection, then restart.
scenario_resume-small-kill() {
  LOG=$R/resume-small-kill.log; : >"$LOG"
  local S=$D/small STOP=$((200 * 1024 * 1024))
  rm -rf "$S/rstore-kill"
  start_sender "$S/sstore" "$S/src" dir ref
  log "## run 1: kill -9 at $STOP bytes"
  run "$B" recv --store "$S/rstore-kill" --ticket "$S/sstore.ticket" --stop-after $STOP --stop-mode kill || log "EXIT run1 $?"
  sizes "$S/rstore-kill"; sys
  log "## run 2: restart and finish"
  run "$B" recv --store "$S/rstore-kill" --ticket "$S/sstore.ticket" || log "EXIT run2 $?"
  sizes "$S/rstore-kill"; sys
  stop_sender
  rm -rf "$S/rstore-kill"
}

# Export from an ext4 store into tmpfs (/dev/shm), Copy and TryReference.
scenario_xfs() {
  LOG=$R/xfs.log; : >"$LOG"
  local S=$D/xfs
  rm -rf "$S" "$SHM"; mkdir -p "$S" "$SHM"
  log "FS store=$(findmnt -no FSTYPE -T "$S") dest=$(findmnt -no FSTYPE -T "$SHM")"
  run "$B" gen-file --path "$S/src.bin" --size-mib 500
  start_sender "$S/sstore" "$S/src.bin" file ref
  log "## fetch + export Copy -> tmpfs"
  run "$B" recv --store "$S/rstore" --ticket "$S/sstore.ticket" --export "$SHM/copy" --export-mode copy || log "EXIT copy $?"
  sizes "$S/rstore" "$SHM/copy"; store_files "$S/rstore"
  cmp "$S/src.bin" "$SHM/copy/file.bin" && log "VERIFY copy ok"
  rm -rf "$SHM/copy"
  log "## export TryReference -> tmpfs (cross-device rename)"
  run "$B" recv --store "$S/rstore" --ticket "$S/sstore.ticket" --export "$SHM/ref" --export-mode ref || log "EXIT ref $?"
  sizes "$S/rstore" "$SHM/ref"; store_files "$S/rstore"
  cmp "$S/src.bin" "$SHM/ref/file.bin" && log "VERIFY ref ok"
  log "## store still serves blob after ref export? export Copy to disk"
  run "$B" recv --store "$S/rstore" --ticket "$S/sstore.ticket" --export "$S/again" --export-mode copy || log "EXIT again $?"
  cmp "$S/src.bin" "$S/again/file.bin" && log "VERIFY again ok"
  sizes "$S/rstore"; store_files "$S/rstore"
  stop_sender
  rm -rf "$S" "$SHM"
}

# ImportMode::Copy vs TryReference cost on the sender (1 GiB, cold cache).
scenario_copyimport() {
  LOG=$R/copyimport.log; : >"$LOG"
  local S=$D/ci
  rm -rf "$S"; mkdir -p "$S"
  run "$B" gen-file --path "$S/src.bin" --size-mib 1024
  for m in ref copy; do
    dropcache "$S/src.bin"; sys
    log "## import $m"
    start_sender "$S/sstore-$m" "$S/src.bin" file $m
    stop_sender; sizes "$S/sstore-$m"
  done
  rm -rf "$S"
}

# Hashing rate with the file already in page cache (CPU bound), 1 GiB.
scenario_warmhash() {
  LOG=$R/warmhash.log; : >"$LOG"
  local S=$D/wh
  rm -rf "$S"; mkdir -p "$S"
  run "$B" gen-file --path "$S/src.bin" --size-mib 1024
  cat "$S/src.bin" >/dev/null
  log "FINCORE $(fincore -nb "$S/src.bin")"; sys
  start_sender "$S/sstore" "$S/src.bin" file ref
  stop_sender
  rm -rf "$S"
}

# Why did a store shutdown take 62 s (xfs.log)? Shutdown closes redb, which fsyncs blobs.db.
# Compare shutdown after: no export; Copy export to tmpfs; Copy export to the same ext4;
# and a plain `cp` (no iroh export) of the same size onto ext4 just before a no-op recv.
scenario_shutdown() {
  LOG=$R/shutdown.log; : >"$LOG"
  local S=$D/sd
  rm -rf "$S" "$SHM"; mkdir -p "$S" "$SHM"
  run "$B" gen-file --path "$S/src.bin" --size-mib 2048
  start_sender "$S/sstore" "$S/src.bin" file ref
  run "$B" recv --store "$S/rstore" --ticket "$S/sstore.ticket"
  sync
  for rep in 1 2; do
    sync; sys; log "## rep $rep: complete store, no export"
    run "$B" recv --store "$S/rstore" --ticket "$S/sstore.ticket"
    sync; sys; log "## rep $rep: export Copy -> tmpfs"
    run "$B" recv --store "$S/rstore" --ticket "$S/sstore.ticket" --export "$SHM/c" --export-mode copy
    rm -rf "$SHM/c"
    sync; sys; log "## rep $rep: export Copy -> same ext4"
    run "$B" recv --store "$S/rstore" --ticket "$S/sstore.ticket" --export "$S/c" --export-mode copy
    local t0=$(date +%s.%N); sync; log "METRIC sync_after_export_secs $(echo "$(date +%s.%N) - $t0" | bc)"
    rm -rf "$S/c"
    sync; sys; log "## rep $rep: plain cp 2 GiB onto ext4, then no-export recv"
    cp "$S/src.bin" "$S/junk.bin"
    run "$B" recv --store "$S/rstore" --ticket "$S/sstore.ticket"
    sync; rm -f "$S/junk.bin"
  done
  stop_sender
  rm -rf "$S" "$SHM"
}

for s in "$@"; do "scenario_$s"; done
