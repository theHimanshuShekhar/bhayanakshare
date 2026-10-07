// What to say to a screen reader about Transfers, decided from how they were and how they are
// now. Pure functions only: Announcer.tsx feeds them state and speaks what they return.

import type { TransferId } from "./bindings";
import { t } from "./i18n";
import {
  batchStatus,
  formatSize,
  isOver,
  isPreparing,
  listItems,
  pendingOffer,
  percent,
  statusText,
  transferName,
  type Transfers,
  type TransferView,
} from "./transfers";

/** Progress is said at each quarter (25%, 50%, 75%); 100% is what "Received" and "Sent" say. */
export const PROGRESS_STEP_PERCENT = 25;

/** And at most this often for one Transfer or Batch, so a fast one is not read out. */
export const PROGRESS_MIN_GAP_MS = 10_000;

/**
 * What was last said and when, for the ways of speaking that are limited: progress, by the
 * Transfer's ID (the time is the event time of its report) and by `progress:<Batch ID>`; how a
 * Batch stands, by `status:<Batch ID>` (the time is `now`; the quarter is not used).
 */
export type ProgressMemory = Record<TransferId | string, { bucket: number; at: number }>;

const line = (title: string, text: string) => t("announce.line", { title, text });

const titleOf = (view: TransferView, peer: string) =>
  t(view.role === "sender" ? "transfer.to" : "transfer.from", { name: transferName(view), peer });

/**
 * What to announce now that Transfers went from `prev` to `next` (`now` is when, in
 * milliseconds), and the memory to give the next call.
 *
 * - A change of what a Transfer's row says (Offer received, accepted, receiving, received,
 *   failed with its reason, declined, expired, cancelled, preparing, reconnecting) is said once,
 *   as the row words it. Saving is not: it is over in a moment and Received follows. Nor is the
 *   Offer that the Offer sheet opens for, which takes focus and reads itself; an Offer that
 *   arrives while another is being answered is, as nothing is focused for it.
 * - Progress is said at each quarter, and not twice within ten seconds. A step reached too soon
 *   after the last is not lost: it is said by the first report after the ten seconds, as the
 *   quarter then reached.
 * - A Batch is said as a whole: when it starts and whenever how it stands changes ("2 of 3
 *   delivered, 1 declined"), but not twice within ten seconds, except when its last Receiver is
 *   done, which is always said (and again if a retry ends it again); and its progress, taken from
 *   all its Receivers together, at the same quarters and gap. Receivers are not read out one by
 *   one, since with many of them that would be a flood, except one whose Transfer failed, at
 *   once with its reason, as it can be retried.
 */
export function announcements(
  prev: Transfers,
  next: Transfers,
  memory: ProgressMemory,
  peerOf: (view: TransferView) => string,
  now: number,
): { messages: string[]; memory: ProgressMemory } {
  const messages: string[] = [];
  let remembered = memory;
  const remember = (key: string, bucket: number, at: number) => {
    remembered = { ...remembered, [key]: { bucket, at } };
  };
  const quiet = (key: string) => {
    const last = remembered[key];
    return last === undefined || now - last.at >= PROGRESS_MIN_GAP_MS;
  };
  const opens = pendingOffer(next);
  const opened = pendingOffer(prev) === undefined;

  for (const id of next.order) {
    const view = next.byId[id];
    const was = prev.byId[id];
    const peer = peerOf(view);
    const title = titleOf(view, peer);

    if (view.batch !== null) {
      if (view.state.kind === "failed" && was?.state.kind !== "failed") {
        messages.push(line(title, statusText(view, peer)));
      }
      continue;
    }

    // The sheet for this Offer is opening, and takes focus.
    const sheet = was === undefined && opened && opens?.id === id;
    if (view.state.kind !== "saving" && !sheet) {
      const text = statusText(view, peer, isPreparing(view));
      const before = was === undefined ? null : statusText(was, peerOf(was), isPreparing(was));
      if (text !== before) messages.push(line(title, text));
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
        remember(id, step, view.progressAt);
      }
    }
  }

  const before = new Map<string, { summary: string; over: boolean; bytes: number }>();
  const standing = (members: TransferView[]) => ({
    summary: batchStatus({ members }),
    over: members.every((m) => isOver(m.state)),
    bytes: members.reduce((sum, m) => sum + m.bytes, 0),
  });
  for (const item of listItems(prev)) {
    if (item.kind === "batch") before.set(item.batch.id, standing(item.batch.members));
  }
  for (const item of listItems(next)) {
    if (item.kind !== "batch") continue;
    const { batch } = item;
    const was = before.get(batch.id);
    const { summary, over, bytes } = standing(batch.members);
    const title = t("batch.title", { name: transferName(batch.members[0]), count: batch.members.length });

    let said = false;
    if (summary !== was?.summary && (over ? was?.over !== true : quiet(`status:${batch.id}`))) {
      messages.push(line(title, summary));
      remember(`status:${batch.id}`, 0, now);
      said = true;
    }

    // The quarter of all the Receivers' content that has arrived. Said only if the standing was
    // not just said: they are the same news.
    const size = batch.members.reduce((sum, m) => sum + m.size, 0);
    const step = size === 0 ? 0 : Math.floor((bytes * 100) / size / PROGRESS_STEP_PERCENT);
    const last = remembered[`progress:${batch.id}`];
    const due = last === undefined || (step > last.bucket && quiet(`progress:${batch.id}`));
    if (!over && bytes !== (was?.bytes ?? 0) && step >= 1 && step < 100 / PROGRESS_STEP_PERCENT && due) {
      if (!said) {
        const done = Math.floor((bytes * 100) / size);
        messages.push(line(title, t("transfer.progress", { percent: done, size: formatSize(size) })));
      }
      remember(`progress:${batch.id}`, step, now);
    }
  }

  return { messages, memory: remembered };
}
