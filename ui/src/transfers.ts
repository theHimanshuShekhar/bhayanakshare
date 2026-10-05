// What the UI knows about Transfers, built up from the Device's event stream. Pure functions
// only: the React side just feeds events in and renders the result.

import type { BatchId, DeviceEvent, Role, TransferId, TransferKind, TransferState } from "./bindings";
import { t } from "./i18n";

export interface TransferView {
  id: TransferId;
  role: Role;
  /** The Batch a Sender made this Transfer in; null for a Receiver's and for a lone send. */
  batch: BatchId | null;
  /** The other Device's ID. */
  peer: string;
  /** What the other Device calls itself, as it announced; null until known. Untrusted text. */
  peerName: string | null;
  /** What it carries: files and folders, or a text that went in the Offer itself. */
  kind: TransferKind;
  /** The text of a text Transfer. Untrusted when received: show it as plain text only. */
  text: string | null;
  /** The first of `items`; empty for text. */
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
  /**
   * Whether the Sender is still hashing the files. It overlays Offered and Accepted (see
   * `isPreparing`); false for text, which has nothing to hash.
   */
  preparing: boolean;
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
          batch: event.batch_id,
          peer: event.peer,
          peerName: event.peer_name,
          kind: event.kind,
          text: event.text,
          name: event.name,
          items: event.items,
          fileCount: event.file_count,
          skippedLinks: event.skipped_links,
          adjustedNames: event.adjusted_names,
          size: event.size,
          state: event.state,
          preparing: false,
          expiresAt: event.expires_at,
          bytes: 0,
          rate: null,
          progressAt: null,
        };
    next.byId = { ...transfers.byId, [view.id]: event.state.kind === "completed" ? done(view) : view };
    next.order = known ? transfers.order : [...transfers.order, view.id];
    return next;
  }

  if (event.type === "preparing") {
    const known = transfers.byId[event.transfer_id];
    if (known) next.byId = { ...transfers.byId, [known.id]: { ...known, preparing: event.preparing } };
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
    case "waiting":
    case "transferring":
    case "reconnecting":
      return true;
    default:
      return false;
  }
}

/**
 * Whether to show Preparing (spec section 4): the files are still being hashed, and the
 * Transfer is Offered or Accepted. A Receiver shows it only once it has accepted; before that
 * it is deciding, which is what the Offer sheet is for.
 */
export function isPreparing(view: TransferView): boolean {
  if (!view.preparing) return false;
  return view.state.kind === "accepted" || (view.state.kind === "offered" && view.role === "sender");
}

/** A Sender can send an Offer nobody answered again in one step. */
export function canResend(view: TransferView): boolean {
  return view.role === "sender" && view.state.kind === "expired";
}

/**
 * A Transfer in a Batch that failed can be sent again to its Receiver with a new Offer. One
 * the Receiver declined cannot: that is their answer.
 */
export function canRetry(view: TransferView): boolean {
  return view.role === "sender" && view.batch !== null && view.state.kind === "failed";
}

/** A Batch as the Sender sees it: its Transfers, grouped. */
export interface BatchView {
  id: BatchId;
  /**
   * One Transfer per Receiver, in the order the Receivers were first sent to: the latest sent
   * to each, so a retry stands in for the Failed Transfer it retries.
   */
  members: TransferView[];
}

/** A row of the Transfer list: a Transfer on its own, or a whole Batch. */
export type ListItem =
  | { kind: "transfer"; transfer: TransferView }
  | { kind: "batch"; batch: BatchView };

/**
 * The rows of the Transfer list, newest first. A Batch is one row, where its first Transfer
 * began; every other Transfer, and every Receiver's, is a row of its own.
 */
export function listItems(transfers: Transfers): ListItem[] {
  const items: ListItem[] = [];
  const batches = new Map<BatchId, Map<string, TransferView>>();
  for (const id of transfers.order) {
    const view = transfers.byId[id];
    if (view.batch === null) {
      items.push({ kind: "transfer", transfer: view });
      continue;
    }
    let members = batches.get(view.batch);
    if (members === undefined) {
      members = new Map();
      batches.set(view.batch, members);
      items.push({ kind: "batch", batch: { id: view.batch, members: [] } });
    }
    // Replacing a value keeps the Receiver's place in the Map.
    members.set(view.peer, view);
  }
  for (const item of items) {
    if (item.kind === "batch") item.batch.members = [...batches.get(item.batch.id)!.values()];
  }
  return items.reverse();
}

/** "2 of 3 delivered, 1 declined": how a Batch stands, counting its Receivers by outcome. */
export function batchStatus(batch: { members: Pick<TransferView, "state">[] }): string {
  const count = (matches: (state: TransferState) => boolean) =>
    batch.members.filter((m) => matches(m.state)).length;
  const kinds = (...wanted: TransferState["kind"][]) => count((s) => wanted.includes(s.kind));
  const parts = [t("batch.delivered", { done: kinds("completed"), total: batch.members.length })];
  const others = [
    ["batch.declined", kinds("declined")],
    ["batch.failed", kinds("failed")],
    ["batch.cancelled", kinds("cancelled")],
    ["batch.expired", kinds("expired")],
    ["batch.inProgress", count((s) => !isOver(s))],
  ] as const;
  for (const [key, n] of others) if (n > 0) parts.push(t(key, { count: n }));
  return parts.join(", ");
}

/** Whether a Transfer in this state has ended, one way or another. */
export function isOver(state: TransferState): boolean {
  return ["declined", "completed", "failed", "expired", "cancelled"].includes(state.kind);
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
export function transferName(view: Pick<TransferView, "kind" | "name" | "items">): string {
  if (view.kind === "text") return t("transfer.textName");
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
