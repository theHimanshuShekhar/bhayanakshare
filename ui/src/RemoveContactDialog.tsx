import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import type { Api, Contact } from "./api";
import { contactName } from "./contacts";
import { t } from "./i18n";
import { fingerprint } from "./transfers";

/** Asks before a Contact is removed. Their Transfer records are kept. */
export function RemoveContactDialog({
  api,
  contact,
  onRemoved,
  onClose,
}: {
  api: Api;
  contact: Contact;
  onRemoved: () => void;
  onClose: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const keep = useRef<HTMLButtonElement>(null);
  const name = contactName(contact) ?? fingerprint(contact.id);

  useEffect(() => {
    // Hand focus back to whatever opened the dialog when it goes away. The safe answer
    // starts focused, so Enter cannot remove a Contact by accident.
    const opener = document.activeElement;
    keep.current?.focus();
    return () => {
      if (opener instanceof HTMLElement) opener.focus();
    };
  }, []);

  const remove = async () => {
    setError(null);
    setBusy(true);
    try {
      await api.removeContact(contact.id);
      onRemoved();
      onClose();
    } catch (e) {
      setError(t("contacts.failed", { reason: e instanceof Error ? e.message : String(e) }));
    } finally {
      setBusy(false);
    }
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") onClose();
  };

  return (
    <div className="overlay" onKeyDown={onKeyDown}>
      <div role="alertdialog" aria-modal="true" aria-labelledby="remove-heading" aria-describedby="remove-body" className="sheet">
        <h2 id="remove-heading">{t("removeContact.heading", { name })}</h2>
        <p id="remove-body">{t("removeContact.body")}</p>
        {error !== null && <p role="alert">{error}</p>}
        <div className="actions">
          <button type="button" onClick={remove} disabled={busy}>
            {t("removeContact.confirm")}
          </button>
          <button type="button" ref={keep} onClick={onClose}>
            {t("removeContact.cancel")}
          </button>
        </div>
      </div>
    </div>
  );
}
