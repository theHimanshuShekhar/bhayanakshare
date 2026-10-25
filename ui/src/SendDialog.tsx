import { useRef, useState } from "react";
import type { Api } from "./api";
import { Banner, Hint } from "./Banner";
import { t } from "./i18n";
import { Sheet } from "./Sheet";
import { parseShareLink } from "./shareLink";
import { TextComposer, useFocusWhenLeft } from "./TextComposer";
import { baseName } from "./transfers";

/**
 * "Send to ID…": paste a Device ID (or share link), pick files or a folder or write text, and the
 * Offer goes out. With `files` already chosen (a second launch, or the tray's "Send files…"),
 * there is nothing to pick: they go out as soon as the Device is. Each of those is a Transfer of
 * its own, files or folders.
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
  // Writing text instead of picking files.
  const [composing, setComposing] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  const writeText = useFocusWhenLeft(composing);

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

  const idField = (
    <div className="bs-field">
      <label className="bs-label" htmlFor="send-to">
        {t("send.idLabel")}
      </label>
      <input
        id="send-to"
        className="bs-input bs-mono"
        ref={input}
        value={to}
        // A pasted share link becomes the Device ID in it; its name is of no use here.
        onChange={(e) => setTo(parseShareLink(e.target.value)?.id ?? e.target.value)}
        aria-describedby="send-to-hint"
        spellCheck={false}
        autoComplete="off"
      />
      <Hint id="send-to-hint">{t("send.idHint")}</Hint>
    </div>
  );

  return (
    <Sheet labelledBy="send-heading" onEscape={onClose} initialFocus={input}>
      <h2 id="send-heading" className="bs-sheet__t">
        {contactName === null ? t("send.heading") : t("send.contactHeading", { name: contactName })}
      </h2>
      {composing ? (
        <TextComposer
          inSheet
          send={(text) => api.sendText(to.trim(), text)}
          onSent={onClose}
          onBack={() => setComposing(false)}
          disabled={to.trim() === ""}
        >
          {idField}
        </TextComposer>
      ) : (
        <>
          <div className="bs-sheet__b">
            {idField}
            {remaining.length > 0 && (
              <p>
                {t("send.files")}: {remaining.map(baseName).join(", ")}
              </p>
            )}
            {error !== null && (
              <Banner kind="err" role="alert">
                {error}
              </Banner>
            )}
          </div>
          <div className="bs-sheet__a">
            {remaining.length > 0 ? (
              <button
                type="button"
                className="bs-btn bs-btn--pri"
                onClick={() => send()}
                disabled={busy || to.trim() === ""}
              >
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
                  onClick={() =>
                    send(() => api.pickFolder().then((folder) => (folder === null ? null : [folder])))
                  }
                  disabled={busy || to.trim() === ""}
                >
                  {t("send.chooseFolder")}
                </button>
                <button
                  type="button"
                  ref={writeText}
                  onClick={() => setComposing(true)}
                  disabled={busy || to.trim() === ""}
                >
                  {t("send.writeText")}
                </button>
              </>
            )}
            <button type="button" onClick={onClose}>
              {t("send.cancel")}
            </button>
          </div>
        </>
      )}
    </Sheet>
  );
}
