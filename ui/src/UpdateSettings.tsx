import { useEffect, useState } from "react";
import type { Api, UpdateAction } from "./api";
import { t } from "./i18n";
import { UpdateBanner, UpdateStatus } from "./UpdateBanner";
import type { Updater } from "./updates";

/**
 * Settings → Updates: this app's version, and a button that looks for a newer release now. What
 * it finds is offered as the banner offers it (install and restart on the Windows installer or an AppImage, a link to the
 * release page on a package), and how it went is the one update status the app has, shown here
 * while Settings is open.
 */
export function UpdateSettings({
  api,
  updater,
  update,
  onCheck,
}: {
  api: Api;
  updater: Updater;
  /** The newer release found so far, if any. */
  update: UpdateAction;
  onCheck: () => void;
}) {
  // null until read; "" if it could not be.
  const [version, setVersion] = useState<string | null>(null);
  const { forget } = updater;

  useEffect(() => {
    let live = true;
    api.appVersion().then(
      (v) => live && setVersion(v),
      () => live && setVersion(""),
    );
    return () => {
      live = false;
    };
  }, [api]);

  // "Nothing newer" is an answer to the button just pressed, not something to show again later.
  useEffect(() => forget, [forget]);

  return (
    <section aria-labelledby="updates-heading">
      <h4 id="updates-heading">{t("updates.heading")}</h4>
      {version !== null && (
        <p>{version === "" ? t("updates.versionUnknown") : t("updates.version", { version })}</p>
      )}
      <p>
        <button type="button" disabled={updater.busy} onClick={onCheck}>
          {t("updates.check")}
        </button>
      </p>
      <UpdateBanner api={api} update={update} busy={updater.busy} onInstall={updater.install} />
      <UpdateStatus updater={updater} />
    </section>
  );
}
