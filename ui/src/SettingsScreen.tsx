import { useEffect, useState } from "react";
import type { Api, UpdateAction, Visibility } from "./api";
import { t } from "./i18n";
import { DeviceNameSettings } from "./DeviceNameSettings";
import { DiagnosticsSettings } from "./DiagnosticsSettings";
import { IdentitySettings } from "./IdentitySettings";
import { MyDeviceId } from "./MyDeviceId";
import { ReceivingSettings } from "./ReceivingSettings";
import { UpdateSettings } from "./UpdateSettings";
import type { Updater } from "./updates";
import { VisibilityField } from "./VisibilityField";

const reasonOf = (e: unknown) => (e instanceof Error ? e.message : String(e));

/**
 * The Settings tab, in sections: This Device (Device Name, My ID), Privacy (Visibility, public
 * DHT), Receiving (the save folder), App (start at login, updates), Identity and Diagnostics.
 */
export function SettingsScreen({
  api,
  updater,
  update,
  onCheckForUpdates,
}: {
  api: Api;
  /** The app's one updater, so that an update started here and one started elsewhere are one. */
  updater: Updater;
  /** The newer release found so far, if any. */
  update: UpdateAction;
  onCheckForUpdates: () => void;
}) {
  // null until the setting has been read; nothing is shown selected before then.
  const [visibility, setVisibility] = useState<Visibility | null>(null);
  const [error, setError] = useState<string | null>(null);
  // null until read.
  const [dht, setDht] = useState<boolean | null>(null);
  const [dhtError, setDhtError] = useState<string | null>(null);
  // null until read.
  const [autostart, setAutostart] = useState<boolean | null>(null);
  const [autostartError, setAutostartError] = useState<string | null>(null);
  // Counts renamings: My ID's share link holds the Device Name, so it is read again.
  const [renamed, setRenamed] = useState(0);

  useEffect(() => {
    let live = true;
    api.visibility().then(
      (v) => live && setVisibility(v),
      () => live && setError(t("visibility.loadFailed")),
    );
    api.publicDht().then(
      (on) => live && setDht(on),
      () => live && setDhtError(t("dht.loadFailed")),
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
      (e) => setAutostartError(t("autostart.failed", { reason: reasonOf(e) })),
    );
  };

  const chooseDht = (on: boolean) => {
    setDhtError(null);
    api.setPublicDht(on).then(
      () => setDht(on),
      (e) => setDhtError(t("dht.failed", { reason: reasonOf(e) })),
    );
  };

  const choose = (value: Visibility) => {
    setError(null);
    api.setVisibility(value).then(
      () => setVisibility(value),
      (e) => setError(t("visibility.failed", { reason: reasonOf(e) })),
    );
  };

  return (
    <section aria-labelledby="settings-heading">
      <h2 id="settings-heading">{t("settings.heading")}</h2>
      <section aria-labelledby="settings-this-device-heading">
        <h3 id="settings-this-device-heading">{t("settings.thisDevice")}</h3>
        <DeviceNameSettings api={api} onSaved={() => setRenamed((n) => n + 1)} />
        <MyDeviceId key={renamed} api={api} nested />
      </section>
      <section aria-labelledby="settings-privacy-heading">
        <h3 id="settings-privacy-heading">{t("settings.privacy")}</h3>
        <VisibilityField value={visibility} onChange={choose} />
        {error !== null && <p role="alert">{error}</p>}
        <p>
          <input
            id="public-dht"
            type="checkbox"
            checked={dht === true}
            disabled={dht === null}
            aria-describedby="public-dht-hint"
            onChange={(e) => chooseDht(e.target.checked)}
          />{" "}
          <label htmlFor="public-dht" className="inline">
            {t("dht.label")}
          </label>
          <span id="public-dht-hint" className="note">
            {t("dht.hint")}
          </span>
        </p>
        {dhtError !== null && <p role="alert">{dhtError}</p>}
      </section>
      <section aria-labelledby="settings-receiving-heading">
        <h3 id="settings-receiving-heading">{t("settings.receiving")}</h3>
        <ReceivingSettings api={api} />
      </section>
      <section aria-labelledby="settings-app-heading">
        <h3 id="settings-app-heading">{t("settings.app")}</h3>
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
        <UpdateSettings api={api} updater={updater} update={update} onCheck={onCheckForUpdates} />
      </section>
      <IdentitySettings api={api} />
      <DiagnosticsSettings api={api} />
    </section>
  );
}
