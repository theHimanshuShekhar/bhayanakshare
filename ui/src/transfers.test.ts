import { describe, expect, it } from "vitest";
import type { DeviceEvent, TransferState } from "./bindings";
import {
  adjustedNamesText,
  applyEvent,
  batchStatus,
  canCancel,
  canResend,
  canRetry,
  fingerprint,
  formatCountdown,
  formatSize,
  listItems,
  newestFirst,
  noTransfers,
  pendingOffer,
  percent,
  transferName,
  type Transfers,
} from "./transfers";

const ID = "ab".repeat(16);
const PEER = "K3QF7XNA" + "A".repeat(44);

function transfer(
  seq: number,
  state: TransferState,
  extra: Partial<{ id: string; role: "sender" | "receiver"; peer: string; batch: string }> = {},
): DeviceEvent {
  return {
    seq,
    at: 1_000 + seq,
    type: "transfer",
    transfer_id: extra.id ?? ID,
    role: extra.role ?? "receiver",
    peer: extra.peer ?? PEER,
    peer_name: null,
    name: "photo.jpg",
    items: ["photo.jpg"],
    file_count: 1,
    skipped_links: 0,
    adjusted_names: 0,
    batch_id: extra.batch ?? null,
    size: 1000,
    expires_at: 601_000,
    state,
  };
}

function progress(seq: number, at: number, bytes: number, id = ID): DeviceEvent {
  return { seq, at, type: "progress", transfer_id: id, bytes, total: 1000 };
}

const run = (events: DeviceEvent[]): Transfers => events.reduce(applyEvent, noTransfers);

describe("applyEvent", () => {
  it("creates a Transfer from its first event and follows its states", () => {
    const state = run([transfer(0, { kind: "offered" }), transfer(1, { kind: "accepted" })]);
    expect(state.order).toEqual([ID]);
    expect(state.byId[ID]).toMatchObject({ role: "receiver", peer: PEER, name: "photo.jpg", size: 1000 });
    expect(state.byId[ID].state).toEqual({ kind: "accepted" });
  });

  it("keeps what an Offer holds: the items, the file count and the links skipped", () => {
    const event = {
      ...transfer(0, { kind: "offered" }),
      name: "album",
      items: ["album", "notes.txt"],
      file_count: 7,
      skipped_links: 2,
    } as DeviceEvent;
    const view = run([event, transfer(1, { kind: "accepted" })]).byId[ID];
    expect(view).toMatchObject({ items: ["album", "notes.txt"], fileCount: 7, skippedLinks: 2 });
    expect(transferName(view)).toBe("album and 1 more");
    expect(transferName({ ...view, items: ["album"] })).toBe("album");
  });

  it("keeps how many names the Receiver adjusted", () => {
    const event = { ...transfer(0, { kind: "offered" }), adjusted_names: 4 } as DeviceEvent;
    const view = run([event, transfer(1, { kind: "accepted" })]).byId[ID];
    expect(view.adjustedNames).toBe(4);
    expect(run([transfer(0, { kind: "offered" })]).byId[ID].adjustedNames).toBe(0);
  });

  it("says how many names were adjusted", () => {
    expect(adjustedNamesText(1)).toBe("1 name adjusted");
    expect(adjustedNamesText(2)).toBe("2 names adjusted");
    expect(adjustedNamesText(1500)).toBe("1500 names adjusted");
  });

  it("ignores an event it has already seen", () => {
    const once = run([transfer(0, { kind: "offered" }), transfer(1, { kind: "accepted" })]);
    expect(applyEvent(once, transfer(1, { kind: "declined" }))).toBe(once);
    expect(applyEvent(once, transfer(0, { kind: "declined" }))).toBe(once);
  });

  it("works out a smoothed rate from the time between progress reports", () => {
    const state = run([
      transfer(0, { kind: "transferring" }),
      progress(1, 2_000, 0),
      progress(2, 3_000, 100), // 100 B/s
    ]);
    expect(state.byId[ID].bytes).toBe(100);
    expect(state.byId[ID].rate).toBe(100);

    const faster = applyEvent(state, progress(3, 4_000, 400)); // sample 300 B/s
    expect(faster.byId[ID].rate).toBeCloseTo(100 + 0.3 * (300 - 100));
  });

  it("has no rate until two reports have arrived", () => {
    const state = run([transfer(0, { kind: "transferring" }), progress(1, 2_000, 10)]);
    expect(state.byId[ID].rate).toBeNull();
    expect(state.byId[ID].bytes).toBe(10);
  });

  it("does not divide by zero when two reports share a timestamp", () => {
    const state = run([transfer(0, { kind: "transferring" }), progress(1, 2_000, 10), progress(2, 2_000, 20)]);
    expect(state.byId[ID].bytes).toBe(20);
    expect(state.byId[ID].rate).toBeNull();
  });

  it("ignores progress for a Transfer it has not heard of", () => {
    const state = run([progress(0, 2_000, 10)]);
    expect(state.order).toEqual([]);
    expect(state.lastSeq).toBe(0);
  });

  it("shows a completed Transfer as fully received and no longer moving", () => {
    const state = run([
      transfer(0, { kind: "transferring" }),
      progress(1, 2_000, 0),
      progress(2, 3_000, 500),
      transfer(3, { kind: "completed", saved_to: "/home/a/photo.jpg" }),
    ]);
    expect(state.byId[ID].bytes).toBe(1000);
    expect(state.byId[ID].rate).toBeNull();
  });

  it("keeps Transfers apart", () => {
    const other = "cd".repeat(16);
    const state = run([
      transfer(0, { kind: "offered" }),
      transfer(1, { kind: "offered" }, { id: other, role: "sender" }),
      transfer(2, { kind: "declined" }),
    ]);
    expect(newestFirst(state).map((x) => x.id)).toEqual([other, ID]);
    expect(state.byId[other].state).toEqual({ kind: "offered" });
  });
});

describe("pendingOffer", () => {
  it("is the oldest incoming Offer not yet answered", () => {
    const second = "cd".repeat(16);
    const state = run([
      transfer(0, { kind: "offered" }, { role: "sender" }),
      transfer(1, { kind: "offered" }, { id: second }),
    ]);
    expect(pendingOffer(state)?.id).toBe(second);
    expect(pendingOffer(applyEvent(state, transfer(2, { kind: "accepted" }, { id: second })))).toBeUndefined();
  });
});

describe("formatting", () => {
  it("shows the first 8 characters of a Device ID as XXXX-XXXX", () => {
    expect(fingerprint(PEER)).toBe("K3QF-7XNA");
  });

  it("formats sizes with binary units", () => {
    expect(formatSize(0)).toBe("0 B");
    expect(formatSize(1023)).toBe("1,023 B");
    expect(formatSize(1536)).toBe("1.5 KiB");
    expect(formatSize(3 * 1024 * 1024)).toBe("3 MiB");
    expect(formatSize(10 * 1024 ** 3)).toBe("10 GiB");
  });

  it("counts whole percent, and an empty file as complete", () => {
    const base = { id: ID, role: "receiver", batch: null, peer: PEER, peerName: null, name: "x", items: ["x"] as string[], fileCount: 1, skippedLinks: 0, adjustedNames: 0, state: { kind: "transferring" }, expiresAt: 0, rate: null, progressAt: null } as const;
    expect(percent({ ...base, size: 1000, bytes: 999 })).toBe(99);
    expect(percent({ ...base, size: 1000, bytes: 1000 })).toBe(100);
    expect(percent({ ...base, size: 0, bytes: 0 })).toBe(100);
  });
});

describe("expiry and cancelling", () => {
  it("remembers when an Offer lapses", () => {
    expect(run([transfer(0, { kind: "offered" })]).byId[ID].expiresAt).toBe(601_000);
  });

  it("shows the time left as minutes and seconds, rounded up, never negative", () => {
    expect(formatCountdown(600_000)).toBe("10:00");
    expect(formatCountdown(581_000)).toBe("9:41");
    expect(formatCountdown(59_001)).toBe("1:00");
    expect(formatCountdown(1)).toBe("0:01");
    expect(formatCountdown(0)).toBe("0:00");
    expect(formatCountdown(-5_000)).toBe("0:00");
  });

  const view = (role: "sender" | "receiver", state: TransferState) =>
    run([transfer(0, state, { role })]).byId[ID];

  it("lets the Sender cancel until the Transfer ends, and the Receiver once it has accepted", () => {
    expect(canCancel(view("sender", { kind: "offered" }))).toBe(true);
    expect(canCancel(view("sender", { kind: "accepted" }))).toBe(true);
    expect(canCancel(view("sender", { kind: "transferring" }))).toBe(true);
    expect(canCancel(view("receiver", { kind: "offered" }))).toBe(false); // Decline is on the sheet
    expect(canCancel(view("receiver", { kind: "accepted" }))).toBe(true);
    expect(canCancel(view("receiver", { kind: "transferring" }))).toBe(true);
    expect(canCancel(view("receiver", { kind: "reconnecting" }))).toBe(true);
    expect(canCancel(view("receiver", { kind: "saving" }))).toBe(false);
    for (const state of [
      { kind: "completed", saved_to: null },
      { kind: "failed", reason: "x" },
      { kind: "declined" },
      { kind: "expired" },
      { kind: "cancelled", by: "sender" },
    ] as const) {
      expect(canCancel(view("sender", state))).toBe(false);
    }
  });

  it("offers to send again only an expired Offer on the Sender", () => {
    expect(canResend(view("sender", { kind: "expired" }))).toBe(true);
    expect(canResend(view("receiver", { kind: "expired" }))).toBe(false);
    expect(canResend(view("sender", { kind: "declined" }))).toBe(false);
    expect(canResend(view("sender", { kind: "failed", reason: "x" }))).toBe(false);
  });
});

describe("Batches", () => {
  const BATCH = "ba".repeat(16);
  const peers = ["A", "B", "C"].map((c) => c.repeat(52));
  const [a, b, c] = peers;
  const id = (n: number) => String(n).padStart(2, "0").repeat(16);
  const delivered: TransferState = { kind: "completed", saved_to: null };
  const failed: TransferState = { kind: "failed", reason: "x" };
  /** One Transfer per Receiver in a Batch, each in the given state. */
  const batch = (...states: TransferState[]) =>
    run(
      states.map((state, i) =>
        transfer(i, state, { id: id(i + 1), role: "sender", peer: peers[i], batch: BATCH }),
      ),
    );
  const only = (state: Transfers) => {
    const items = listItems(state);
    expect(items).toHaveLength(1);
    const [item] = items;
    if (item.kind !== "batch") throw new Error("expected a Batch row");
    return item.batch;
  };

  it("shows a Batch's Transfers as one row, with each Receiver in it", () => {
    const row = only(batch({ kind: "offered" }, { kind: "accepted" }, { kind: "waiting" }));
    expect(row.id).toBe(BATCH);
    expect(row.members.map((m) => m.peer)).toEqual([a, b, c]);
    expect(row.members.map((m) => m.state.kind)).toEqual(["offered", "accepted", "waiting"]);
  });

  it("lists a Batch where it began among the other Transfers, newest first", () => {
    const state = run([
      transfer(0, { kind: "offered" }, { id: id(1), role: "sender" }),
      transfer(1, { kind: "offered" }, { id: id(2), role: "sender", peer: a, batch: BATCH }),
      transfer(2, { kind: "offered" }, { id: id(3), role: "sender", peer: b, batch: BATCH }),
      transfer(3, { kind: "offered" }, { id: id(4), role: "sender" }),
    ]);
    expect(listItems(state).map((i) => (i.kind === "batch" ? "batch" : i.transfer.id))).toEqual([
      id(4),
      "batch",
      id(1),
    ]);
  });

  it("never groups a Receiver's Transfers, which have no Batch", () => {
    const state = run([transfer(0, { kind: "offered" }), transfer(1, { kind: "offered" }, { id: id(2) })]);
    expect(listItems(state).map((i) => i.kind)).toEqual(["transfer", "transfer"]);
  });

  it("lets a retry stand in for the Failed Transfer it retries, in the same place", () => {
    const retry = transfer(3, { kind: "offered" }, { id: id(9), role: "sender", peer: b, batch: BATCH });
    const row = only(applyEvent(batch(delivered, failed, { kind: "declined" }), retry));
    expect(row.members.map((m) => [m.peer, m.id])).toEqual([
      [a, id(1)],
      [b, id(9)],
      [c, id(3)],
    ]);
  });

  it("says how many arrived and what became of the rest", () => {
    expect(batchStatus(only(batch(delivered, delivered, { kind: "declined" })))).toBe(
      "2 of 3 delivered, 1 declined",
    );
    expect(batchStatus(only(batch(delivered, { kind: "transferring" }, { kind: "waiting" })))).toBe(
      "1 of 3 delivered, 2 in progress",
    );
    expect(batchStatus(only(batch(failed, { kind: "cancelled", by: "sender" }, { kind: "expired" })))).toBe(
      "0 of 3 delivered, 1 failed, 1 cancelled, 1 expired",
    );
  });

  it("lets a Failed Transfer in a Batch be retried, and nothing else", () => {
    const inBatch = (state: TransferState) =>
      run([transfer(0, state, { role: "sender", batch: BATCH })]).byId[ID];
    expect(canRetry(inBatch(failed))).toBe(true);
    expect(canRetry(inBatch({ kind: "declined" }))).toBe(false);
    expect(canRetry(inBatch({ kind: "expired" }))).toBe(false);
    expect(canRetry(inBatch({ kind: "transferring" }))).toBe(false);
    // Not one sent on its own, and not a Receiver's.
    expect(canRetry(run([transfer(0, failed, { role: "sender" })]).byId[ID])).toBe(false);
    expect(canRetry(run([transfer(0, failed)]).byId[ID])).toBe(false);
  });

  it("lets the Sender cancel a Transfer that is waiting for a slot", () => {
    expect(canCancel(run([transfer(0, { kind: "waiting" }, { role: "sender" })]).byId[ID])).toBe(true);
  });
});
