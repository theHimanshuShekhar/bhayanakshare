import { useEffect, useRef, useState, type FormEvent, type KeyboardEvent } from "react";
import type { Api } from "./api";
import { t } from "./i18n";
import { identityFailure, passwordProblem, suggestedFileName, MIN_PASSWORD_CHARS } from "./identity";

/**
 * "Export identity…": asks for a password twice, then where to save the file. The file holds
 * the secret key and nothing else.
 */
export function ExportIdentityDialog({
  api,
  fingerprint,
  onClose,
}: {
  api: Api;
  /** This Device's Fingerprint, which names the file the save dialog suggests. */
  fingerprint: string;
  onClose: () => void;
}) {
  const [password, setPassword] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [savedTo, setSavedTo] = useState<string | null>(null);
  const first = useRef<HTMLInputElement>(null);
  const close = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    // Hand focus back to whatever opened the dialog when it goes away.
    const opener = document.activeElement;
    return () => {
      if (opener instanceof HTMLElement) opener.focus();
    };
  }, []);

  // Focus starts on the first field, and moves to Close once the file is saved.
  useEffect(() => {
    (savedTo === null ? first : close).current?.focus();
  }, [savedTo]);

  const save = async (e: FormEvent) => {
    e.preventDefault();
    const problem = passwordProblem(password, confirmation);
    if (problem !== null) {
      setError(problem);
      first.current?.focus();
      return;
    }
    setError(null);
    setBusy(true);
    try {
      const path = await api.pickIdentitySavePath(suggestedFileName(fingerprint));
      // Cancelling the save dialog leaves this one as it was.
      if (path === null) return;
      await api.exportIdentity(path, password);
      setPassword("");
      setConfirmation("");
      setSavedTo(path);
    } catch (e) {
      setError(identityFailure(e));
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
        role="dialog"
        aria-modal="true"
        aria-labelledby="export-identity-heading"
        aria-describedby="export-identity-body"
        className="sheet"
      >
        <h2 id="export-identity-heading">{t("identity.exportHeading")}</h2>
        <p id="export-identity-body">{t("identity.exportBody")}</p>
        {savedTo !== null ? (
          <>
            <p role="status">{t("identity.exported", { path: savedTo })}</p>
            <div className="actions">
              <button type="button" ref={close} onClick={onClose}>
                {t("identity.close")}
              </button>
            </div>
          </>
        ) : (
          <form onSubmit={save}>
            <label htmlFor="export-identity-password">{t("identity.password")}</label>
            <input
              id="export-identity-password"
              ref={first}
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              aria-describedby="export-identity-password-hint"
              autoComplete="new-password"
            />
            <p id="export-identity-password-hint" className="note">
              {t("identity.passwordHint", { min: MIN_PASSWORD_CHARS })}
            </p>
            <label htmlFor="export-identity-confirm">{t("identity.confirm")}</label>
            <input
              id="export-identity-confirm"
              type="password"
              value={confirmation}
              onChange={(e) => setConfirmation(e.target.value)}
              autoComplete="new-password"
            />
            {error !== null && <p role="alert">{error}</p>}
            <div className="actions">
              <button type="submit" disabled={busy}>
                {t("identity.exportSave")}
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
