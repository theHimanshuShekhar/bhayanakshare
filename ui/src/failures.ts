// Why a Transfer failed, in the error-message catalogue's words (section 3).
//
// The core gives a failed Transfer one plain-English reason, kept in History and sent to the other
// Device, so it is never shown as it is: each known reason is mapped here to the catalogue message
// for its cause, and anything else is the catch-all (C12), which shows the reason in brackets.
// `failureReasons.ts` is the core's list of reasons, generated from it (`pnpm bindings`).

import type { Role } from "./bindings";
import { FAILURE_REASONS, FAILURE_REASON_PREFIXES, FAILURE_REASON_SUFFIXES } from "./failureReasons";
import { t, type MessageKey } from "./i18n";

/** C12: the Transfer stopped for a reason nobody can fix from here. It shows the reason itself. */
export const UNEXPECTED_FAILURE = "failure.unexpected" satisfies MessageKey;

/** A message that depends on which side shows it has one for each. */
export type FailureMessage = MessageKey | Record<Role, MessageKey>;

/**
 * The message for each reason that is exactly one text, by the reason's id. A new reason in the
 * core fails the typecheck until it is given one here. The protocol errors and the failures
 * inside the core are C12 on purpose: nothing in them is for the user to act on.
 */
export const EXACT_FAILURES: Record<keyof typeof FAILURE_REASONS, FailureMessage> = {
  unreachable: "failure.unreachable",
  lost: "failure.connectionLost",
  unexpected: UNEXPECTED_FAILURE,
  busy: "failure.busy",
  invalid_names: "failure.invalidNames",
  stalled: { sender: "failure.stalledSending", receiver: "failure.stalledReceiving" },
  restarted: "failure.connectionLost",
  forgotten: "failure.forgotten",
  bad_offer: UNEXPECTED_FAILURE,
  cant_fetch: "failure.connectionLost",
  wrong_file: UNEXPECTED_FAILURE,
  too_much: UNEXPECTED_FAILURE,
  cant_save: "failure.cantSave",
  cant_keep_text: "failure.cantKeepText",
  cant_open: "failure.unreachable",
  cant_record: UNEXPECTED_FAILURE,
  cant_prepare_space: UNEXPECTED_FAILURE,
  cant_look_up: UNEXPECTED_FAILURE,
  cant_check_files: UNEXPECTED_FAILURE,
  cant_read_files: UNEXPECTED_FAILURE,
  cant_prepare_files: UNEXPECTED_FAILURE,
};

/** The message for each reason that starts with a text and ends with a file's name. */
export const PREFIXED_FAILURES: Record<keyof typeof FAILURE_REASON_PREFIXES, FailureMessage> = {
  file_changed: "failure.fileChanged",
};

/**
 * The message for each reason that starts with a Device's name and ends with a text. A Transfer
 * refused for its versions reads as the notice on Home does (catalogue C9).
 */
export const SUFFIXED_FAILURES: Record<keyof typeof FAILURE_REASON_SUFFIXES, FailureMessage> = {
  peer_older: "version.peerOlder",
  this_older: "version.thisOlder",
};

const forRole = (message: FailureMessage, role: Role): MessageKey =>
  typeof message === "string" ? message : message[role];

/** The catalogue message that says `reason` for `role`, and the file name in it, if it has one. */
export function failureMessageKey(reason: string, role: Role): { key: MessageKey; path: string } {
  for (const [id, text] of Object.entries(FAILURE_REASONS)) {
    if (reason === text) {
      return { key: forRole(EXACT_FAILURES[id as keyof typeof FAILURE_REASONS], role), path: "" };
    }
  }
  for (const [id, text] of Object.entries(FAILURE_REASON_PREFIXES)) {
    if (reason.startsWith(text)) {
      const message = PREFIXED_FAILURES[id as keyof typeof FAILURE_REASON_PREFIXES];
      return { key: forRole(message, role), path: reason.slice(text.length) };
    }
  }
  for (const [id, text] of Object.entries(FAILURE_REASON_SUFFIXES)) {
    if (reason.endsWith(text)) {
      return { key: forRole(SUFFIXED_FAILURES[id as keyof typeof FAILURE_REASON_SUFFIXES], role), path: "" };
    }
  }
  return { key: UNEXPECTED_FAILURE, path: "" };
}

/**
 * The sentence for a failed Transfer: its cause, then what to do. `role` is this Device's, and
 * `who` the other Device as its tile shows it (a Contact's name, or a name and Fingerprint).
 */
export function failureMessage(reason: string, role: Role, who: string): string {
  const { key, path } = failureMessageKey(reason, role);
  return t(key, { peer: who, name: who, path, reason });
}
