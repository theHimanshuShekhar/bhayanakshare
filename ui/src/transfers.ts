// What the UI knows about Transfers, built up from the Device's event stream. Pure functions
// only: the React side just feeds events in and renders the result.

import type { DeviceEvent, Role, TransferId, TransferState } from "./bindings";

export interface TransferView {
  id: TransferId;
  role: Role;
  /** The other Device's ID. */
  peer: string;
  name: string;
  size: number;
  state: TransferState;
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
      ? { ...known, state: event.state }
      : {
          id: event.transfer_id,
          role: event.role,
          peer: event.peer,
          name: event.name,
          size: event.size,
          state: event.state,
          bytes: 0,
          rate: null,
          progressAt: null,
        };
    next.byId = { ...transfers.byId, [view.id]: event.state.kind === "completed" ? done(view) : view };
    next.order = known ? transfers.order : [...transfers.order, view.id];
    return next;
  }

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

/** The oldest incoming Offer still waiting for an answer. */
export function pendingOffer(transfers: Transfers): TransferView | undefined {
  return transfers.order
    .map((id) => transfers.byId[id])
    .find((x) => x.role === "receiver" && x.state.kind === "offered");
}

/** Whole percent received, 0 to 100. An empty file is complete as soon as it is known. */
export function percent(view: TransferView): number {
  if (view.size === 0) return 100;
  return Math.min(100, Math.floor((view.bytes * 100) / view.size));
}

/** `XXXX-XXXX`: the first 8 characters of a Device ID, for checking by eye. */
export function fingerprint(deviceId: string): string {
  return `${deviceId.slice(0, 4)}-${deviceId.slice(4, 8)}`;
}

const UNITS = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];

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
