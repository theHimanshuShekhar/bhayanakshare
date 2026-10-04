// How the UI names the Devices it shows. Pure functions only.

import type { Contact } from "./bindings";
import { fingerprint } from "./transfers";

/** A Device ID is 52 characters of base32, in either case. */
const DEVICE_ID = /^[A-Za-z2-7]{52}$/;

export function isDeviceId(text: string): boolean {
  return DEVICE_ID.test(text);
}

/** The Contact with this Device ID, if there is one. */
export function findContact(contacts: Contact[], deviceId: string): Contact | undefined {
  return contacts.find((c) => c.id === deviceId);
}

/** The Nickname if there is one, else the Device Name; null when neither is known. */
export function contactName(contact: Contact): string | null {
  return contact.nickname ?? contact.device_name;
}

/**
 * What to call a Device in a sentence. A Contact goes by its Nickname, else its Device Name
 * (else what it announced, else its Fingerprint). Anyone else is shown as `name · Fingerprint`,
 * or the Fingerprint alone when it announced no name: a name is only what the Device says.
 */
export function peerName(deviceId: string, contacts: Contact[], announced: string | null): string {
  const print = fingerprint(deviceId);
  const contact = findContact(contacts, deviceId);
  if (contact) return contactName(contact) ?? announced ?? print;
  return announced ? `${announced} · ${print}` : print;
}

/** Contacts by what they are called, ignoring case; unnamed ones (shown by Fingerprint) last. */
export function sortedContacts(contacts: Contact[]): Contact[] {
  const key = (c: Contact) => (contactName(c) ?? "").toLocaleLowerCase();
  return [...contacts].sort((a, b) => {
    const [x, y] = [key(a), key(b)];
    if (x === "" || y === "") return x === y ? a.added_at - b.added_at : x === "" ? 1 : -1;
    return x.localeCompare(y) || a.added_at - b.added_at;
  });
}
