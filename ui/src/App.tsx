import { useCallback, useEffect, useReducer, useState } from "react";
import { AddContactDialog } from "./AddContactDialog";
import { tauriApi, type Api, type Contact } from "./api";
import { ContactsScreen } from "./ContactsScreen";
import { MyDeviceId } from "./MyDeviceId";
import { OfferSheet } from "./OfferSheet";
import { RemoveContactDialog } from "./RemoveContactDialog";
import { SendDialog } from "./SendDialog";
import { TransferList } from "./TransferList";
import { contactName, sortedContacts } from "./contacts";
import { t, type MessageKey } from "./i18n";
import { applyEvent, fingerprint, newestFirst, noTransfers, pendingOffer } from "./transfers";

const TABS = [
  { id: "home", label: "tab.home", placeholder: "home.placeholder" },
  { id: "history", label: "tab.history", placeholder: "history.placeholder" },
  { id: "contacts", label: "tab.contacts", placeholder: "contacts.placeholder" },
  { id: "settings", label: "tab.settings", placeholder: "settings.placeholder" },
] as const satisfies readonly { id: string; label: MessageKey; placeholder: MessageKey }[];

type TabId = (typeof TABS)[number]["id"];

interface AppProps {
  /** Where commands go and events come from; the Rust shell by default, a stub in tests. */
  api?: Api;
}

export function App({ api = tauriApi }: AppProps) {
  const [tab, setTab] = useState<TabId>("home");
  // Sending to a pasted ID (`to` empty) or to a Contact, whose ID is filled in.
  const [sending, setSending] = useState<{ to: string; name: string | null } | null>(null);
  const [contacts, setContacts] = useState<Contact[]>([]);
  const [adding, setAdding] = useState(false);
  const [removing, setRemoving] = useState<Contact | null>(null);
  const [transfers, dispatch] = useReducer(applyEvent, noTransfers);
  const [saveFolder, setSaveFolder] = useState<string | null>(null);
  const current = TABS.find((x) => x.id === tab) ?? TABS[0];
  const offer = pendingOffer(transfers);
  // While a sheet is open the page behind it can be neither clicked nor tabbed to.
  const inert = sending !== null || adding || removing !== null || offer !== undefined;

  const loadContacts = useCallback(() => api.contacts().then(setContacts, () => {}), [api]);

  useEffect(() => {
    let live = true;
    let unlisten: (() => void) | undefined;
    api.onDeviceEvent(dispatch).then((stop) => (live ? (unlisten = stop) : stop()));
    api.saveFolder().then((folder) => live && setSaveFolder(folder), () => {});
    return () => {
      live = false;
      unlisten?.();
    };
  }, [api]);

  // A connection can refresh a Contact's Device Name or address behind the UI's back, so look
  // again whenever a tab is opened or a Transfer begins or learns the other Device's name.
  const named = Object.values(transfers.byId).filter((x) => x.peerName !== null).length;
  useEffect(() => {
    loadContacts();
  }, [loadContacts, tab, transfers.order.length, named]);

  return (
    <div className="app">
      <header inert={inert}>
        <h1>{t("app.name")}</h1>
        <nav aria-label={t("nav.label")}>
          {TABS.map((x) => (
            <button
              key={x.id}
              type="button"
              aria-current={x.id === tab ? "page" : undefined}
              onClick={() => setTab(x.id)}
            >
              {t(x.label)}
            </button>
          ))}
        </nav>
      </header>
      <main inert={inert}>
        {tab !== "contacts" && <p>{t(current.placeholder)}</p>}
        {tab === "home" && (
          <>
            <MyDeviceId api={api} />
            <section aria-labelledby="devices-heading">
              <h2 id="devices-heading">{t("home.devices")}</h2>
              <div className="tiles">
                {sortedContacts(contacts).map((c) => {
                  const name = contactName(c) ?? fingerprint(c.id);
                  return (
                    <button
                      key={c.id}
                      type="button"
                      className="tile"
                      aria-label={t("home.sendToContact", { name })}
                      onClick={() => setSending({ to: c.id, name })}
                    >
                      <strong>{name}</strong>
                      <span className="badge">{t("contacts.badge")}</span>
                      <span className="note">{fingerprint(c.id)}</span>
                    </button>
                  );
                })}
                <button
                  type="button"
                  className="tile"
                  onClick={() => setSending({ to: "", name: null })}
                >
                  {t("home.sendToId")}
                </button>
              </div>
            </section>
            <section aria-labelledby="transfers-heading">
              <h2 id="transfers-heading">{t("home.transfers")}</h2>
              {transfers.order.length === 0 ? (
                <p>{t("home.noTransfers")}</p>
              ) : (
                <TransferList api={api} contacts={contacts} transfers={newestFirst(transfers)} />
              )}
            </section>
          </>
        )}
        {tab === "contacts" && (
          <ContactsScreen
            api={api}
            contacts={contacts}
            onChanged={loadContacts}
            onAdd={() => setAdding(true)}
            onRemove={setRemoving}
          />
        )}
      </main>
      {sending && (
        <SendDialog
          api={api}
          to={sending.to}
          contactName={sending.name}
          onClose={() => setSending(null)}
        />
      )}
      {adding && (
        <AddContactDialog api={api} onAdded={loadContacts} onClose={() => setAdding(false)} />
      )}
      {removing && (
        <RemoveContactDialog
          api={api}
          contact={removing}
          onRemoved={loadContacts}
          onClose={() => setRemoving(null)}
        />
      )}
      {offer && (
        <OfferSheet
          key={offer.id}
          api={api}
          offer={offer}
          contacts={contacts}
          saveFolder={saveFolder}
        />
      )}
    </div>
  );
}
