import { useState } from "react";
import { useAnnounce } from "./Announcer";
import type { Api } from "./api";
import { t } from "./i18n";

/**
 * Copying text to the clipboard, and how it went: `outcome` is what the control shows beside its
 * button. A copy that worked is also said through the Announcer; one that failed is not, as the
 * user has to do something about it, so the control shows it as an alert.
 */
export function useCopy(api: Api) {
  const announce = useAnnounce();
  const [outcome, setOutcome] = useState<"idle" | "copied" | "failed">("idle");
  const copy = (text: string) =>
    api.copyText(text).then(
      () => {
        setOutcome("copied");
        announce(t("announce.copied"));
      },
      () => setOutcome("failed"),
    );
  return { outcome, copy };
}
