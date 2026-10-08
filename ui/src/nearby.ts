// What the UI knows about Nearby Devices, built up from the Device's event stream. Pure functions
// only: the React side just feeds events in and renders the result.

import type { Contact, DeviceEvent, DiscoveryStatus, NearbyDevice } from "./bindings";
import { findContact } from "./contacts";
import type { MessageKey } from "./i18n";

/** How long Home waits for a Nearby Device before it suggests the firewall may be in the way. */
export const NEARBY_WAIT_MS = 30_000;

/** The page that explains how to allow local discovery through a firewall. */
export const FIREWALL_DOCS_URL =
  "https://github.com/theHimanshuShekhar/bhayanakshare/blob/main/docs/firewall.md";

/** Its section on what to do when local discovery could not start at all. */
export const DISCOVERY_DOCS_URL = `${FIREWALL_DOCS_URL}#when-home-says-local-discovery-could-not-start`;

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

/** The status the UI should hold once `next` is known: `current` itself if it is the same, so
 * that nothing renders again for it. */
export function settleDiscovery(current: DiscoveryStatus, next: DiscoveryStatus): DiscoveryStatus {
  const same =
    current.state === next.state &&
    (current.state === "working" || (next.state === "unavailable" && current.reason === next.reason));
  return same ? current : next;
}

/** Applies one event: a `discovery_status` event holds the status as it is then. */
export function applyDiscovery(status: DiscoveryStatus, event: DeviceEvent): DiscoveryStatus {
  return event.type === "discovery_status" ? settleDiscovery(status, event.status) : status;
}

/**
 * What to tell the user when local discovery could not start: the words (i18n keys) for what
 * happened and for why, or `null` while it works. A Hidden Device looks for nobody, so what it
 * loses is that Devices holding its ID cannot reach it.
 */
export function discoveryHint(
  status: DiscoveryStatus,
  hidden: boolean,
): { summary: MessageKey; reason: MessageKey } | null {
  if (status.state === "working") return null;
  const reasons = {
    port_in_use: "home.discoveryDownPort",
    no_interface: "home.discoveryDownInterface",
    other: "home.discoveryDownOther",
  } as const;
  return {
    summary: hidden ? "home.discoveryDownHidden" : "home.discoveryDown",
    reason: reasons[status.reason],
  };
}
