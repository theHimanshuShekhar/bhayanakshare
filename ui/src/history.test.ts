// Nothing here needs a document, so no jsdom: building one costs seconds of CPU on the Windows runner.
// @vitest-environment node
import { describe, expect, it } from "vitest";
import type { Contact, HistoryEntry, HistoryTransfer, TransferState } from "./bindings";
import {
  canRetry,
  deviceOptions,
  latestPerReceiver,
  noteDevices,
  oldestFirst,
  retriedIds,
} from "./history";

const BATCH = "ba".repeat(16);
const BOB = "B".repeat(52);
const CAROL = "C".repeat(52);

function transfer(n: number, over: Partial<HistoryTransfer["record"]> = {}): HistoryTransfer {
  return {
    saved_present: null,
    record: {
      id: n.toString(16).padStart(2, "0").repeat(16),
      role: "sender",
      peer: BOB,
      peer_name: null,
      name: "a.txt",
      kind: "files",
      size: 1,
      text: null,
      items: ["a.txt"],
      file_count: 1,
      skipped_links: 0,
      adjusted_names: 0,
      batch_id: BATCH,
      state: { kind: "failed", reason: "x" } as TransferState,
      created_at: n,
      accepted_at: null,
      updated_at: n,
      ...over,
    },
  };
}

const single = (t: HistoryTransfer): HistoryEntry => ({ kind: "transfer", transfer: t });

function contact(id: string, nickname: string | null): Contact {
  return {
    id,
    nickname,
    device_name: null,
    auto_accept: false,
    last_known_address: { relay_url: null, direct: [] },
    added_at: 1,
  };
}

describe("oldestFirst", () => {
  it("turns newest-first entries, with Batches oldest first inside, into one list oldest first", () => {
    const entries: HistoryEntry[] = [
      single(transfer(5, { batch_id: null })),
      { kind: "batch", batch_id: BATCH, transfers: [transfer(2), transfer(3), transfer(4)] },
      single(transfer(1, { batch_id: null })),
    ];
    expect(oldestFirst(entries).map((t) => t.record.created_at)).toEqual([1, 2, 3, 4, 5]);
  });
});

describe("Retry", () => {
  it("is for a Failed Transfer of a Batch that this Device sent and has not sent again", () => {
    const failed = transfer(1);
    const sentAgain = transfer(2, { state: { kind: "completed", saved_to: null } });
    const other = transfer(3, { peer: CAROL });
    const entries: HistoryEntry[] = [{ kind: "batch", batch_id: BATCH, transfers: [failed, sentAgain, other] }];
    const retried = retriedIds(entries);

    // Bob's first attempt was retried by the second; Carol's was never.
    expect(canRetry(failed, retried)).toBe(false);
    expect(canRetry(transfer(4, { peer: CAROL }), retriedIds([]))).toBe(true);
    // Only a Failed one: not one that ended some other way, nor a Receiver's, nor one on its own.
    expect(canRetry(transfer(5, { state: { kind: "declined" } }), retriedIds([]))).toBe(false);
    expect(canRetry(transfer(6, { role: "receiver" }), retriedIds([]))).toBe(false);
    expect(canRetry(transfer(7, { batch_id: null }), retriedIds([]))).toBe(false);
  });

  it("looks at the same Receiver of the same Batch, in a list of Transfers on their own too", () => {
    // Newest first, as a History narrowed to one Device lists a Batch.
    const entries = [single(transfer(2)), single(transfer(1))];
    expect([...retriedIds(entries)]).toEqual([transfer(1).record.id]);
    const elsewhere = [single(transfer(2, { batch_id: "cd".repeat(16) })), single(transfer(1))];
    expect(retriedIds(elsewhere).size).toBe(0);
  });
});

describe("latestPerReceiver", () => {
  it("keeps one Transfer for each Receiver, the latest, where the Receiver first came", () => {
    const first = transfer(1);
    const carol = transfer(2, { peer: CAROL });
    const again = transfer(3);
    expect(latestPerReceiver([first, carol, again])).toEqual([again, carol]);
  });
});

describe("Devices to filter by", () => {
  it("are every Contact and every Device in History, called what they go by here", () => {
    const known = noteDevices({}, [single(transfer(2, { peer: CAROL, peer_name: "Carol's phone" }))]);
    expect(known).toEqual({ [CAROL]: "Carol's phone" });
    expect(deviceOptions([contact(BOB, "Bob")], known).map((o) => o.name)).toEqual([
      "Bob",
      "Carol's phone · CCCC-CCCC",
    ]);
  });

  it("remember a Device when a narrower list no longer shows it, and take its newest name", () => {
    const seen = noteDevices({}, [single(transfer(2, { peer_name: "Bob's laptop" })), single(transfer(1, { peer_name: "Old name" }))]);
    expect(seen[BOB]).toBe("Bob's laptop");
    // A later list without that Device keeps it; one where it has no name keeps the name.
    expect(noteDevices(seen, [single(transfer(3, { peer: CAROL }))])[BOB]).toBe("Bob's laptop");
    expect(noteDevices(seen, [single(transfer(4, { peer_name: null }))])[BOB]).toBe("Bob's laptop");
  });
});
