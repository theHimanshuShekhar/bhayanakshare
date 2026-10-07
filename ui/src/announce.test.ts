import { describe, expect, it } from "vitest";
import { PROGRESS_MIN_GAP_MS, announcements, type ProgressMemory } from "./announce";
import type { DeviceEvent, TransferState } from "./bindings";
import { applyEvent, noTransfers, type TransferView, type Transfers } from "./transfers";

const ID = "ab".repeat(16);
const PEER = "K3QF7XNA" + "A".repeat(44);
const OTHER = "Q2WERTYU" + "C".repeat(44);
const BATCH = "ba".repeat(16);

let seq = 0;

function transfer(
  state: TransferState,
  extra: Partial<{ id: string; role: "sender" | "receiver"; peer: string; batch: string; kind: "files" | "text" }> = {},
): DeviceEvent {
  seq += 1;
  return {
    seq,
    at: 1_000 + seq,
    type: "transfer",
    transfer_id: extra.id ?? ID,
    role: extra.role ?? "receiver",
    peer: extra.peer ?? PEER,
    peer_name: null,
    kind: extra.kind ?? "files",
    text: null,
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

/** `bytes` of the 1000 have arrived, as reported at `at`. */
function progress(bytes: number, at: number, id = ID): DeviceEvent {
  seq += 1;
  return { seq, at, type: "progress", transfer_id: id, bytes, total: 1000 };
}

function preparing(on: boolean, id = ID): DeviceEvent {
  seq += 1;
  return { seq, at: 1_000 + seq, type: "preparing", transfer_id: id, preparing: on };
}

const peerOf = (view: TransferView) => (view.peer === PEER ? "Mum" : "Dad");

/** Plays events one after the other, gathering what would be said after each. */
function play(events: DeviceEvent[], start: Transfers = noTransfers, memory: ProgressMemory = {}) {
  let state = start;
  let remembered = memory;
  const said: string[][] = [];
  for (const event of events) {
    const next = applyEvent(state, event);
    const result = announcements(state, next, remembered, peerOf);
    said.push(result.messages);
    remembered = result.memory;
    state = next;
  }
  return { said, state, memory: remembered };
}

describe("announcements: what a state change says", () => {
  it("says nothing when nothing changed", () => {
    const state = applyEvent(noTransfers, transfer({ kind: "offered" }));
    expect(announcements(state, state, {}, peerOf).messages).toEqual([]);
  });

  it("says an Offer was received, with who it is from and what it is", () => {
    const { said } = play([transfer({ kind: "offered" })]);
    expect(said).toEqual([["photo.jpg from Mum: Waiting for your answer."]]);
  });

  it("says a Transfer started when this Device sent it", () => {
    const { said } = play([transfer({ kind: "offered" }, { role: "sender" })]);
    expect(said).toEqual([["photo.jpg to Mum: Waiting for Mum…"]]);
  });

  it("says each change of state once: accepted, receiving, received", () => {
    const { said } = play([
      transfer({ kind: "offered" }),
      transfer({ kind: "accepted" }),
      transfer({ kind: "accepted" }),
      transfer({ kind: "transferring" }),
      transfer({ kind: "completed", saved_to: "/x/photo.jpg" }),
    ]);
    expect(said.map((m) => m.join())).toEqual([
      "photo.jpg from Mum: Waiting for your answer.",
      "photo.jpg from Mum: Accepted. Starting…",
      "",
      "photo.jpg from Mum: Receiving…",
      "photo.jpg from Mum: Received.",
    ]);
  });

  it("says why a Transfer failed", () => {
    const { said } = play([
      transfer({ kind: "offered" }, { role: "sender" }),
      transfer({ kind: "failed", reason: "The connection dropped." }, { role: "sender" }),
    ]);
    expect(said[1]).toEqual(["photo.jpg to Mum: Could not send. The connection dropped."]);
  });

  it("says who declined, cancelled and did not answer", () => {
    const sent = (state: TransferState) =>
      play([transfer({ kind: "offered" }, { role: "sender" }), transfer(state, { role: "sender" })]).said[1];
    expect(sent({ kind: "declined" })).toEqual(["photo.jpg to Mum: Mum declined."]);
    expect(sent({ kind: "cancelled", by: "receiver" })).toEqual(["photo.jpg to Mum: Mum cancelled."]);
    expect(sent({ kind: "expired" })).toEqual(["photo.jpg to Mum: Mum did not answer in time. The Offer expired."]);
  });

  it("says Preparing, and that the Receiver lost contact", () => {
    const { said } = play([
      transfer({ kind: "offered" }, { role: "sender" }),
      preparing(true),
      preparing(false),
      transfer({ kind: "accepted" }, { role: "sender" }),
      transfer({ kind: "transferring" }, { role: "receiver", id: "cd".repeat(16) }),
      transfer({ kind: "reconnecting" }, { role: "receiver", id: "cd".repeat(16) }),
    ]);
    expect(said[1]).toEqual(["photo.jpg to Mum: Preparing the files for Mum…"]);
    expect(said[2]).toEqual(["photo.jpg to Mum: Waiting for Mum…"]);
    expect(said[5]).toEqual(["photo.jpg from Mum: Lost contact with Mum. Reconnecting…"]);
  });

  it("leaves out Saving, which is over in a moment and followed by Received", () => {
    const { said } = play([transfer({ kind: "transferring" }), transfer({ kind: "saving" })]);
    expect(said[1]).toEqual([]);
  });

  it("names a text by the word Text", () => {
    const { said } = play([transfer({ kind: "offered" }, { kind: "text" })]);
    expect(said[0]).toEqual(["Text from Mum: Waiting for your answer."]);
  });

  it("says what happened to several Transfers at once, in the order they began", () => {
    const first = applyEvent(noTransfers, transfer({ kind: "transferring" }, { id: "11".repeat(16) }));
    const both = applyEvent(first, transfer({ kind: "transferring" }, { id: "22".repeat(16), peer: OTHER }));
    const next = [
      transfer({ kind: "failed", reason: "No." }, { id: "11".repeat(16) }),
      transfer({ kind: "completed", saved_to: null }, { id: "22".repeat(16), peer: OTHER }),
    ].reduce(applyEvent, both);
    expect(announcements(both, next, {}, peerOf).messages).toEqual([
      "photo.jpg from Mum: Could not receive. No.",
      "photo.jpg from Dad: Received.",
    ]);
  });
});

describe("announcements: progress", () => {
  // Built first, so that its events come before the progress that follows.
  const start = () => play([transfer({ kind: "transferring" })]).state;

  it("stays quiet below the first step", () => {
    const from = start();
    const { said } = play([progress(0, 10_000), progress(100, 11_000), progress(240, 12_000)], from);
    expect(said).toEqual([[], [], []]);
  });

  it("says each quarter once, however many reports come inside it", () => {
    const from = start();
    const { said } = play(
      [progress(250, 20_000), progress(260, 21_000), progress(300, 22_000)],
      from,
    );
    expect(said).toEqual([["photo.jpg from Mum: 25% of 1,000 B"], [], []]);
  });

  it("waits ten seconds between two, and says the later step once they have passed", () => {
    const from = start();
    const { said } = play(
      [
        progress(260, 20_000),
        // The next quarter, too soon after: held back, not lost.
        progress(510, 20_000 + PROGRESS_MIN_GAP_MS - 1),
        progress(530, 20_000 + PROGRESS_MIN_GAP_MS),
      ],
      from,
    );
    expect(said.map((m) => m.join())).toEqual([
      "photo.jpg from Mum: 26% of 1,000 B",
      "",
      "photo.jpg from Mum: 53% of 1,000 B",
    ]);
  });

  it("says the step reached when several are skipped at once", () => {
    const from = start();
    const { said } = play([progress(100, 20_000), progress(800, 21_000)], from);
    expect(said[1]).toEqual(["photo.jpg from Mum: 80% of 1,000 B"]);
  });

  it("does not say 100%: Received says it", () => {
    const from = start();
    const { said } = play([progress(260, 20_000), progress(1000, 40_000)], from);
    expect(said[1]).toEqual([]);
  });

  it("keeps a count for each Transfer", () => {
    const from = start();
    const both = play(
      [
        transfer({ kind: "transferring" }, { id: "22".repeat(16), peer: OTHER }),
        progress(300, 20_000),
        progress(300, 20_000, "22".repeat(16)),
      ],
      from,
    );
    expect(both.said[1]).toEqual(["photo.jpg from Mum: 30% of 1,000 B"]);
    expect(both.said[2]).toEqual(["photo.jpg from Dad: 30% of 1,000 B"]);
  });

  it("does not change the memory it was given", () => {
    const memory: ProgressMemory = {};
    const before = applyEvent(noTransfers, transfer({ kind: "transferring" }));
    const after = applyEvent(before, progress(300, 20_000));
    const result = announcements(before, after, memory, peerOf);
    expect(memory).toEqual({});
    expect(result.memory[ID]).toEqual({ bucket: 1, at: 20_000 });
  });
});

describe("announcements: a Batch", () => {
  const member = (n: number, peer: string, state: TransferState) =>
    transfer(state, { id: String(n).repeat(32), role: "sender", peer, batch: BATCH });

  it("is not announced Receiver by Receiver", () => {
    const { said } = play([
      member(1, PEER, { kind: "offered" }),
      member(2, OTHER, { kind: "offered" }),
      member(1, PEER, { kind: "transferring" }),
      member(1, PEER, { kind: "completed", saved_to: null }),
    ]);
    expect(said).toEqual([[], [], [], []]);
  });

  it("says how it ended, once every Receiver is done", () => {
    const { said } = play([
      member(1, PEER, { kind: "offered" }),
      member(2, OTHER, { kind: "offered" }),
      member(1, PEER, { kind: "completed", saved_to: null }),
      member(2, OTHER, { kind: "declined" }),
    ]);
    expect(said[2]).toEqual([]);
    expect(said[3]).toEqual(["photo.jpg to 2 Devices: 1 of 2 delivered, 1 declined"]);
  });

  it("says so again when a retry ends", () => {
    const { said } = play([
      member(1, PEER, { kind: "offered" }),
      member(1, PEER, { kind: "failed", reason: "No." }),
      // The retry is a new Transfer that takes the failed one's place.
      member(3, PEER, { kind: "offered" }),
      member(3, PEER, { kind: "completed", saved_to: null }),
    ]);
    expect(said[1]).toEqual(["photo.jpg to 1 Devices: 0 of 1 delivered, 1 failed"]);
    expect(said[2]).toEqual([]);
    expect(said[3]).toEqual(["photo.jpg to 1 Devices: 1 of 1 delivered"]);
  });
});
