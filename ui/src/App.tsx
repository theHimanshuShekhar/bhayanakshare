import { useEffect, useState } from "react";
import { myId, type MyId } from "./api";
import { t, type MessageKey } from "./i18n";

const TABS = [
  { id: "home", label: "tab.home", placeholder: "home.placeholder" },
  { id: "history", label: "tab.history", placeholder: "history.placeholder" },
  { id: "contacts", label: "tab.contacts", placeholder: "contacts.placeholder" },
  { id: "settings", label: "tab.settings", placeholder: "settings.placeholder" },
] as const satisfies readonly { id: string; label: MessageKey; placeholder: MessageKey }[];

type TabId = (typeof TABS)[number]["id"];

interface AppProps {
  /** Where this Device's ID comes from; the Rust shell by default, a stub in tests. */
  loadMyId?: () => Promise<MyId>;
}

export function App({ loadMyId = myId }: AppProps) {
  const [tab, setTab] = useState<TabId>("home");
  const current = TABS.find((x) => x.id === tab) ?? TABS[0];

  return (
    <div className="app">
      <header>
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
      <main>
        <p>{t(current.placeholder)}</p>
        {tab === "home" && <MyDeviceId loadMyId={loadMyId} />}
      </main>
    </div>
  );
}

function MyDeviceId({ loadMyId }: { loadMyId: () => Promise<MyId> }) {
  const [state, setState] = useState<"loading" | "error" | MyId>("loading");

  useEffect(() => {
    let live = true;
    loadMyId().then(
      (id) => live && setState(id),
      () => live && setState("error"),
    );
    return () => {
      live = false;
    };
  }, [loadMyId]);

  if (state === "loading") return <p role="status">{t("home.loading")}</p>;
  if (state === "error") return <p role="alert">{t("home.error")}</p>;
  return (
    <section aria-labelledby="my-id-heading">
      <h2 id="my-id-heading">{t("home.myId")}</h2>
      <p>
        {t("home.fingerprint")}: <strong>{state.fingerprint}</strong>
      </p>
      <code>{state.id}</code>
    </section>
  );
}
