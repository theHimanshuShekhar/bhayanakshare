import { useState, type FormEvent } from "react";
import type { Api, Contact } from "./api";
import { Hint } from "./Banner";
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
  onShowHistory,
}: {
  api: Api;
  contacts: Contact[];
  /** A Contact was changed; reload the list. */
  onChanged: () => void;
  onAdd: () => void;
  onRemove: (contact: Contact) => void;
  /** Open the Transfer History with a Contact. */
  onShowHistory: (contact: Contact) => void;
}) {
  return (
    <section aria-labelledby="contacts-heading" className="sect">
      <div className="pagehead">
        <h2 id="contacts-heading" className="bs-h1">
          {t("contacts.heading")}
        </h2>
        <button type="button" className="bs-btn--pri" onClick={onAdd}>
          {t("contacts.add")}
        </button>
      </div>
      {contacts.length === 0 ? (
        <p className="empty">{t("contacts.empty")}</p>
      ) : (
        <ul className="list">
          {sortedContacts(contacts).map((c) => (
            <ContactRow
              key={c.id}
              api={api}
              contact={c}
              onChanged={onChanged}
              onRemove={onRemove}
              onShowHistory={onShowHistory}
            />
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
  onShowHistory,
}: {
  api: Api;
  contact: Contact;
  onChanged: () => void;
  onRemove: (contact: Contact) => void;
  onShowHistory: (contact: Contact) => void;
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
    <li className="card cc">
      <div className="cc__h">
        <span className="bs-av" aria-hidden="true">
          {Array.from(name)[0]?.toUpperCase()}
        </span>
        <h3 className="bs-h3">{name}</h3>
        <span className="bs-badge">{t("contacts.badge")}</span>
      </div>
      <dl className="cc__dl">
        <dt>{t("contacts.deviceName")}</dt>
        <dd>{contact.device_name ?? t("contacts.deviceNameUnknown")}</dd>
        <dt>{t("contacts.fingerprint")}</dt>
        <dd className="bs-mono">{print}</dd>
      </dl>
      <form className="cc__nick" onSubmit={saveNickname}>
        <div className="bs-field">
          <label className="bs-label" htmlFor={nicknameId}>
            {t("contacts.nickname")}
          </label>
          <input
            id={nicknameId}
            className="bs-input"
            value={nickname}
            onChange={(e) => setNickname(e.target.value)}
            maxLength={64}
            autoComplete="off"
          />
        </div>
        <button
          type="submit"
          aria-label={t("contacts.nicknameSaveLabel", { name })}
          disabled={nickname.trim() === saved}
        >
          {t("contacts.nicknameSave")}
        </button>
      </form>
      <div className="srow srow--split">
        <div className="bs-opt__t">
          <label className="bs-label" htmlFor={autoId}>
            {t("contacts.autoAccept")}
          </label>
          <span id={`${autoId}-hint`} className="bs-hint">
            {t("contacts.autoAcceptHint")}
          </span>
        </div>
        <input
          id={autoId}
          className="bs-switch"
          type="checkbox"
          checked={contact.auto_accept}
          aria-describedby={`${autoId}-hint`}
          onChange={(e) => change(api.setAutoAccept(contact.id, e.target.checked))}
        />
      </div>
      {error !== null && (
        <Hint icon="err" tone="err" role="alert">
          {error}
        </Hint>
      )}
      <div className="cc__f">
        <button type="button" aria-label={t("contacts.historyLabel", { name })} onClick={() => onShowHistory(contact)}>
          {t("contacts.history")}
        </button>
        <button type="button" aria-label={t("contacts.removeLabel", { name })} onClick={() => onRemove(contact)}>
          {t("contacts.remove")}
        </button>
      </div>
    </li>
  );
}
