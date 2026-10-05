import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import type { Api } from "./api";
import { t } from "./i18n";

/**
 * Asks before History is cleared. Transfers still going stay, and files already received stay
 * where they were saved.
 */
export function ClearHistoryDialog({
  api,
  onCleared,
  onClose,
}: {
  api: Api;
  onCleared: () => void;
  onClose: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const keep = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    // Hand focus back to whatever opened the dialog when it goes away. The safe answer
    // starts focused, so Enter cannot clear History by accident.
    const opener = document.activeElement;
    keep.current?.focus();
    return () => {
      if (opener instanceof HTMLElement) opener.focus();
    };
  }, []);

  const clear = async () => {
    setError(null);
    setBusy(true);
    try {
      await api.clearHistory();
      onCleared();
      onClose();
    } catch (e) {
      setError(t("history.clearFailed", { reason: e instanceof Error ? e.message : String(e) }));
    } finally {
      setBusy(false);
    }
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") onClose();
  };

  return (
    <div className="overlay" onKeyDown={onKeyDown}>
      <div
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="clear-history-heading"
        aria-describedby="clear-history-body"
        className="sheet"
      >
        <h2 id="clear-history-heading">{t("history.clearHeading")}</h2>
        <p id="clear-history-body">{t("history.clearBody")}</p>
        {error !== null && <p role="alert">{error}</p>}
        <div className="actions">
          <button type="button" onClick={clear} disabled={busy}>
            {t("history.clearConfirm")}
          </button>
          <button type="button" ref={keep} onClick={onClose}>
            {t("history.clearCancel")}
          </button>
        </div>
      </div>
    </div>
  );
}
