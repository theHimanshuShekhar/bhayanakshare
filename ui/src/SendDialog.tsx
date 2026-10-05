import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import type { Api } from "./api";
import { t } from "./i18n";
import { baseName } from "./transfers";

/**
 * "Send to ID…": paste a Device ID, pick files or a folder, and the Offer goes out. With `files`
 * already chosen (a second launch, or the tray's "Send files…"), there is nothing to pick: they
 * go out as soon as the Device is. Each of those is a Transfer of its own, files or folders.
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
  /** Files and folders to send instead of asking for some. */
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

  /** Sends what `pick` asks the user for, or the files already chosen if there are some. */
  const send = async (pick?: () => Promise<string[] | null>) => {
    setError(null);
    setBusy(true);
    try {
      if (pick !== undefined) {
        const paths = await pick();
        if (paths === null) return;
        await api.sendFiles(to.trim(), paths);
      } else {
        for (const path of remaining) {
          await api.sendFiles(to.trim(), [path]);
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
          {remaining.length > 0 ? (
            <button type="button" onClick={() => send()} disabled={busy || to.trim() === ""}>
              {t("send.sendFiles")}
            </button>
          ) : (
            <>
              <button
                type="button"
                onClick={() => send(api.pickFiles)}
                disabled={busy || to.trim() === ""}
              >
                {t("send.chooseFiles")}
              </button>
              <button
                type="button"
                onClick={() => send(() => api.pickFolder().then((folder) => (folder === null ? null : [folder])))}
                disabled={busy || to.trim() === ""}
              >
                {t("send.chooseFolder")}
              </button>
            </>
          )}
          <button type="button" onClick={onClose}>
            {t("send.cancel")}
          </button>
        </div>
      </div>
    </div>
  );
}
