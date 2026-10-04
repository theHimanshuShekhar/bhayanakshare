import { useState, type FormEvent } from "react";
import type { Api, Contact } from "./api";
import { contactName, sortedContacts } from "./contacts";
import { t } from "./i18n";
import { fingerprint } from "./transfers";

const reasonOf = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** The Contacts tab: each Contact with its Nickname, Device Name, Fingerprint and Auto-accept. */
export function ContactsScreen({
  api,
  contacts,
  onChanged,
  onAdd,
  onRemove,
}: {
  api: Api;
  contacts: Contact[];
  /** A Contact was changed; reload the list. */
  onChanged: () => void;
  onAdd: () => void;
  onRemove: (contact: Contact) => void;
}) {
  return (
    <section aria-labelledby="contacts-heading">
      <h2 id="contacts-heading">{t("contacts.heading")}</h2>
      <p>
        <button type="button" onClick={onAdd}>
          {t("contacts.add")}
        </button>
      </p>
      {contacts.length === 0 ? (
        <p>{t("contacts.empty")}</p>
      ) : (
        <ul className="contacts">
          {sortedContacts(contacts).map((c) => (
            <ContactRow key={c.id} api={api} contact={c} onChanged={onChanged} onRemove={onRemove} />
          ))}
        </ul>
      )}
    </section>
  );
}

function ContactRow({
  api,
  contact,
  onChanged,
  onRemove,
}: {
  api: Api;
  contact: Contact;
  onChanged: () => void;
  onRemove: (contact: Contact) => void;
}) {
  const print = fingerprint(contact.id);
  const name = contactName(contact) ?? print;
  const saved = contact.nickname ?? "";
  const [nickname, setNickname] = useState(saved);
  const [error, setError] = useState<string | null>(null);
  const nicknameId = `nickname-${contact.id}`;
  const autoId = `auto-accept-${contact.id}`;

  const change = (command: Promise<unknown>) => {
    setError(null);
    return command.then(onChanged, (e) => setError(t("contacts.failed", { reason: reasonOf(e) })));
  };

  const saveNickname = (e: FormEvent) => {
    e.preventDefault();
    return change(api.setNickname(contact.id, nickname));
  };

  return (
    <li>
      <h3>
        {name} <span className="badge">{t("contacts.badge")}</span>
      </h3>
      <dl>
        <dt>{t("contacts.deviceName")}</dt>
        <dd>{contact.device_name ?? t("contacts.deviceNameUnknown")}</dd>
        <dt>{t("contacts.fingerprint")}</dt>
        <dd>{print}</dd>
      </dl>
      <form onSubmit={saveNickname}>
        <label htmlFor={nicknameId}>{t("contacts.nickname")}</label>
        <div className="row">
          <input
            id={nicknameId}
            value={nickname}
            onChange={(e) => setNickname(e.target.value)}
            maxLength={64}
            autoComplete="off"
          />
          <button
            type="submit"
            aria-label={t("contacts.nicknameSaveLabel", { name })}
            disabled={nickname.trim() === saved}
          >
            {t("contacts.nicknameSave")}
          </button>
        </div>
      </form>
      <p>
        <input
          id={autoId}
          type="checkbox"
          checked={contact.auto_accept}
          aria-describedby={`${autoId}-hint`}
          onChange={(e) => change(api.setAutoAccept(contact.id, e.target.checked))}
        />{" "}
        <label htmlFor={autoId} className="inline">
          {t("contacts.autoAccept")}
        </label>
        <span id={`${autoId}-hint`} className="note">
          {t("contacts.autoAcceptHint")}
        </span>
      </p>
      {error !== null && <p role="alert">{error}</p>}
      <p>
        <button type="button" aria-label={t("contacts.removeLabel", { name })} onClick={() => onRemove(contact)}>
          {t("contacts.remove")}
        </button>
      </p>
    </li>
  );
}
