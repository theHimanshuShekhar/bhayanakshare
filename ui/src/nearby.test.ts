// Nothing here needs a document, so no jsdom: building one costs seconds of CPU on the Windows runner.
// @vitest-environment node
import { describe, expect, it } from "vitest";
import type { Contact, DeviceEvent, DiscoveryStatus, NearbyDevice } from "./bindings";
import { applyDiscovery, applyNearby, discoveryHint, nearbyStrangers, settleDiscovery } from "./nearby";

const A = "A".repeat(52);
const B = "B".repeat(52);

const nearby = (...devices: NearbyDevice[]): DeviceEvent => ({
  seq: 0,
  at: 0,
  type: "nearby",
  devices,
});

const contact = (id: string): Contact => ({
  id,
  nickname: null,
  device_name: null,
  auto_accept: false,
  last_known_address: { relay_url: null, direct: [] },
  added_at: 1,
});

describe("applyNearby", () => {
  it("takes the list from each nearby event, whole", () => {
    const first = applyNearby([], nearby({ id: A, name: "Alice" }, { id: B, name: null }));
    expect(first.map((d) => d.id)).toEqual([A, B]);
    expect(applyNearby(first, nearby({ id: B, name: "Bob" }))).toEqual([{ id: B, name: "Bob" }]);
    expect(applyNearby(first, nearby())).toEqual([]);
  });

  it("ignores events that are not about Nearby Devices", () => {
    const list = [{ id: A, name: "Alice" }];
    const progress: DeviceEvent = {
      seq: 1,
      at: 0,
      type: "progress",
      transfer_id: "ab".repeat(16),
      bytes: 1,
      total: 2,
    };
    expect(applyNearby(list, progress)).toBe(list);
  });
});

describe("nearbyStrangers", () => {
  it("leaves out Contacts, matched by Device ID, and keeps the order", () => {
    const list = [
      { id: A, name: "Alice" },
      { id: B, name: "Bob" },
    ];
    expect(nearbyStrangers(list, [contact(A)])).toEqual([{ id: B, name: "Bob" }]);
    expect(nearbyStrangers(list, [])).toEqual(list);
    expect(nearbyStrangers(list, [contact(A), contact(B)])).toEqual([]);
  });
});

const discovery = (status: DiscoveryStatus): DeviceEvent => ({
  seq: 0,
  at: 0,
  type: "discovery_status",
  status,
});

const WORKING: DiscoveryStatus = { state: "working" };
const NO_PORT: DiscoveryStatus = { state: "unavailable", reason: "port_in_use" };

describe("applyDiscovery", () => {
  it("takes the status from each discovery_status event", () => {
    expect(applyDiscovery(WORKING, discovery(NO_PORT))).toEqual(NO_PORT);
    expect(applyDiscovery(NO_PORT, discovery(WORKING))).toEqual(WORKING);
  });

  it("ignores events that are not about discovery", () => {
    expect(applyDiscovery(NO_PORT, nearby())).toBe(NO_PORT);
  });

  it("keeps the status it has when the event says the same, so nothing renders again", () => {
    expect(applyDiscovery(NO_PORT, discovery({ ...NO_PORT }))).toBe(NO_PORT);
    expect(applyDiscovery(WORKING, discovery({ state: "working" }))).toBe(WORKING);
    const other: DiscoveryStatus = { state: "unavailable", reason: "other" };
    expect(settleDiscovery(NO_PORT, other)).toBe(other);
    expect(settleDiscovery(WORKING, NO_PORT)).toBe(NO_PORT);
  });
});

describe("discoveryHint", () => {
  it("says nothing while discovery works", () => {
    expect(discoveryHint(WORKING, false)).toBeNull();
    expect(discoveryHint(WORKING, true)).toBeNull();
  });

  it("gives each reason its own words", () => {
    const reasons = (["port_in_use", "no_interface", "other"] as const).map(
      (reason) => discoveryHint({ state: "unavailable", reason }, false)?.reason,
    );
    expect(new Set(reasons).size).toBe(3);
    expect(reasons).not.toContain(undefined);
  });

  it("says that ID holders cannot reach a Hidden Device, which a Device that looks for others does not", () => {
    expect(discoveryHint(NO_PORT, true)?.summary).not.toBe(discoveryHint(NO_PORT, false)?.summary);
  });
});
