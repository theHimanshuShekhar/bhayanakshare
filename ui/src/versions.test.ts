import { describe, expect, it } from "vitest";
import type { DeviceEvent, Outdated } from "./bindings";
import { applyVersionNotices } from "./versions";

const A = "A".repeat(52);
const B = "B".repeat(52);

const mismatch = (peer: string, outdated: Outdated, peer_name: string | null = null): DeviceEvent => ({
  seq: 0,
  at: 0,
  type: "version_mismatch",
  peer,
  peer_name,
  peer_app_version: "0.2.0",
  outdated,
});

describe("applyVersionNotices", () => {
  it("adds a notice for a refused Device, and keeps one per Device with the newest cause", () => {
    const first = applyVersionNotices([], mismatch(A, "peer", "Alice"));
    expect(first).toEqual([{ peer: A, peerName: "Alice", outdated: "peer" }]);

    const two = applyVersionNotices(first, mismatch(B, "this_device"));
    expect(two.map((n) => n.peer)).toEqual([A, B]);

    // Alice updated past this Device: now this Device is the one that must update.
    const again = applyVersionNotices(two, mismatch(A, "this_device", "Alice"));
    expect(again.map((n) => [n.peer, n.outdated])).toEqual([
      [B, "this_device"],
      [A, "this_device"],
    ]);
  });

  it("removes a notice that was dismissed", () => {
    const notices = applyVersionNotices([], mismatch(A, "peer"));
    expect(applyVersionNotices(notices, { type: "dismiss_version_notice", peer: A })).toEqual([]);
  });

  it("ignores events that are not about versions", () => {
    const notices = applyVersionNotices([], mismatch(A, "peer"));
    const nearby: DeviceEvent = { seq: 1, at: 0, type: "nearby", devices: [] };
    expect(applyVersionNotices(notices, nearby)).toBe(notices);
  });
});
