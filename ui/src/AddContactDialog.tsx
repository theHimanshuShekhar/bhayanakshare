import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import type { Api, Contact } from "./api";
import { isDeviceId } from "./contacts";
import { t } from "./i18n";
import { fingerprint } from "./transfers";

/**
 * "Add Contact…": paste a Device ID (and a name, if known), then check the Fingerprint with
 * the owner before it is saved. A Device ID pasted from anywhere could be someone else's.
 */
export function AddContactDialog({
  api,
  onAdded,
  onClose,
}: {
  api: Api;
  onAdded: (contact: Contact) => void;
  onClose: () => void;
}) {
  const [id, setId] = useState("");
  const [name, setName] = useState("");
  const [checking, setChecking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const idInput = useRef<HTMLInputElement>(null);
  const shownFingerprint = useRef<HTMLElement>(null);
  const trimmed = id.trim();
  const valid = isDeviceId(trimmed);

  useEffect(() => {
    // Hand focus back to whatever opened the dialog when it goes away.
    const opener = document.activeElement;
    return () => {
      if (opener instanceof HTMLElement) opener.focus();
    };
  }, []);

  // Each step starts with focus on its first control: the ID field, then the Fingerprint.
  useEffect(() => {
    (checking ? shownFingerprint : idInput).current?.focus();
  }, [checking]);

  const add = async () => {
    setError(null);
    setBusy(true);
    try {
      const contact = await api.addContact(trimmed, name.trim() === "" ? null : name.trim());
      onAdded(contact);
      onClose();
    } catch (e) {
      setError(t("addContact.failed", { reason: e instanceof Error ? e.message : String(e) }));
    } finally {
      setBusy(false);
    }
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") onClose();
  };

  return (
    <div className="overlay" onKeyDown={onKeyDown}>
      <div role="dialog" aria-modal="true" aria-labelledby="add-contact-heading" className="sheet">
        <h2 id="add-contact-heading">
          {checking ? t("addContact.checkHeading") : t("addContact.heading")}
        </h2>
        {checking ? (
          <>
            <p>{t("addContact.check")}</p>
            <p className="fingerprint">
              <strong ref={shownFingerprint} tabIndex={-1}>
                {fingerprint(trimmed.toUpperCase())}
              </strong>
            </p>
            {error !== null && <p role="alert">{error}</p>}
            <div className="actions">
              <button type="button" onClick={add} disabled={busy}>
                {t("addContact.confirm")}
              </button>
              <button type="button" onClick={() => setChecking(false)} disabled={busy}>
                {t("addContact.back")}
              </button>
            </div>
          </>
        ) : (
          <>
            <label htmlFor="add-contact-id">{t("addContact.idLabel")}</label>
            <input
              id="add-contact-id"
              ref={idInput}
              value={id}
              onChange={(e) => setId(e.target.value)}
              aria-describedby="add-contact-id-hint"
              aria-invalid={trimmed !== "" && !valid}
              spellCheck={false}
              autoComplete="off"
            />
            <p id="add-contact-id-hint" className="note">
              {t("addContact.idHint")}
            </p>
            {trimmed !== "" && !valid && <p role="alert">{t("addContact.idInvalid")}</p>}
            <label htmlFor="add-contact-name">{t("addContact.nameLabel")}</label>
            <input
              id="add-contact-name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              aria-describedby="add-contact-name-hint"
              maxLength={64}
              autoComplete="off"
            />
            <p id="add-contact-name-hint" className="note">
              {t("addContact.nameHint")}
            </p>
            <div className="actions">
              <button type="button" onClick={() => setChecking(true)} disabled={!valid}>
                {t("addContact.next")}
              </button>
              <button type="button" onClick={onClose}>
                {t("addContact.cancel")}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
