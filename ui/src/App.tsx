import { useEffect, useReducer, useState } from "react";
import { tauriApi, type Api } from "./api";
import { MyDeviceId } from "./MyDeviceId";
import { OfferSheet } from "./OfferSheet";
import { SendDialog } from "./SendDialog";
import { TransferList } from "./TransferList";
import { t, type MessageKey } from "./i18n";
import { applyEvent, newestFirst, noTransfers, pendingOffer } from "./transfers";

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
  const [sending, setSending] = useState(false);
  const [transfers, dispatch] = useReducer(applyEvent, noTransfers);
  const [saveFolder, setSaveFolder] = useState<string | null>(null);
  const current = TABS.find((x) => x.id === tab) ?? TABS[0];
  const offer = pendingOffer(transfers);
  // While a sheet is open the page behind it can be neither clicked nor tabbed to.
  const inert = sending || offer !== undefined;

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
        <p>{t(current.placeholder)}</p>
        {tab === "home" && (
          <>
            <MyDeviceId api={api} />
            <section aria-labelledby="devices-heading">
              <h2 id="devices-heading">{t("home.devices")}</h2>
              <div className="tiles">
                <button type="button" className="tile" onClick={() => setSending(true)}>
                  {t("home.sendToId")}
                </button>
              </div>
            </section>
            <section aria-labelledby="transfers-heading">
              <h2 id="transfers-heading">{t("home.transfers")}</h2>
              {transfers.order.length === 0 ? (
                <p>{t("home.noTransfers")}</p>
              ) : (
                <TransferList api={api} transfers={newestFirst(transfers)} />
              )}
            </section>
          </>
        )}
      </main>
      {sending && <SendDialog api={api} onClose={() => setSending(false)} />}
      {offer && <OfferSheet api={api} offer={offer} saveFolder={saveFolder} />}
    </div>
  );
}
