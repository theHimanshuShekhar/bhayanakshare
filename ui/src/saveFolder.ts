// Choosing the save folder: how a refused folder is worded.

import type { SaveFolderError } from "./bindings";
import { t } from "./i18n";

function isSaveFolderError(e: unknown): e is SaveFolderError {
  return typeof e === "object" && e !== null && "kind" in e && "message" in e;
}

/** Why a folder could not be made the save folder, in words for the person using the app. */
export function saveFolderFailure(e: unknown): string {
  if (isSaveFolderError(e)) {
    switch (e.kind) {
      case "not_absolute":
        return t("saveFolder.error.notAbsolute");
      case "not_a_folder":
        return t("saveFolder.error.notAFolder");
      case "cannot_create":
        return t("saveFolder.error.cannotCreate");
      case "not_writable":
        return t("saveFolder.error.notWritable");
      case "not_text":
        return t("saveFolder.error.notText");
      case "other":
        return t("saveFolder.failed", { reason: e.message });
    }
  }
  return t("saveFolder.failed", { reason: e instanceof Error ? e.message : String(e) });
}
