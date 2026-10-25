import { useEffect, useRef, useState, type FormEvent } from "react";
import type { Api, IdentityOwner, MyId } from "./api";
import { Banner, FieldError } from "./Banner";
import { t } from "./i18n";
import { Sheet } from "./Sheet";
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
  const [incoming, setIncoming] = useState<IdentityOwner | null>(null);
  // How many Transfers the replacement stops; read with the file's owner, for the warning.
  const [active, setActive] = useState(0);
  const [restarting, setRestarting] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const passwordInput = useRef<HTMLInputElement>(null);
  const keep = useRef<HTMLButtonElement>(null);
  const close = useRef<HTMLButtonElement>(null);
  const same = incoming !== null && incoming.id === current.id;
  const confirming = incoming !== null && !same;

  // Each step starts with focus on its first control: the password, then the safe answer, or
  // Close when the file holds this Device's own identity.
  useEffect(() => {
    (confirming ? keep : same ? close : passwordInput).current?.focus();
  }, [confirming, same]);

  const check = async (e: FormEvent) => {
    e.preventDefault();
    setError(null);
    setBusy(true);
    try {
      const owner = await api.checkIdentityImport(path, password);
      // Only a count for the warning: not knowing it must not stop the import.
      setActive(await api.transfersInProgress().catch(() => 0));
      setIncoming(owner);
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

  return (
    <Sheet
      role={confirming ? "alertdialog" : "dialog"}
      labelledBy="import-identity-heading"
      describedBy={confirming ? "import-identity-body" : undefined}
      onEscape={restarting ? undefined : onClose}
      initialFocus={passwordInput}
    >
      <h2 id="import-identity-heading" className="bs-sheet__t">
        {confirming ? t("identity.replaceHeading") : t("identity.importHeading")}
      </h2>
      {confirming ? (
        <>
          <div className="bs-sheet__b">
            <div id="import-identity-body" className="stack">
              <p>
                {t("identity.replaceId", { current: current.fingerprint, incoming: incoming.fingerprint })}
              </p>
              <Banner kind="warn">{t("identity.replaceOld")}</Banner>
              {active > 0 && (
                <Banner kind="warn">
                  {active === 1 ? t("identity.replaceActiveOne") : t("identity.replaceActive", { count: active })}
                </Banner>
              )}
              <p>{t("identity.replaceRestart")}</p>
            </div>
            {error !== null && (
              <Banner kind="err" role="alert">
                {error}
              </Banner>
            )}
            {restarting && (
              <Banner kind="info" role="status">
                {t("identity.restarting")}
              </Banner>
            )}
          </div>
          <div className="bs-sheet__a">
            <button
              type="button"
              className="bs-btn bs-btn--danger"
              onClick={replace}
              disabled={busy || restarting}
            >
              {t("identity.replaceConfirm")}
            </button>
            <button type="button" ref={keep} onClick={onClose} disabled={restarting}>
              {t("identity.cancel")}
            </button>
          </div>
        </>
      ) : same ? (
        <>
          <div className="bs-sheet__b">
            <Banner kind="info" role="status">
              {t("identity.importSame", { fingerprint: current.fingerprint })}
            </Banner>
          </div>
          <div className="bs-sheet__a">
            <button type="button" className="bs-btn bs-btn--pri" ref={close} onClick={onClose}>
              {t("identity.close")}
            </button>
          </div>
        </>
      ) : (
        <form onSubmit={check}>
          <div className="bs-sheet__b">
            <p>{t("identity.importFile", { name: baseName(path) })}</p>
            <div className="bs-field">
              <label className="bs-label" htmlFor="import-identity-password">
                {t("identity.importPassword")}
              </label>
              <input
                id="import-identity-password"
                className="bs-input"
                ref={passwordInput}
                type="password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                aria-invalid={error !== null ? true : undefined}
                autoComplete="current-password"
              />
              {error !== null && <FieldError>{error}</FieldError>}
            </div>
          </div>
          <div className="bs-sheet__a">
            <button type="submit" className="bs-btn bs-btn--pri" disabled={busy || password === ""}>
              {t("identity.importNext")}
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
