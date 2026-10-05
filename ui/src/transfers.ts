// What the UI knows about Transfers, built up from the Device's event stream. Pure functions
// only: the React side just feeds events in and renders the result.

import type { DeviceEvent, Role, TransferId, TransferState } from "./bindings";
import { t } from "./i18n";

export interface TransferView {
  id: TransferId;
  role: Role;
  /** The other Device's ID. */
  peer: string;
  /** What the other Device calls itself, as it announced; null until known. Untrusted text. */
  peerName: string | null;
  /** The first of `items`. */
  name: string;
  /** The names at the top of what is being sent: the files and folders picked, each once. */
  items: string[];
  /** How many files there are in all, inside folders too. */
  fileCount: number;
  /** Symlinks the Sender found in the folders it picked and left out. */
  skippedLinks: number;
  /** Names the Receiver changed to make them safe to write; always 0 for a Sender. */
  adjustedNames: number;
  size: number;
  state: TransferState;
  /** When an unanswered Offer lapses, by the Device's clock (Unix milliseconds). */
  expiresAt: number;
  /** Bytes of the file received so far. */
  bytes: number;
  /** Smoothed transfer rate in bytes per second; null until two reports have arrived. */
  rate: number | null;
  /** When the last progress report was emitted (event time, Unix milliseconds). */
  progressAt: number | null;
}

export interface Transfers {
  /** Sequence number of the last event applied; -1 before the first. */
  lastSeq: number;
  /** Transfer IDs, oldest first. */
  order: TransferId[];
  byId: Record<TransferId, TransferView>;
}

export const noTransfers: Transfers = { lastSeq: -1, order: [], byId: {} };

/** How much of each new rate sample is mixed into the displayed rate. */
const RATE_WEIGHT = 0.3;

/** Applies one event. An event at or before the last sequence number is a duplicate. */
export function applyEvent(transfers: Transfers, event: DeviceEvent): Transfers {
  if (event.seq <= transfers.lastSeq) return transfers;
  const next = { ...transfers, lastSeq: event.seq };

  if (event.type === "transfer") {
    const known = transfers.byId[event.transfer_id];
    const view: TransferView = known
      ? { ...known, state: event.state, peerName: event.peer_name ?? known.peerName }
      : {
          id: event.transfer_id,
          role: event.role,
          peer: event.peer,
          peerName: event.peer_name,
          name: event.name,
          items: event.items,
          fileCount: event.file_count,
          skippedLinks: event.skipped_links,
          adjustedNames: event.adjusted_names,
          size: event.size,
          state: event.state,
          expiresAt: event.expires_at,
          bytes: 0,
          rate: null,
          progressAt: null,
        };
    next.byId = { ...transfers.byId, [view.id]: event.state.kind === "completed" ? done(view) : view };
    next.order = known ? transfers.order : [...transfers.order, view.id];
    return next;
  }

  if (event.type !== "progress") return next;
  const known = transfers.byId[event.transfer_id];
  if (!known) return next;
  const elapsed = known.progressAt === null ? 0 : event.at - known.progressAt;
  let rate = known.rate;
  if (elapsed > 0) {
    const sample = ((event.bytes - known.bytes) * 1000) / elapsed;
    rate = rate === null ? sample : rate + RATE_WEIGHT * (sample - rate);
  }
  next.byId = {
    ...transfers.byId,
    [known.id]: { ...known, bytes: event.bytes, rate, progressAt: event.at },
  };
  return next;
}

/** A finished Transfer has everything and is no longer moving. */
function done(view: TransferView): TransferView {
  return { ...view, bytes: view.size, rate: null };
}

/** The Transfers in the order they began, newest first. */
export function newestFirst(transfers: Transfers): TransferView[] {
  return transfers.order.map((id) => transfers.byId[id]).reverse();
}

/**
 * The incoming Offer to answer next: `preferred` (the one a notification was clicked for) if it
 * is still waiting, else the oldest.
 */
export function pendingOffer(transfers: Transfers, preferred?: TransferId): TransferView | undefined {
  const waiting = transfers.order
    .map((id) => transfers.byId[id])
    .filter((x) => x.role === "receiver" && x.state.kind === "offered");
  return waiting.find((x) => x.id === preferred) ?? waiting[0];
}

/**
 * Whether the user can still stop this Transfer from its row. Not once it is saving, which
 * cannot be taken back, and not a Receiver's unanswered Offer, which the Offer sheet's
 * Decline covers.
 */
export function canCancel(view: TransferView): boolean {
  switch (view.state.kind) {
    case "offered":
      return view.role === "sender";
    case "accepted":
    case "transferring":
    case "reconnecting":
      return true;
    default:
      return false;
  }
}

/** A Sender can send an Offer nobody answered again in one step. */
export function canResend(view: TransferView): boolean {
  return view.role === "sender" && view.state.kind === "expired";
}

/** 581_000 ms becomes "9:41": the time left, rounded up to whole seconds, never below 0:00. */
export function formatCountdown(millis: number): string {
  const total = Math.max(0, Math.ceil(millis / 1000));
  const seconds = String(total % 60).padStart(2, "0");
  return `${Math.floor(total / 60)}:${seconds}`;
}

/** Whole percent received, 0 to 100. An empty file is complete as soon as it is known. */
export function percent(view: TransferView): number {
  if (view.size === 0) return 100;
  return Math.min(100, Math.floor((view.bytes * 100) / view.size));
}

/** What a Transfer holds, for a sentence: "photo.jpg", or "photo.jpg and 2 more". */
export function transferName(view: TransferView): string {
  return view.items.length > 1
    ? t("transfer.nameMore", { name: view.name, count: view.items.length - 1 })
    : view.name;
}

/** The last part of a path, for showing a file by name. */
export function baseName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

/** `XXXX-XXXX`: the first 8 characters of a Device ID, for checking by eye. */
export function fingerprint(deviceId: string): string {
  return `${deviceId.slice(0, 4)}-${deviceId.slice(4, 8)}`;
}

const UNITS = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];

/** "1 name adjusted" or "7 names adjusted": names the Receiver changed to be able to save them. */
export function adjustedNamesText(count: number): string {
  return count === 1 ? t("names.adjustedOne") : t("names.adjusted", { count });
}

/** 1536 becomes "1.5 KiB". */
export function formatSize(bytes: number): string {
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const digits = unit === 0 ? 0 : 1;
  return `${new Intl.NumberFormat("en", { maximumFractionDigits: digits }).format(value)} ${UNITS[unit]}`;
}
