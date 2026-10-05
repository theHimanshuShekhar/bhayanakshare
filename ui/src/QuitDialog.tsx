import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import type { Api } from "./api";
import { t } from "./i18n";

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

  useEffect(() => {
    // Hand focus back to whatever had it when the dialog goes away.
    const opener = document.activeElement;
    keep.current?.focus();
    return () => {
      if (opener instanceof HTMLElement) opener.focus();
    };
  }, []);

  const quit = () => {
    setError(null);
    onSaving(true);
    api.quitApp().catch((e) => {
      onSaving(false);
      setError(t("quit.failed", { reason: e instanceof Error ? e.message : String(e) }));
    });
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape" && !saving) onCancel();
  };

  return (
    <div className="overlay" onKeyDown={onKeyDown}>
      <div role="dialog" aria-modal="true" aria-labelledby="quit-heading" className="sheet">
        <h2 id="quit-heading">{saving ? t("quit.saving") : t("quit.heading")}</h2>
        {saving ? (
          <p role="status">{t("quit.savingHint")}</p>
        ) : (
          <>
            <p>
              {active === 1 ? t("quit.inProgressOne") : t("quit.inProgress", { count: active })}{" "}
              {active === 1 ? t("quit.resumeOne") : t("quit.resume")}
            </p>
            {error !== null && <p role="alert">{error}</p>}
            <div className="actions">
              <button type="button" onClick={quit}>
                {t("quit.confirm")}
              </button>
              <button type="button" ref={keep} onClick={onCancel}>
                {t("quit.cancel")}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
