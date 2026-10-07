import { useCallback, useEffect, useReducer, useRef, useState } from "react";
import { AddContactDialog } from "./AddContactDialog";
import { Announcer, TransferAnnouncements } from "./Announcer";
import { tauriApi, type Api, type Contact, type TransferId, type UpdateAction, type Visibility } from "./api";
import { ClearHistoryDialog } from "./ClearHistoryDialog";
import { ContactsScreen } from "./ContactsScreen";
import { FirstRunScreen } from "./FirstRunScreen";
import { useFocusRescue } from "./focusRescue";
import { HistoryScreen } from "./HistoryScreen";
import { MyDeviceId } from "./MyDeviceId";
import { OfferSheet } from "./OfferSheet";
import { QuitDialog } from "./QuitDialog";
import { RemoveContactDialog } from "./RemoveContactDialog";
import { SelectBox, SelectionBar } from "./SelectionBar";
import { SendDialog } from "./SendDialog";
import { SettingsScreen } from "./SettingsScreen";
import { TransferList } from "./TransferList";
import { UpdateBanner, UpdateStatus } from "./UpdateBanner";
import { VersionNotices } from "./VersionNotices";
import { peerName, sortedContacts } from "./contacts";
import { t, type MessageKey } from "./i18n";
import { FIREWALL_DOCS_URL, NEARBY_WAIT_MS, applyNearby, nearbyStrangers } from "./nearby";
import { parseShareLink, type Shared } from "./shareLink";
import { applyEvent, baseName, fingerprint, listItems, noTransfers, pendingOffer } from "./transfers";
import { offeredVersion, useUpdater } from "./updates";
import { applyVersionNotices } from "./versions";

/** Gives focus to a heading that is there for it (`tabIndex={-1}`). */
const focusHeading = (id: string) => document.getElementById(id)?.focus();

const TABS = [
  { id: "home", label: "tab.home", placeholder: "home.placeholder" },
  { id: "history", label: "tab.history", placeholder: "history.placeholder" },
  { id: "contacts", label: "tab.contacts", placeholder: "contacts.placeholder" },
  { id: "settings", label: "tab.settings", placeholder: "settings.placeholder" },
] as const satisfies readonly { id: string; label: MessageKey; placeholder: MessageKey }[];

type TabId = (typeof TABS)[number]["id"];

/** A link the user opened, as the Add Contact dialog it leads to: `suggested` is null for a link
 * that is not a share link. `stamp` tells one opening from another. */
interface Opened {
  suggested: Shared | null;
  stamp: number;
}

interface AppProps {
  /** Where commands go and events come from; the Rust shell by default, a stub in tests. */
  api?: Api;
}

export function App({ api = tauriApi }: AppProps) {
  return (
    <Announcer>
      <Screens api={api} />
    </Announcer>
  );
}

function Screens({ api }: { api: Api }) {
  useFocusRescue();
  const [tab, setTab] = useState<TabId>("home");
  // Sending to a pasted ID (`to` empty) or to a Contact, whose ID is filled in.
  const [sending, setSending] = useState<{ to: string; name: string | null } | null>(null);
  const [contacts, setContacts] = useState<Contact[]>([]);
  // Device IDs ticked on Home, to send the same files to all of them as a Batch.
  const [selected, setSelected] = useState<string[]>([]);
  const [adding, setAdding] = useState(false);
  // A Nearby Device being saved as a Contact: its ID is already known.
  const [saving, setSaving] = useState<{ id: string; name: string | null } | null>(null);
  // An opened link waits in `pendingLink` while another dialog or sheet is open (the newest
  // replaces an older one), and moves to `linked` once there is none.
  const [pendingLink, setPendingLink] = useState<Opened | null>(null);
  const [linked, setLinked] = useState<Opened | null>(null);
  const linkCount = useRef(0);
  const [removing, setRemoving] = useState<Contact | null>(null);
  // History is narrowed to this Device (from a Contact's History link, or the filter).
  const [historyDevice, setHistoryDevice] = useState<string | null>(null);
  const [clearing, setClearing] = useState(false);
  // Counts how often History was cleared, so the History tab reads it again.
  const [cleared, setCleared] = useState(0);
  const [transfers, dispatch] = useReducer(applyEvent, noTransfers);
  const [nearby, dispatchNearby] = useReducer(applyNearby, []);
  // Devices that were refused for their version, until the user dismisses the notice.
  const [versionNotices, dispatchVersion] = useReducer(applyVersionNotices, []);
  // A newer release the shell found, and the version of it the user dismissed.
  const [update, setUpdate] = useState<UpdateAction>({ type: "none" });
  const [dismissedUpdate, setDismissedUpdate] = useState<string | null>(null);
  // Looking for and installing an update: one at a time, whatever started it.
  const updater = useUpdater(api);
  // Set once Home has waited long enough for a Nearby Device to show up.
  const [waited, setWaited] = useState(false);
  // A Hidden Device lists nobody Nearby, so an empty list there is no sign of a firewall.
  const [visibility, setVisibility] = useState<Visibility | null>(null);
  // Whether first run is still to be done; null until the Device has said. Until it is done its
  // screen is all there is.
  const [firstRunNeeded, setFirstRunNeeded] = useState<boolean | null>(null);
  // Files waiting for the user to say whom to send them to (from a second launch or the tray).
  const [queued, setQueued] = useState<string[]>([]);
  // The Offer a notification was clicked for; it is shown ahead of older ones.
  const [preferred, setPreferred] = useState<TransferId | undefined>();
  // Quit was chosen while Transfers are in progress.
  const [quit, setQuit] = useState<{ active: number; saving: boolean } | null>(null);
  const current = TABS.find((x) => x.id === tab) ?? TABS[0];
  const offer = pendingOffer(transfers, preferred);
  // The tab on show (none while first run is, or may be, still to do), which the tab bar and the
  // tab's own content follow.
  const shownTab = firstRunNeeded === false ? tab : null;
  // Add Contact (and Save as Contact) give way to a link; these do not.
  const busy = sending !== null || removing !== null || clearing || offer !== undefined || quit !== null;
  // While a sheet is open the page behind it can be neither clicked nor tabbed to.
  const inert = busy || adding || saving !== null || linked !== null;

  // A check made in Settings: what it finds is noted, so the banner offers it too.
  const { check } = updater;
  const checkForUpdates = useCallback(async () => {
    const found = await check();
    if (found !== null) setUpdate(found);
  }, [check]);

  const loadContacts = useCallback(() => api.contacts().then(setContacts, () => {}), [api]);

  const strangers = nearbyStrangers(nearby, contacts);
  // A Device that has gone from Home cannot stay chosen: there is no tile to untick.
  const shown = new Set([...contacts.map((c) => c.id), ...strangers.map((d) => d.id)]);
  const chosen = selected.filter((id) => shown.has(id));
  const toggle = (id: string) =>
    setSelected((now) => (now.includes(id) ? now.filter((x) => x !== id) : [...now, id]));

  useEffect(() => {
    let live = true;
    let unlisten: (() => void) | undefined;
    api
      .onDeviceEvent((event) => {
        dispatch(event);
        dispatchNearby(event);
        dispatchVersion(event);
      })
      .then((stop) => (live ? (unlisten = stop) : stop()));
    return () => {
      live = false;
      unlisten?.();
    };
  }, [api]);

  useEffect(() => {
    let live = true;
    // If the Device cannot say, the user is not held up by a screen about settings.
    api.needsFirstRun().then(
      (needed) => live && setFirstRunNeeded(needed),
      () => live && setFirstRunNeeded(false),
    );
    return () => {
      live = false;
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
          case "update_available":
            setUpdate(event.action);
            break;
        }
      })
      .then((stop) => (live ? (unlisten = stop) : stop()));
    // The check at startup may have finished before this was listening.
    api.pendingUpdate().then((found) => live && setUpdate(found), () => {});
    return () => {
      live = false;
      unlisten?.();
    };
  }, [api]);

  useEffect(() => {
    let live = true;
    let unlisten: (() => void) | undefined;
    api
      .onOpenLink((url) => {
        // A link that is not a share link still opens the dialog, to say what is wrong with it.
        setPendingLink({ suggested: parseShareLink(url), stamp: ++linkCount.current });
      })
      .then((stop) => (live ? (unlisten = stop) : stop()));
    return () => {
      live = false;
      unlisten?.();
    };
  }, [api]);

  useEffect(() => {
    if (pendingLink === null || busy) return;
    setAdding(false);
    setSaving(null);
    setLinked(pendingLink);
    setPendingLink(null);
  }, [pendingLink, busy]);

  useEffect(() => {
    const timer = setTimeout(() => setWaited(true), NEARBY_WAIT_MS);
    return () => clearTimeout(timer);
  }, []);

  // The setting is changed in Settings, the tray or first run, so look again whenever a tab is
  // opened, first run is done and when the window is back in front.
  useEffect(() => {
    let live = true;
    const load = () =>
      api.visibility().then(
        (v) => live && setVisibility(v),
        () => {},
      );
    load();
    window.addEventListener("focus", load);
    return () => {
      live = false;
      window.removeEventListener("focus", load);
    };
  }, [api, tab, firstRunNeeded]);

  // A connection can refresh a Contact's Device Name or address behind the UI's back, so look
  // again whenever a tab is opened, a Transfer begins or learns the other Device's name, or the
  // Nearby Devices change.
  const named = Object.values(transfers.byId).filter((x) => x.peerName !== null).length;
  useEffect(() => {
    loadContacts();
  }, [loadContacts, tab, transfers.order.length, named, nearby]);

  return (
    <div className="app">
      <TransferAnnouncements transfers={transfers} contacts={contacts} />
      <header inert={inert}>
        <h1>{t("app.name")}</h1>
        {shownTab !== null && (
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
        )}
      </header>
      {shownTab !== null && (
        <div inert={inert}>
          {/* Settings has the banner's place in its Updates section: one notice, one status. */}
          {shownTab !== "settings" && offeredVersion(update) !== dismissedUpdate && (
            <UpdateBanner
              api={api}
              update={update}
              busy={updater.busy}
              onInstall={updater.install}
              onDismiss={() => setDismissedUpdate(offeredVersion(update))}
            />
          )}
          <VersionNotices
            contacts={contacts}
            notices={versionNotices}
            updating={updater.busy}
            onUpdateNow={updater.updateNow}
            onDismiss={(peer) => dispatchVersion({ type: "dismiss_version_notice", peer })}
          />
          {shownTab !== "settings" && <UpdateStatus updater={updater} />}
        </div>
      )}
      <main inert={inert}>
        {firstRunNeeded === true && <FirstRunScreen api={api} onDone={() => setFirstRunNeeded(false)} />}
        {shownTab === "home" && <p>{t(current.placeholder)}</p>}
        {shownTab === "home" && (
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
                  const picked = chosen.includes(c.id);
                  return (
                    <div key={c.id} className={picked ? "tile-group selected" : "tile-group"}>
                      <button
                        type="button"
                        className="tile"
                        aria-label={t("home.sendToContact", { name })}
                        aria-describedby={`tile-${c.id}-state`}
                        onClick={() => setSending({ to: c.id, name })}
                      >
                        <strong>{name}</strong>
                        {/* What the tile says besides the name, which the name label leaves out. */}
                        <span id={`tile-${c.id}-state`} className="tile-state">
                          <span className="badge">{t("contacts.badge")}</span>
                          <span className="note">{fingerprint(c.id)}</span>
                          {here && <span className="note">{t("home.nearby")}</span>}
                          {picked && <span className="note">{t("selection.selected")}</span>}
                        </span>
                      </button>
                      <SelectBox name={name} checked={picked} onChange={() => toggle(c.id)} />
                    </div>
                  );
                })}
                {strangers.map((d) => {
                  const label = peerName(d.id, contacts, d.name);
                  const picked = chosen.includes(d.id);
                  return (
                    <div key={d.id} className={picked ? "tile-group selected" : "tile-group"}>
                      <button
                        type="button"
                        className="tile"
                        aria-label={t("home.sendToNearby", { name: label })}
                        aria-describedby={`tile-${d.id}-state`}
                        onClick={() => setSending({ to: d.id, name: label })}
                      >
                        <strong>{d.name ?? fingerprint(d.id)}</strong>
                        <span id={`tile-${d.id}-state`} className="tile-state">
                          {d.name !== null && <span className="note">{fingerprint(d.id)}</span>}
                          <span className="note">{t("home.nearby")}</span>
                          {picked && <span className="note">{t("selection.selected")}</span>}
                        </span>
                      </button>
                      <button
                        type="button"
                        aria-label={t("home.saveAsContactLabel", { name: label })}
                        onClick={() => setSaving({ id: d.id, name: d.name })}
                      >
                        {t("home.saveAsContact")}
                      </button>
                      <SelectBox name={label} checked={picked} onChange={() => toggle(d.id)} />
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
              {chosen.length > 0 && (
                <SelectionBar
                  api={api}
                  ids={chosen}
                  files={queued}
                  onClear={() => setSelected([])}
                  // The bar goes with the selection and the button pressed with it: what was
                  // sent is what is looked at next, so that is where focus goes.
                  onSent={() => {
                    focusHeading("transfers-heading");
                    setSelected([]);
                  }}
                  onFilesSent={() => setQueued([])}
                />
              )}
              {visibility === "hidden" && nearby.length === 0 && (
                // Not a live region: it is a state that is there on arriving, not news.
                <p className="hint">
                  {t("home.hiddenHint")}{" "}
                  <button type="button" onClick={() => setTab("settings")}>
                    {t("home.hiddenChange")}
                  </button>
                </p>
              )}
              {visibility !== "hidden" && waited && nearby.length === 0 && (
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
              <h2 id="transfers-heading" tabIndex={-1}>
                {t("home.transfers")}
              </h2>
              {transfers.order.length === 0 ? (
                <p>{t("home.noTransfers")}</p>
              ) : (
                <TransferList api={api} contacts={contacts} items={listItems(transfers)} />
              )}
            </section>
          </>
        )}
        {shownTab === "contacts" && (
          <ContactsScreen
            api={api}
            contacts={contacts}
            onChanged={loadContacts}
            onAdd={() => setAdding(true)}
            onRemove={setRemoving}
            onShowHistory={(c) => {
              setHistoryDevice(c.id);
              setTab("history");
            }}
          />
        )}
        {shownTab === "history" && (
          <HistoryScreen
            api={api}
            contacts={contacts}
            device={historyDevice}
            onDevice={setHistoryDevice}
            // History lists what is going on too: read it again as Transfers change state.
            stamp={Object.values(transfers.byId)
              .map((x) => `${x.id}:${x.state.kind}`)
              .join()}
            version={cleared}
            onClear={() => setClearing(true)}
          />
        )}
        {shownTab === "settings" && (
          <SettingsScreen
            api={api}
            updater={updater}
            update={update}
            onCheckForUpdates={checkForUpdates}
          />
        )}
      </main>
      {/* An Offer sheet goes over whatever else is open, and the sheets under it must not be reached. */}
      <div inert={offer !== undefined}>
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
        {linked && (
          <AddContactDialog
            key={linked.stamp}
            api={api}
            suggested={linked.suggested ?? undefined}
            badLink={linked.suggested === null}
            onAdded={loadContacts}
            onClose={() => setLinked(null)}
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
        {clearing && (
          <ClearHistoryDialog
            api={api}
            onCleared={() => setCleared((n) => n + 1)}
            onClose={() => setClearing(false)}
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
      </div>
      {offer && (
        <OfferSheet
          key={offer.id}
          api={api}
          offer={offer}
          contacts={contacts}
        />
      )}
    </div>
  );
}
