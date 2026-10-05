import { useEffect, useState } from "react";
import type { Api, Visibility } from "./api";
import { t, type MessageKey } from "./i18n";

const OPTIONS = [
  { value: "everyone", label: "visibility.everyone", hint: "visibility.everyoneHint" },
  { value: "id_holders", label: "visibility.idHolders", hint: "visibility.idHoldersHint" },
  { value: "hidden", label: "visibility.hidden", hint: "visibility.hiddenHint" },
] as const satisfies readonly { value: Visibility; label: MessageKey; hint: MessageKey }[];

/** The Settings tab: who can see this Device as a Nearby Device, and start at login. */
export function SettingsScreen({ api }: { api: Api }) {
  // null until the setting has been read; nothing is shown selected before then.
  const [visibility, setVisibility] = useState<Visibility | null>(null);
  const [error, setError] = useState<string | null>(null);
  // null until read.
  const [autostart, setAutostart] = useState<boolean | null>(null);
  const [autostartError, setAutostartError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    api.visibility().then(
      (v) => live && setVisibility(v),
      () => live && setError(t("visibility.loadFailed")),
    );
    api.autostartEnabled().then(
      (on) => live && setAutostart(on),
      () => live && setAutostartError(t("autostart.loadFailed")),
    );
    return () => {
      live = false;
    };
  }, [api]);

  const chooseAutostart = (on: boolean) => {
    setAutostartError(null);
    api.setAutostart(on).then(
      () => setAutostart(on),
      (e) =>
        setAutostartError(
          t("autostart.failed", { reason: e instanceof Error ? e.message : String(e) }),
        ),
    );
  };

  const choose = (value: Visibility) => {
    setError(null);
    api.setVisibility(value).then(
      () => setVisibility(value),
      (e) =>
        setError(t("visibility.failed", { reason: e instanceof Error ? e.message : String(e) })),
    );
  };

  return (
    <section aria-labelledby="settings-heading">
      <h2 id="settings-heading">{t("settings.heading")}</h2>
      <fieldset>
        <legend>{t("visibility.legend")}</legend>
        {OPTIONS.map((option) => (
          <p key={option.value}>
            <input
              id={`visibility-${option.value}`}
              type="radio"
              name="visibility"
              checked={visibility === option.value}
              disabled={visibility === null}
              aria-describedby={`visibility-${option.value}-hint`}
              onChange={() => choose(option.value)}
            />{" "}
            <label htmlFor={`visibility-${option.value}`} className="inline">
              {t(option.label)}
            </label>
            <span id={`visibility-${option.value}-hint`} className="note">
              {t(option.hint)}
            </span>
          </p>
        ))}
      </fieldset>
      {error !== null && <p role="alert">{error}</p>}
      <p>
        <input
          id="autostart"
          type="checkbox"
          checked={autostart === true}
          disabled={autostart === null}
          aria-describedby="autostart-hint"
          onChange={(e) => chooseAutostart(e.target.checked)}
        />{" "}
        <label htmlFor="autostart" className="inline">
          {t("autostart.label")}
        </label>
        <span id="autostart-hint" className="note">
          {t("autostart.hint")}
        </span>
      </p>
      {autostartError !== null && <p role="alert">{autostartError}</p>}
    </section>
  );
}
