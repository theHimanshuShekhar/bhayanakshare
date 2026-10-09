// Nothing here needs a document, so no jsdom: building one costs seconds of CPU on the Windows runner.
// @vitest-environment node
import { describe, expect, it } from "vitest";
import type { Contact } from "./bindings";
import { contactName, isDeviceId, peerName, sortedContacts } from "./contacts";

const ID = "K3QF7XNA" + "B".repeat(44);

function contact(over: Partial<Contact> = {}): Contact {
  return {
    id: ID,
    nickname: null,
    device_name: null,
    auto_accept: false,
    last_known_address: { relay_url: null, direct: [] },
    added_at: 1,
    ...over,
  };
}

describe("contactName", () => {
  it("prefers the Nickname over the Device Name", () => {
    expect(contactName(contact({ nickname: "Mum", device_name: "DESKTOP-1" }))).toBe("Mum");
    expect(contactName(contact({ device_name: "DESKTOP-1" }))).toBe("DESKTOP-1");
    expect(contactName(contact())).toBeNull();
  });
});

describe("peerName", () => {
  it("uses the Contact's name, and the Fingerprint for anyone else", () => {
    const contacts = [contact({ nickname: "Mum" })];
    expect(peerName(ID, contacts, null)).toBe("Mum");
    expect(peerName("Z".repeat(52), contacts, null)).toBe("ZZZZ-ZZZZ");
  });

  it("uses the Fingerprint for a Contact with no name yet", () => {
    expect(peerName(ID, [contact()], null)).toBe("K3QF-7XNA");
  });

  it("shows a stranger as name · Fingerprint, whatever name it announces", () => {
    expect(peerName(ID, [], "Alice's desktop")).toBe("Alice's desktop · K3QF-7XNA");
  });

  it("goes by the Nickname, then the Device Name, before the announced name", () => {
    expect(peerName(ID, [contact({ nickname: "Mum", device_name: "D" })], "Other")).toBe("Mum");
    expect(peerName(ID, [contact({ device_name: "D" })], "Other")).toBe("D");
    expect(peerName(ID, [contact()], "Other")).toBe("Other");
  });
});

describe("isDeviceId", () => {
  it("accepts 52 base32 characters in either case and nothing else", () => {
    expect(isDeviceId(ID)).toBe(true);
    expect(isDeviceId(ID.toLowerCase())).toBe(true);
    expect(isDeviceId(ID.slice(1))).toBe(false);
    expect(isDeviceId("1" + ID.slice(1))).toBe(false);
    expect(isDeviceId(` ${ID}`)).toBe(false);
  });
});

describe("sortedContacts", () => {
  it("sorts by name ignoring case, with unnamed Contacts last", () => {
    const sorted = sortedContacts([
      contact({ id: "1", added_at: 1 }),
      contact({ id: "2", nickname: "bob", added_at: 2 }),
      contact({ id: "3", device_name: "Alice", added_at: 3 }),
    ]);
    expect(sorted.map((c) => c.id)).toEqual(["3", "2", "1"]);
  });
});
