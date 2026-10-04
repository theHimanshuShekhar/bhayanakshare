import { describe, expect, it } from "vitest";
import type { DeviceEvent, TransferState } from "./bindings";
import {
  applyEvent,
  canCancel,
  canResend,
  fingerprint,
  formatCountdown,
  formatSize,
  newestFirst,
  noTransfers,
  pendingOffer,
  percent,
  type Transfers,
} from "./transfers";

const ID = "ab".repeat(16);
const PEER = "K3QF7XNA" + "A".repeat(44);

function transfer(seq: number, state: TransferState, extra: Partial<{ id: string; role: "sender" | "receiver" }> = {}): DeviceEvent {
  return {
    seq,
    at: 1_000 + seq,
    type: "transfer",
    transfer_id: extra.id ?? ID,
    role: extra.role ?? "receiver",
    peer: PEER,
    peer_name: null,
    name: "photo.jpg",
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
    const base = { id: ID, role: "receiver", peer: PEER, peerName: null, name: "x", state: { kind: "transferring" }, expiresAt: 0, rate: null, progressAt: null } as const;
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
