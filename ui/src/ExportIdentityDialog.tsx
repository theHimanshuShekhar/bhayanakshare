import { useEffect, useRef, useState, type FormEvent } from "react";
import type { Api } from "./api";
import { Banner, FieldError, Hint } from "./Banner";
import { t } from "./i18n";
import { Sheet } from "./Sheet";
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
  // The field an error is about, when it is about one: it is said under that field, which is
  // marked. An error about saving the file is a notice instead.
  const [errorField, setErrorField] = useState<"password" | "confirm" | null>(null);
  const [savedTo, setSavedTo] = useState<string | null>(null);
  const first = useRef<HTMLInputElement>(null);
  const close = useRef<HTMLButtonElement>(null);

  // Focus starts on the first field, and moves to Close once the file is saved.
  useEffect(() => {
    (savedTo === null ? first : close).current?.focus();
  }, [savedTo]);

  const save = async (e: FormEvent) => {
    e.preventDefault();
    const problem = passwordProblem(password, confirmation);
    if (problem !== null) {
      setError(problem);
      setErrorField([...password].length < MIN_PASSWORD_CHARS ? "password" : "confirm");
      first.current?.focus();
      return;
    }
    setError(null);
    setErrorField(null);
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

  return (
    <Sheet
      labelledBy="export-identity-heading"
      describedBy="export-identity-body"
      onEscape={onClose}
      initialFocus={first}
    >
      <h2 id="export-identity-heading" className="bs-sheet__t">
        {t("identity.exportHeading")}
      </h2>
      {savedTo !== null ? (
        <>
          <div className="bs-sheet__b">
            <p id="export-identity-body">{t("identity.exportBody")}</p>
            <Banner kind="ok" role="status">
              {t("identity.exported", { path: savedTo })}
            </Banner>
          </div>
          <div className="bs-sheet__a">
            <button type="button" className="bs-btn bs-btn--pri" ref={close} onClick={onClose}>
              {t("identity.close")}
            </button>
          </div>
        </>
      ) : (
        <form onSubmit={save}>
          <div className="bs-sheet__b">
            <p id="export-identity-body">{t("identity.exportBody")}</p>
            <div className="bs-field">
              <label className="bs-label" htmlFor="export-identity-password">
                {t("identity.password")}
              </label>
              <input
                id="export-identity-password"
                className="bs-input"
                ref={first}
                type="password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                aria-describedby="export-identity-password-hint"
                aria-invalid={errorField === "password" ? true : undefined}
                autoComplete="new-password"
              />
              <Hint id="export-identity-password-hint">
                {t("identity.passwordHint", { min: MIN_PASSWORD_CHARS })}
              </Hint>
              {error !== null && errorField === "password" && <FieldError>{error}</FieldError>}
            </div>
            <div className="bs-field">
              <label className="bs-label" htmlFor="export-identity-confirm">
                {t("identity.confirm")}
              </label>
              <input
                id="export-identity-confirm"
                className="bs-input"
                type="password"
                value={confirmation}
                onChange={(e) => setConfirmation(e.target.value)}
                aria-invalid={errorField === "confirm" ? true : undefined}
                autoComplete="new-password"
              />
              {error !== null && errorField === "confirm" && <FieldError>{error}</FieldError>}
            </div>
            {error !== null && errorField === null && (
              <Banner kind="err" role="alert">
                {error}
              </Banner>
            )}
          </div>
          <div className="bs-sheet__a">
            <button type="submit" className="bs-btn bs-btn--pri" disabled={busy}>
              {t("identity.exportSave")}
            </button>
            <button type="button" onClick={onClose}>
              {t("identity.cancel")}
            </button>
          </div>
        </form>
      )}
    </Sheet>
  );
}
