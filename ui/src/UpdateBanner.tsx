import type { Api, UpdateAction } from "./api";
import { t } from "./i18n";
import { offeredVersion, useUpdater, type UpdateProgress } from "./updates";
import { RELEASES_URL } from "./versions";

/** What an update is doing now, or why it did not work. */
export function UpdateStatus({ progress }: { progress: UpdateProgress | null }) {
  if (progress === null) return null;
  switch (progress.step) {
    case "checking":
      return <p role="status">{t("update.checking")}</p>;
    case "installing":
      return <p role="status">{t("update.installing", { version: progress.version })}</p>;
    case "failed":
      return <p role="alert">{progress.message}</p>;
  }
}

/**
 * A newer release exists. An AppImage offers to install it and restart; a package, which never
 * updates itself, links to the release page.
 */
export function UpdateBanner({
  api,
  update,
  onDismiss,
}: {
  api: Api;
  update: UpdateAction;
  onDismiss: () => void;
}) {
  const updater = useUpdater(api);
  const version = offeredVersion(update);
  if (version === null) return null;
  return (
    <>
      <p role="status" className="hint">
        {t("update.available", { version })}{" "}
        {update.type === "install" ? (
          <>
            <button type="button" disabled={updater.busy} onClick={() => updater.install(version)}>
              {t("update.install")}
            </button>{" "}
            {t("update.restartHint")}
          </>
        ) : (
          <a
            href={RELEASES_URL}
            onClick={(e) => {
              // The webview must not navigate away from the app.
              e.preventDefault();
              api.openUrl(RELEASES_URL).catch(() => {});
            }}
          >
            {t("update.releasePage")}
          </a>
        )}{" "}
        <button type="button" disabled={updater.busy} onClick={onDismiss}>
          {t("update.dismiss")}
        </button>
      </p>
      <UpdateStatus progress={updater.progress} />
    </>
  );
}
