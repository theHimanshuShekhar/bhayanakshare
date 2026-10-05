import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import type { Api } from "./api";
import { t } from "./i18n";
import { baseName } from "./transfers";

/**
 * "Send to ID…": paste a Device ID, pick a file, and the Offer goes out. With `files` already
 * chosen (a second launch, or the tray's "Send files…"), there is nothing to pick: they go out
 * as soon as the Device is.
 */
export function SendDialog({
  api,
  to: initialTo,
  contactName,
  files = [],
  onSent,
  onClose,
}: {
  api: Api;
  /** The Device ID to start with: a tile's, or empty. */
  to: string;
  /** What the tile calls the Device (a Contact's name, or a Nearby Device's name and
   * Fingerprint); null when the ID is typed. */
  contactName: string | null;
  /** Files to send instead of asking for one. */
  files?: string[];
  /** Called once every one of `files` has been offered. */
  onSent?: () => void;
  onClose: () => void;
}) {
  const [to, setTo] = useState(initialTo);
  // What has not been offered yet: a failure part-way keeps the rest for another try.
  const [remaining, setRemaining] = useState(files);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => {
    // Hand focus back to whatever opened the dialog when it goes away.
    const opener = document.activeElement;
    input.current?.focus();
    return () => {
      if (opener instanceof HTMLElement) opener.focus();
    };
  }, []);

  const send = async () => {
    setError(null);
    setBusy(true);
    try {
      if (remaining.length === 0) {
        const path = await api.pickFile();
        if (path === null) return;
        await api.sendFile(to.trim(), path);
      } else {
        for (const path of remaining) {
          await api.sendFile(to.trim(), path);
          setRemaining((left) => left.slice(1));
        }
        onSent?.();
      }
      onClose();
    } catch (e) {
      setError(t("send.failed", { reason: e instanceof Error ? e.message : String(e) }));
    } finally {
      setBusy(false);
    }
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") onClose();
  };

  return (
    <div className="overlay" onKeyDown={onKeyDown}>
      <div role="dialog" aria-modal="true" aria-labelledby="send-heading" className="sheet">
        <h2 id="send-heading">
          {contactName === null ? t("send.heading") : t("send.contactHeading", { name: contactName })}
        </h2>
        <label htmlFor="send-to">{t("send.idLabel")}</label>
        <input
          id="send-to"
          ref={input}
          value={to}
          onChange={(e) => setTo(e.target.value)}
          aria-describedby="send-to-hint"
          spellCheck={false}
          autoComplete="off"
        />
        <p id="send-to-hint" className="note">
          {t("send.idHint")}
        </p>
        {remaining.length > 0 && (
          <p>
            {t("send.files")}: {remaining.map(baseName).join(", ")}
          </p>
        )}
        {error !== null && <p role="alert">{error}</p>}
        <div className="actions">
          <button type="button" onClick={send} disabled={busy || to.trim() === ""}>
            {remaining.length > 0 ? t("send.sendFiles") : t("send.chooseFile")}
          </button>
          <button type="button" onClick={onClose}>
            {t("send.cancel")}
          </button>
        </div>
      </div>
    </div>
  );
}
