import { useRef, useState } from "react";
import type { Api } from "./api";
import { Banner } from "./Banner";
import { t } from "./i18n";
import { Sheet } from "./Sheet";

/**
 * Quit with Transfers in progress: asks first, since they stop until BhayanakShare next runs,
 * then says it is saving their progress while the Device shuts down.
 */
export function QuitDialog({
  api,
  active,
  saving,
  onSaving,
  onCancel,
}: {
  api: Api;
  /** How many Transfers are in progress. */
  active: number;
  /** Shutdown has begun; there is nothing left to choose. */
  saving: boolean;
  /** Shutdown began (true), or turned out not to have (false). */
  onSaving: (saving: boolean) => void;
  onCancel: () => void;
}) {
  const [error, setError] = useState<string | null>(null);
  const keep = useRef<HTMLButtonElement>(null);

  const quit = () => {
    setError(null);
    onSaving(true);
    api.quitApp().catch((e) => {
      onSaving(false);
      setError(t("quit.failed", { reason: e instanceof Error ? e.message : String(e) }));
    });
  };

  return (
    <Sheet labelledBy="quit-heading" onEscape={saving ? undefined : onCancel} initialFocus={keep}>
      <h2 id="quit-heading" className="bs-sheet__t">
        {saving ? t("quit.saving") : t("quit.heading")}
      </h2>
      {saving ? (
        <div className="bs-sheet__b">
          <p role="status">{t("quit.savingHint")}</p>
          <div className="bs-bar bs-bar--prep" role="progressbar" aria-label={t("quit.saving")} />
        </div>
      ) : (
        <>
          <div className="bs-sheet__b">
            <p>
              {active === 1 ? t("quit.inProgressOne") : t("quit.inProgress", { count: active })}{" "}
              {active === 1 ? t("quit.resumeOne") : t("quit.resume")}
            </p>
            {error !== null && (
              <Banner kind="err" role="alert">
                {error}
              </Banner>
            )}
          </div>
          <div className="bs-sheet__a">
            <button type="button" className="bs-btn bs-btn--pri" ref={keep} onClick={onCancel}>
              {t("quit.cancel")}
            </button>
            <button type="button" className="bs-btn" onClick={quit}>
              {t("quit.confirm")}
            </button>
          </div>
        </>
      )}
    </Sheet>
  );
}
