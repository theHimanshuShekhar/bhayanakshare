// What the UI knows about Devices it was refused for their version, built up from the Device's
// event stream. Pure functions only.

import type { DeviceEvent, Outdated } from "./bindings";

/**
 * Where "Update now" leads until the updater exists: the page with the latest installers.
 * The updater (#44) takes over the button.
 */
export const RELEASES_URL = "https://github.com/theHimanshuShekhar/bhayanakshare/releases";

/** A Device that could not be talked to, and who has to update. */
export interface VersionNotice {
  peer: string;
  peerName: string | null;
  outdated: Outdated;
}

export type VersionAction = DeviceEvent | { type: "dismiss_version_notice"; peer: string };

/**
 * Applies one event: a refusal adds a notice, or replaces the one for the same Device (it
 * says who has to update now), and dismissing removes it. Any other event leaves the list alone.
 */
export function applyVersionNotices(notices: VersionNotice[], action: VersionAction): VersionNotice[] {
  if (action.type === "dismiss_version_notice") return notices.filter((n) => n.peer !== action.peer);
  if (action.type !== "version_mismatch") return notices;
  const notice = { peer: action.peer, peerName: action.peer_name, outdated: action.outdated };
  return [...notices.filter((n) => n.peer !== action.peer), notice];
}
