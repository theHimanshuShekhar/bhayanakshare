import { useRef, useState } from "react";
import type { Api, Contact } from "./api";
import { Banner } from "./Banner";
import { contactName } from "./contacts";
import { t } from "./i18n";
import { Sheet } from "./Sheet";
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

  return (
    <Sheet
      role="alertdialog"
      labelledBy="remove-heading"
      describedBy="remove-body"
      onEscape={onClose}
      // The safe answer starts focused, so Enter cannot remove a Contact by accident.
      initialFocus={keep}
    >
      <h2 id="remove-heading" className="bs-sheet__t">
        {t("removeContact.heading", { name })}
      </h2>
      <div className="bs-sheet__b">
        <p id="remove-body">{t("removeContact.body")}</p>
        {error !== null && (
          <Banner kind="err" role="alert">
            {error}
          </Banner>
        )}
      </div>
      <div className="bs-sheet__a">
        <button type="button" className="bs-btn" ref={keep} onClick={onClose}>
          {t("removeContact.cancel")}
        </button>
        <button type="button" className="bs-btn bs-btn--danger" onClick={remove} disabled={busy}>
          {t("removeContact.confirm")}
        </button>
      </div>
    </Sheet>
  );
}
