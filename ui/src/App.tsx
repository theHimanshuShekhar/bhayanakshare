import { useCallback, useEffect, useReducer, useState } from "react";
import { AddContactDialog } from "./AddContactDialog";
import { tauriApi, type Api, type Contact, type TransferId } from "./api";
import { ContactsScreen } from "./ContactsScreen";
import { MyDeviceId } from "./MyDeviceId";
import { OfferSheet } from "./OfferSheet";
import { QuitDialog } from "./QuitDialog";
import { RemoveContactDialog } from "./RemoveContactDialog";
import { SendDialog } from "./SendDialog";
import { SettingsScreen } from "./SettingsScreen";
import { TransferList } from "./TransferList";
import { peerName, sortedContacts } from "./contacts";
import { t, type MessageKey } from "./i18n";
import { FIREWALL_DOCS_URL, NEARBY_WAIT_MS, applyNearby, nearbyStrangers } from "./nearby";
import { applyEvent, baseName, fingerprint, newestFirst, noTransfers, pendingOffer } from "./transfers";

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
  // A Nearby Device being saved as a Contact: its ID is already known.
  const [saving, setSaving] = useState<{ id: string; name: string | null } | null>(null);
  const [removing, setRemoving] = useState<Contact | null>(null);
  const [transfers, dispatch] = useReducer(applyEvent, noTransfers);
  const [nearby, dispatchNearby] = useReducer(applyNearby, []);
  // Set once Home has waited long enough for a Nearby Device to show up.
  const [waited, setWaited] = useState(false);
  const [saveFolder, setSaveFolder] = useState<string | null>(null);
  // Files waiting for the user to say whom to send them to (from a second launch or the tray).
  const [queued, setQueued] = useState<string[]>([]);
  // The Offer a notification was clicked for; it is shown ahead of older ones.
  const [preferred, setPreferred] = useState<TransferId | undefined>();
  // Quit was chosen while Transfers are in progress.
  const [quit, setQuit] = useState<{ active: number; saving: boolean } | null>(null);
  const current = TABS.find((x) => x.id === tab) ?? TABS[0];
  const offer = pendingOffer(transfers, preferred);
  // While a sheet is open the page behind it can be neither clicked nor tabbed to.
  const inert =
    sending !== null ||
    adding ||
    saving !== null ||
    removing !== null ||
    offer !== undefined ||
    quit !== null;

  const loadContacts = useCallback(() => api.contacts().then(setContacts, () => {}), [api]);

  useEffect(() => {
    let live = true;
    let unlisten: (() => void) | undefined;
    api
      .onDeviceEvent((event) => {
        dispatch(event);
        dispatchNearby(event);
      })
      .then((stop) => (live ? (unlisten = stop) : stop()));
    api.saveFolder().then((folder) => live && setSaveFolder(folder), () => {});
    return () => {
      live = false;
      unlisten?.();
    };
  }, [api]);

  useEffect(() => {
    let live = true;
    let unlisten: (() => void) | undefined;
    api
      .onShellEvent((event) => {
        switch (event.type) {
          case "send_files":
            setQueued(event.paths);
            setTab("home");
            break;
          case "open_offer":
            setPreferred(event.transfer_id);
            break;
          case "confirm_quit":
            setQuit({ active: event.active, saving: false });
            break;
          case "quitting":
            setQuit((q) => ({ active: q?.active ?? 0, saving: true }));
            break;
        }
      })
      .then((stop) => (live ? (unlisten = stop) : stop()));
    return () => {
      live = false;
      unlisten?.();
    };
  }, [api]);

  useEffect(() => {
    const timer = setTimeout(() => setWaited(true), NEARBY_WAIT_MS);
    return () => clearTimeout(timer);
  }, []);

  // A connection can refresh a Contact's Device Name or address behind the UI's back, so look
  // again whenever a tab is opened, a Transfer begins or learns the other Device's name, or the
  // Nearby Devices change.
  const named = Object.values(transfers.byId).filter((x) => x.peerName !== null).length;
  useEffect(() => {
    loadContacts();
  }, [loadContacts, tab, transfers.order.length, named, nearby]);

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
        {tab !== "contacts" && tab !== "settings" && <p>{t(current.placeholder)}</p>}
        {tab === "home" && (
          <>
            <MyDeviceId api={api} />
            {queued.length > 0 && (
              <p role="status">
                {t("queued.banner", { names: queued.map(baseName).join(", ") })}{" "}
                <button type="button" onClick={() => setQueued([])}>
                  {t("queued.clear")}
                </button>
              </p>
            )}
            <section aria-labelledby="devices-heading">
              <h2 id="devices-heading">{t("home.devices")}</h2>
              <div className="tiles">
                {sortedContacts(contacts).map((c) => {
                  // A Contact that is Nearby shows what it announces until it has a name here.
                  const here = nearby.find((d) => d.id === c.id);
                  const name = peerName(c.id, contacts, here?.name ?? null);
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
                      {here && <span className="note">{t("home.nearby")}</span>}
                    </button>
                  );
                })}
                {nearbyStrangers(nearby, contacts).map((d) => {
                  const label = peerName(d.id, contacts, d.name);
                  return (
                    <div key={d.id} className="tile-group">
                      <button
                        type="button"
                        className="tile"
                        aria-label={t("home.sendToNearby", { name: label })}
                        onClick={() => setSending({ to: d.id, name: label })}
                      >
                        <strong>{d.name ?? fingerprint(d.id)}</strong>
                        {d.name !== null && <span className="note">{fingerprint(d.id)}</span>}
                        <span className="note">{t("home.nearby")}</span>
                      </button>
                      <button
                        type="button"
                        aria-label={t("home.saveAsContactLabel", { name: label })}
                        onClick={() => setSaving({ id: d.id, name: d.name })}
                      >
                        {t("home.saveAsContact")}
                      </button>
                    </div>
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
              {waited && nearby.length === 0 && (
                <p role="status" className="hint">
                  {t("home.firewallHint")}{" "}
                  <a
                    href={FIREWALL_DOCS_URL}
                    onClick={(e) => {
                      // The webview must not navigate away from the app.
                      e.preventDefault();
                      api.openUrl(FIREWALL_DOCS_URL).catch(() => {});
                    }}
                  >
                    {t("home.firewallDocs")}
                  </a>
                </p>
              )}
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
        {tab === "settings" && <SettingsScreen api={api} />}
      </main>
      {sending && (
        <SendDialog
          key={queued.join("\n")}
          api={api}
          to={sending.to}
          contactName={sending.name}
          files={queued}
          onSent={() => setQueued([])}
          onClose={() => setSending(null)}
        />
      )}
      {adding && (
        <AddContactDialog api={api} onAdded={loadContacts} onClose={() => setAdding(false)} />
      )}
      {saving && (
        <AddContactDialog
          api={api}
          prefilled={saving}
          onAdded={loadContacts}
          onClose={() => setSaving(null)}
        />
      )}
      {removing && (
        <RemoveContactDialog
          api={api}
          contact={removing}
          onRemoved={loadContacts}
          onClose={() => setRemoving(null)}
        />
      )}
      {quit && (
        <QuitDialog
          api={api}
          active={quit.active}
          saving={quit.saving}
          onSaving={(saving) => setQuit({ active: quit.active, saving })}
          onCancel={() => setQuit(null)}
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
