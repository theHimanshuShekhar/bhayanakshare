import { describe, expect, it } from "vitest";
import type { Contact, DeviceEvent, NearbyDevice } from "./bindings";
import { applyNearby, nearbyStrangers } from "./nearby";

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
