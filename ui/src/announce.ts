// What to say to a screen reader about Transfers, decided from how they were and how they are
// now. Pure functions only: Announcer.tsx feeds them state and speaks what they return.

import type { TransferId } from "./bindings";
import { t } from "./i18n";
import { statusText } from "./TransferList";
import {
  batchStatus,
  formatSize,
  isOver,
  isPreparing,
  listItems,
  percent,
  transferName,
  type Transfers,
  type TransferView,
} from "./transfers";

/** Progress is said at each quarter (25%, 50%, 75%); 100% is what "Received" and "Sent" say. */
export const PROGRESS_STEP_PERCENT = 25;

/** And at most this often for one Transfer, in event time, so a fast one is not read out. */
export const PROGRESS_MIN_GAP_MS = 10_000;

/** The last progress said for each Transfer: which quarter, and when. */
export type ProgressMemory = Record<TransferId, { bucket: number; at: number }>;

const line = (title: string, text: string) => t("announce.line", { title, text });

/**
 * What to announce now that Transfers went from `prev` to `next`, and the memory to give the
 * next call.
 *
 * - A change of what a Transfer's row says (Offer received, accepted, receiving, received,
 *   failed with its reason, declined, expired, cancelled, preparing, reconnecting) is said once,
 *   as the row words it. Saving is not: it is over in a moment and Received follows.
 * - Progress is said at each quarter, and not twice within ten seconds. A step reached too soon
 *   after the last is not lost: it is said by the first report after the ten seconds, as the
 *   quarter then reached.
 * - A Batch is said once, when its last Receiver is done, as its row words it ("2 of 3
 *   delivered, 1 declined"), and again if a retry ends it again. Its Receivers are not read out
 *   one by one: with many of them that would be a flood.
 */
export function announcements(
  prev: Transfers,
  next: Transfers,
  memory: ProgressMemory,
  peerOf: (view: TransferView) => string,
): { messages: string[]; memory: ProgressMemory } {
  const messages: string[] = [];
  let remembered = memory;

  for (const id of next.order) {
    const view = next.byId[id];
    if (view.batch !== null) continue;
    const was = prev.byId[id];
    const peer = peerOf(view);
    const title = t(view.role === "sender" ? "transfer.to" : "transfer.from", {
      name: transferName(view),
      peer,
    });

    if (view.state.kind !== "saving") {
      const now = statusText(view, peer, isPreparing(view));
      const before = was === undefined ? null : statusText(was, peerOf(was), isPreparing(was));
      if (now !== before) messages.push(line(title, now));
    }

    const moving = view.state.kind === "transferring" || view.state.kind === "reconnecting";
    if (moving && view.progressAt !== null && was?.bytes !== view.bytes) {
      const step = Math.floor(percent(view) / PROGRESS_STEP_PERCENT);
      const last = remembered[id];
      const due = last === undefined || (step > last.bucket && view.progressAt - last.at >= PROGRESS_MIN_GAP_MS);
      if (step >= 1 && step < 100 / PROGRESS_STEP_PERCENT && due) {
        messages.push(
          line(title, t("transfer.progress", { percent: percent(view), size: formatSize(view.size) })),
        );
        remembered = { ...remembered, [id]: { bucket: step, at: view.progressAt } };
      }
    }
  }

  const before = new Map<string, boolean>();
  for (const item of listItems(prev)) {
    if (item.kind === "batch") before.set(item.batch.id, item.batch.members.every((m) => isOver(m.state)));
  }
  for (const item of listItems(next)) {
    if (item.kind !== "batch") continue;
    const { batch } = item;
    if (batch.members.every((m) => isOver(m.state)) && before.get(batch.id) !== true) {
      const name = transferName(batch.members[0]);
      messages.push(line(t("batch.title", { name, count: batch.members.length }), batchStatus(batch)));
    }
  }

  return { messages, memory: remembered };
}
