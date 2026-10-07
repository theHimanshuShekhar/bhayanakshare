// The user-facing side of updating: looking for a release now and installing one. What the shell
// finds and how it installs are behind the `Api`.

import { useCallback, useState } from "react";
import type { Api, UpdateAction, UpdateError } from "./api";
import { t, type MessageKey } from "./i18n";
import { RELEASES_URL } from "./versions";

/** Where an update has got to. There is one, for the whole window. */
export type UpdateProgress =
  /** Looking for a release. */
  | { step: "checking" }
  /** Finding out whether Transfers are in progress, which the user is told about first. */
  | { step: "preparing" }
  /** Transfers are in progress and stop for the restart: waiting for the user to say go. */
  | { step: "confirm"; version: string; active: number }
  | { step: "installing"; version: string }
  /** A check in Settings found nothing newer. */
  | { step: "up_to_date" }
  | { step: "failed"; message: string };

const ERRORS: Record<UpdateError, MessageKey> = {
  not_app_image: "update.error.notAppImage",
  none_pending: "update.error.nonePending",
  version_changed: "update.error.versionChanged",
  already_installing: "update.error.alreadyInstalling",
  quitting: "update.error.quitting",
  download_failed: "update.error.downloadFailed",
  signature_invalid: "update.error.signatureInvalid",
  install_failed: "update.error.installFailed",
};

/** What a rejected install says, in words. Anything that is not a known kind is a failed install. */
export function updateErrorMessage(e: unknown): string {
  const key = typeof e === "string" ? ERRORS[e as UpdateError] : undefined;
  return t(key ?? "update.error.installFailed");
}

/** The version an action offers, or null when it offers nothing. */
export function offeredVersion(action: UpdateAction): string | null {
  return action.type === "none" ? null : action.version;
}

/**
 * Looking for and installing an update. `install` is for a release that was found; `updateNow`
 * looks first, then installs (an AppImage) or opens the release page (a package): where the
 * user has pressed a button that says so, that press is the agreement to install, and the
 * version is shown while it goes. Either way, with Transfers in progress the user is asked
 * first (`confirm` or `cancel`), as they stop for the restart.
 *
 * `check` only looks, for Settings: what it finds is left for the user to install (or not), and
 * is handed back so the app can note it.
 *
 * Used once, by the app, so that everything that can start an update sees whether one is
 * under way.
 */
export function useUpdater(api: Api) {
  const [progress, setProgress] = useState<UpdateProgress | null>(null);

  const run = useCallback(
    async (agreedVersion: string) => {
      setProgress({ step: "installing", version: agreedVersion });
      try {
        // The app restarts when this succeeds.
        await api.installUpdate(agreedVersion);
      } catch (e) {
        setProgress({ step: "failed", message: updateErrorMessage(e) });
      }
    },
    [api],
  );

  const install = useCallback(
    async (agreedVersion: string) => {
      setProgress({ step: "preparing" });
      let active = 0;
      try {
        active = await api.transfersInProgress();
      } catch {
        // Not knowing is not a reason to refuse: the Device saves its progress either way.
      }
      if (active > 0) setProgress({ step: "confirm", version: agreedVersion, active });
      else await run(agreedVersion);
    },
    [api, run],
  );

  const updateNow = useCallback(async () => {
    setProgress({ step: "checking" });
    let found: UpdateAction;
    try {
      found = await api.checkForUpdate();
    } catch {
      setProgress({ step: "failed", message: t("update.checkFailed") });
      return;
    }
    switch (found.type) {
      case "none":
        setProgress({ step: "failed", message: t("update.none") });
        break;
      case "open_page":
        setProgress(null);
        api.openUrl(RELEASES_URL).catch(() => {});
        break;
      case "install":
        await install(found.version);
        break;
    }
  }, [api, install]);

  const check = useCallback(async (): Promise<UpdateAction | null> => {
    setProgress({ step: "checking" });
    try {
      const found = await api.checkForUpdate();
      setProgress(found.type === "none" ? { step: "up_to_date" } : null);
      return found;
    } catch {
      setProgress({ step: "failed", message: t("update.checkFailed") });
      return null;
    }
  }, [api]);

  /** Drops the answer of a check (nothing newer), which means nothing once Settings is closed. */
  const forget = useCallback(() => setProgress((p) => (p?.step === "up_to_date" ? null : p)), []);

  const confirm = useCallback(async () => {
    if (progress?.step === "confirm") await run(progress.version);
  }, [progress, run]);
  const cancel = useCallback(() => setProgress(null), []);

  /** Something is under way, or waiting for an answer: nothing else may start. */
  const busy = progress !== null && progress.step !== "failed" && progress.step !== "up_to_date";
  return { progress, busy, install, updateNow, check, forget, confirm, cancel };
}

export type Updater = ReturnType<typeof useUpdater>;
