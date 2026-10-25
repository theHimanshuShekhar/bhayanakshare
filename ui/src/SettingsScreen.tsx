import { useEffect, useState } from "react";
import type { Api, UpdateAction, Visibility } from "./api";
import { AutostartField } from "./AutostartField";
import { FieldError } from "./Banner";
import { t } from "./i18n";
import { DeviceNameSettings } from "./DeviceNameSettings";
import { DiagnosticsSettings } from "./DiagnosticsSettings";
import { IdentitySettings } from "./IdentitySettings";
import { MyDeviceId } from "./MyDeviceId";
import { ReceivingSettings } from "./ReceivingSettings";
import { UpdateSettings } from "./UpdateSettings";
import { SwitchRow } from "./SwitchRow";
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
    <section className="settings" aria-labelledby="settings-heading">
      <div className="pagehead">
        <h2 className="bs-h1" id="settings-heading">
          {t("settings.heading")}
        </h2>
      </div>
      <section className="sect" aria-labelledby="settings-this-device-heading">
        <h3 className="bs-h2" id="settings-this-device-heading">
          {t("settings.thisDevice")}
        </h3>
        <div className="sbody">
          <div className="srow">
            <DeviceNameSettings api={api} onSaved={() => setRenamed((n) => n + 1)} />
          </div>
          <div className="srow">
            <MyDeviceId key={renamed} api={api} nested />
          </div>
        </div>
      </section>
      <section className="sect" aria-labelledby="settings-privacy-heading">
        <h3 className="bs-h2" id="settings-privacy-heading">
          {t("settings.privacy")}
        </h3>
        <div className="sbody">
          <div className="srow">
            <VisibilityField value={visibility} onChange={choose} />
            {error !== null && <FieldError>{error}</FieldError>}
          </div>
          <SwitchRow
            id="public-dht"
            label={t("dht.label")}
            hint={t("dht.hint")}
            checked={dht === true}
            disabled={dht === null}
            error={dhtError}
            onChange={chooseDht}
          />
        </div>
      </section>
      <section className="sect" aria-labelledby="settings-receiving-heading">
        <h3 className="bs-h2" id="settings-receiving-heading">
          {t("settings.receiving")}
        </h3>
        <div className="sbody">
          <ReceivingSettings api={api} />
        </div>
      </section>
      <section className="sect" aria-labelledby="settings-app-heading">
        <h3 className="bs-h2" id="settings-app-heading">
          {t("settings.app")}
        </h3>
        <div className="sbody">
          <AutostartField
            checked={autostart === true}
            disabled={autostart === null}
            error={autostartError}
            onChange={chooseAutostart}
          />
          <UpdateSettings api={api} updater={updater} update={update} onCheck={onCheckForUpdates} />
        </div>
      </section>
      <IdentitySettings api={api} />
      <DiagnosticsSettings api={api} />
    </section>
  );
}
