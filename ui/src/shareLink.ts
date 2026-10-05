// Share links: how a Device ID is handed over as a link, a QR code or a click. Pure functions
// only. A link is `bhayanakshare://add/<Device ID>?name=<Device Name>`; the QR code holds the
// same text.

import { isDeviceId } from "./contacts";

/** What a link or a pasted ID says about a Device. The name is only what the sender suggests. */
export interface Shared {
  /** The Device ID, in capitals. */
  id: string;
  /** The suggested Device Name, cleaned; null if the link has none. */
  name: string | null;
}

/** The most characters a Device Name has (the same limit the core applies). */
const MAX_NAME_CHARS = 64;

const LINK = /^bhayanakshare:\/\/add\/([A-Za-z2-7]{52})\/?(?:\?([^#\s]*))?(?:#.*)?$/i;

/** The link that hands over a Device, with its Device Name as the suggested name. */
export function shareLink(deviceId: string, deviceName: string | null): string {
  const link = `bhayanakshare://add/${deviceId}`;
  return deviceName ? `${link}?name=${encodeURIComponent(deviceName)}` : link;
}

/**
 * Reads a share link, or null if the text is not one. Only the Device ID decides that: a name
 * that cannot be read is dropped and the rest of the link still counts. Case in the scheme and
 * the host is ignored (a system may change it), as are a trailing slash, a fragment, other
 * parameters and whitespace around the link.
 */
export function parseShareLink(text: string): Shared | null {
  const match = LINK.exec(text.trim());
  if (match === null) return null;
  return { id: match[1].toUpperCase(), name: suggestedName(match[2] ?? "") };
}

/** A Device ID wherever one is accepted: the bare ID, or a share link that holds it. */
export function parseIdOrLink(text: string): Shared | null {
  const bare = text.trim();
  return isDeviceId(bare) ? { id: bare.toUpperCase(), name: null } : parseShareLink(bare);
}

/** The first `name` in a query string, cleaned the way the core cleans a Device Name. */
function suggestedName(query: string): string | null {
  for (const pair of query.split("&")) {
    const [key, value = ""] = pair.split(/=(.*)/s);
    if (key !== "name") continue;
    try {
      const cleaned = decodeURIComponent(value.replaceAll("+", " ")).replace(/\p{Cc}/gu, "");
      return [...cleaned.trim()].slice(0, MAX_NAME_CHARS).join("").trim() || null;
    } catch {
      return null; // not valid percent-encoding: no name, but still a Device
    }
  }
  return null;
}
