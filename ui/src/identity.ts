// Identity export and import: what the dialogs ask of the user and how a failure is worded.

import type { IdentityError, IdentityErrorKind } from "./bindings";
import { t, type MessageKey } from "./i18n";

/**
 * The catalogue message (section 10) for each kind of identity failure. A kind added in the shell
 * fails the typecheck until it is worded.
 */
export const IDENTITY_ERRORS: Record<IdentityErrorKind, MessageKey> = {
  wrong_password: "identity.wrongPassword",
  not_an_identity_file: "identity.notAFile",
  store_unavailable: "identity.storeUnavailable",
  replace_uncertain: "identity.replaceUncertain",
  other: "identity.failed",
};

/** The shortest password an export is protected with. The core only refuses an empty one. */
export const MIN_PASSWORD_CHARS = 8;

/** What a save dialog suggests: the Fingerprint says whose identity is in the file. */
export function suggestedFileName(fingerprint: string): string {
  return `bhayanakshare-identity-${fingerprint}.bhid`;
}

/** Why a password cannot be used for an export, or null if it can. `confirmation` is what was
 * typed the second time. */
export function passwordProblem(password: string, confirmation: string): string | null {
  // Characters, not UTF-16 units: an emoji is one character to the person typing it.
  if ([...password].length < MIN_PASSWORD_CHARS) {
    return t("identity.tooShort", { min: MIN_PASSWORD_CHARS });
  }
  if (password !== confirmation) return t("identity.mismatch");
  return null;
}

function isIdentityError(e: unknown): e is IdentityError {
  return typeof e === "object" && e !== null && "kind" in e && "message" in e;
}

/** What went wrong with an export or import, in words for the person using the app. */
export function identityFailure(e: unknown): string {
  if (isIdentityError(e)) {
    return t(IDENTITY_ERRORS[e.kind], { reason: e.message });
  }
  return t("identity.failed", { reason: e instanceof Error ? e.message : String(e) });
}
