// Choosing the save folder: how a refused folder is worded.

import type { SaveFolderError, SaveFolderErrorKind } from "./bindings";
import { t, type MessageKey } from "./i18n";

/**
 * The catalogue message (section 6) for each kind of refused folder. `other` has the reason of the
 * system in brackets. A kind added in the shell fails the typecheck until it is worded.
 */
export const SAVE_FOLDER_ERRORS: Record<SaveFolderErrorKind, MessageKey> = {
  not_absolute: "saveFolder.error.notAbsolute",
  not_a_folder: "saveFolder.error.notAFolder",
  cannot_create: "saveFolder.error.cannotCreate",
  not_writable: "saveFolder.error.notWritable",
  not_text: "saveFolder.error.notText",
  other: "saveFolder.failed",
};

function isSaveFolderError(e: unknown): e is SaveFolderError {
  return typeof e === "object" && e !== null && "kind" in e && "message" in e;
}

/** Why a folder could not be made the save folder, in words for the person using the app. */
export function saveFolderFailure(e: unknown): string {
  if (isSaveFolderError(e)) {
    return t(SAVE_FOLDER_ERRORS[e.kind], { reason: e.message });
  }
  return t("saveFolder.failed", { reason: e instanceof Error ? e.message : String(e) });
}
