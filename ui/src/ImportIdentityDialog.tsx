import { useEffect, useRef, useState, type FormEvent, type KeyboardEvent } from "react";
import type { Api, MyId } from "./api";
import { t } from "./i18n";
import { identityFailure } from "./identity";
import { baseName } from "./transfers";

/**
 * "Import identity…", once the user has chosen a file: its password, which is checked before
 * anything is asked of the user, then a warning that the Device ID is replaced, and the app
 * restarts. The safe answer starts focused, so Enter cannot replace an identity by accident.
 */
export function ImportIdentityDialog({
  api,
  path,
  current,
  onClose,
}: {
  api: Api;
  /** The identity file the user chose. */
  path: string;
  /** This Device's own ID, to tell the user what is being replaced. */
  current: MyId;
  onClose: () => void;
}) {
  const [password, setPassword] = useState("");
  // Set once the password has opened the file: whose identity it holds.
  const [incoming, setIncoming] = useState<MyId | null>(null);
  const [restarting, setRestarting] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const passwordInput = useRef<HTMLInputElement>(null);
  const keep = useRef<HTMLButtonElement>(null);
  const same = incoming !== null && incoming.id === current.id;
  const confirming = incoming !== null && !same;

  useEffect(() => {
    // Hand focus back to whatever opened the dialog when it goes away.
    const opener = document.activeElement;
    return () => {
      if (opener instanceof HTMLElement) opener.focus();
    };
  }, []);

  // Each step starts with focus on its first control: the password, then the safe answer.
  useEffect(() => {
    (confirming ? keep : passwordInput).current?.focus();
  }, [confirming]);

  const check = async (e: FormEvent) => {
    e.preventDefault();
    setError(null);
    setBusy(true);
    try {
      setIncoming(await api.checkIdentityImport(path, password));
    } catch (e) {
      setError(identityFailure(e));
      passwordInput.current?.focus();
    } finally {
      setBusy(false);
    }
  };

  const replace = async () => {
    setError(null);
    setBusy(true);
    try {
      await api.importIdentity(path, password);
      // The app is on its way out and back in; this window is about to go.
      setRestarting(true);
    } catch (e) {
      setError(identityFailure(e));
      setBusy(false);
    }
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape" && !restarting) onClose();
  };

  return (
    <div className="overlay" onKeyDown={onKeyDown}>
      <div
        role={confirming ? "alertdialog" : "dialog"}
        aria-modal="true"
        aria-labelledby="import-identity-heading"
        aria-describedby={confirming ? "import-identity-body" : undefined}
        className="sheet"
      >
        <h2 id="import-identity-heading">
          {confirming ? t("identity.replaceHeading") : t("identity.importHeading")}
        </h2>
        {confirming ? (
          <>
            <div id="import-identity-body">
              <p>
                {t("identity.replaceId", { current: current.fingerprint, incoming: incoming.fingerprint })}
              </p>
              <p>{t("identity.replaceOld")}</p>
              <p>{t("identity.replaceRestart")}</p>
            </div>
            {error !== null && <p role="alert">{error}</p>}
            {restarting && <p role="status">{t("identity.restarting")}</p>}
            <div className="actions">
              <button type="button" onClick={replace} disabled={busy || restarting}>
                {t("identity.replaceConfirm")}
              </button>
              <button type="button" ref={keep} onClick={onClose} disabled={restarting}>
                {t("identity.cancel")}
              </button>
            </div>
          </>
        ) : same ? (
          <>
            <p role="status">{t("identity.importSame", { fingerprint: current.fingerprint })}</p>
            <div className="actions">
              <button type="button" onClick={onClose}>
                {t("identity.close")}
              </button>
            </div>
          </>
        ) : (
          <form onSubmit={check}>
            <p>{t("identity.importFile", { name: baseName(path) })}</p>
            <label htmlFor="import-identity-password">{t("identity.importPassword")}</label>
            <input
              id="import-identity-password"
              ref={passwordInput}
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              autoComplete="current-password"
            />
            {error !== null && <p role="alert">{error}</p>}
            <div className="actions">
              <button type="submit" disabled={busy || password === ""}>
                {t("identity.importNext")}
              </button>
              <button type="button" onClick={onClose}>
                {t("identity.cancel")}
              </button>
            </div>
          </form>
        )}
      </div>
    </div>
  );
}
