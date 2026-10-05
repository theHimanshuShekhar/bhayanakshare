// What the UI makes of Transfer History: how its entries are read, which Devices can be filtered
// on, and when a Failed Transfer can still be retried. Pure functions only.

import type { Contact, HistoryEntry, HistoryTransfer, TransferId } from "./bindings";
import { peerName } from "./contacts";

/** The Transfers of `entries`, oldest first. Entries are newest first; a Batch's are oldest first. */
export function oldestFirst(entries: HistoryEntry[]): HistoryTransfer[] {
  return [...entries].reverse().flatMap((e) => (e.kind === "batch" ? e.transfers : [e.transfer]));
}

/**
 * The Failed Transfers that have been sent again: a later Transfer of the same Batch went to the
 * same Receiver. Each can be retried once, so the Device refuses a second time.
 */
export function retriedIds(entries: HistoryEntry[]): Set<TransferId> {
  const retried = new Set<TransferId>();
  const latest = new Map<string, TransferId>();
  for (const { record } of oldestFirst(entries)) {
    if (record.batch_id === null) continue;
    const key = `${record.batch_id}/${record.peer}`;
    const earlier = latest.get(key);
    if (earlier !== undefined) retried.add(earlier);
    latest.set(key, record.id);
  }
  return retried;
}

/** A Failed Transfer of a Batch that this Device sent and has not yet sent again. */
export function canRetry(transfer: HistoryTransfer, retried: Set<TransferId>): boolean {
  const { record } = transfer;
  return (
    record.role === "sender" &&
    record.batch_id !== null &&
    record.state.kind === "failed" &&
    !retried.has(record.id)
  );
}

/** One Transfer per Receiver of a Batch's, the latest sent to each, in the order first sent to. */
export function latestPerReceiver(transfers: HistoryTransfer[]): HistoryTransfer[] {
  const latest = new Map<string, HistoryTransfer>();
  // Replacing a value keeps the Receiver's place in the Map.
  for (const transfer of transfers) latest.set(transfer.record.peer, transfer);
  return [...latest.values()];
}

/** The Devices seen in History so far, by ID, with the name each last called itself. */
export type KnownDevices = Record<string, string | null>;

/** Adds the Devices of `entries`, which are newest first, to those already `known`. */
export function noteDevices(known: KnownDevices, entries: HistoryEntry[]): KnownDevices {
  const next = { ...known };
  // The newest Transfer that has a name says what the Device is called now.
  for (const { record } of oldestFirst(entries)) {
    next[record.peer] = record.peer_name ?? next[record.peer] ?? null;
  }
  return next;
}

/** A Device to filter History by. */
export interface DeviceOption {
  id: string;
  name: string;
}

/** Every Contact and every Device seen in History, by the name they go by here, sorted by it. */
export function deviceOptions(contacts: Contact[], known: KnownDevices): DeviceOption[] {
  const ids = new Set([...contacts.map((c) => c.id), ...Object.keys(known)]);
  return [...ids]
    .map((id) => ({ id, name: peerName(id, contacts, known[id] ?? null) }))
    .sort((a, b) => a.name.localeCompare(b.name) || a.id.localeCompare(b.id));
}

/** A time in the user's own format, for a place with room for the date. */
export function formatTime(millis: number): string {
  return new Date(millis).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}
