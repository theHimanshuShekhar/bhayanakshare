// The user-facing side of updating: looking for a release now and installing one. What the shell
// finds and how it installs are behind the `Api`.

import { useCallback, useState } from "react";
import type { Api, UpdateAction } from "./api";
import { t } from "./i18n";
import { RELEASES_URL } from "./versions";

/** Where an update has got to, for the one that asked for it. */
export type UpdateProgress =
  | { step: "checking" }
  | { step: "installing"; version: string }
  | { step: "failed"; message: string };

/** The version an action offers, or null when it offers nothing. */
export function offeredVersion(action: UpdateAction): string | null {
  return action.type === "none" ? null : action.version;
}

/**
 * Looking for and installing an update. `install` is for a release that was found; `updateNow`
 * looks first, then installs (an AppImage) or opens the release page (a package): where the
 * user has pressed a button that says so, that press is the agreement to install, and the
 * version is shown while it goes.
 */
export function useUpdater(api: Api) {
  const [progress, setProgress] = useState<UpdateProgress | null>(null);

  const install = useCallback(
    async (version: string) => {
      setProgress({ step: "installing", version });
      try {
        // The app restarts when this succeeds.
        await api.installUpdate();
      } catch (e) {
        const reason = e instanceof Error ? e.message : String(e);
        setProgress({ step: "failed", message: t("update.installFailed", { reason }) });
      }
    },
    [api],
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

  const busy = progress?.step === "checking" || progress?.step === "installing";
  return { progress, busy, install, updateNow };
}
