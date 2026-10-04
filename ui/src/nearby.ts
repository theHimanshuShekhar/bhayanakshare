// What the UI knows about Nearby Devices, built up from the Device's event stream. Pure functions
// only: the React side just feeds events in and renders the result.

import type { Contact, DeviceEvent, NearbyDevice } from "./bindings";
import { findContact } from "./contacts";

/** How long Home waits for a Nearby Device before it suggests the firewall may be in the way. */
export const NEARBY_WAIT_MS = 30_000;

/** The page that explains how to allow local discovery through a firewall. */
export const FIREWALL_DOCS_URL =
  "https://github.com/theHimanshuShekhar/bhayanakshare/blob/main/docs/firewall.md";

/**
 * Applies one event. Every `nearby` event holds the whole list as it is then, so it replaces
 * the last one; any other event leaves the list alone.
 */
export function applyNearby(devices: NearbyDevice[], event: DeviceEvent): NearbyDevice[] {
  return event.type === "nearby" ? event.devices : devices;
}

/** The Nearby Devices that are not Contacts: the ones shown by what they announce. */
export function nearbyStrangers(devices: NearbyDevice[], contacts: Contact[]): NearbyDevice[] {
  return devices.filter((d) => findContact(contacts, d.id) === undefined);
}
