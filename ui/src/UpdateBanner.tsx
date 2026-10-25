import type { Api, UpdateAction } from "./api";
import { Banner } from "./Banner";
import { t } from "./i18n";
import { offeredVersion, type Updater } from "./updates";
import { RELEASES_URL } from "./versions";

/** What the one update under way is doing, or why it did not work. */
export function UpdateStatus({ updater }: { updater: Updater }) {
  const { progress } = updater;
  if (progress === null || progress.step === "preparing") return null;
  switch (progress.step) {
    case "checking":
      return <Banner kind="info" role="status" title={t("update.checking")} />;
    case "installing":
      return <Banner kind="info" role="status" title={t("update.installing", { version: progress.version })} />;
    case "up_to_date":
      return <Banner kind="info" role="status" title={t("update.none")} />;
    case "failed":
      return <Banner kind="err" role="alert" title={progress.message} />;
    case "confirm":
      return (
        <Banner
          kind="warn"
          role="alert"
          title={progress.active === 1 ? t("update.activeOne") : t("update.active", { count: progress.active })}
          actions={
            <>
              <button type="button" className="bs-btn bs-btn--pri" onClick={() => updater.confirm()}>
                {t("update.confirm", { version: progress.version })}
              </button>
              <button type="button" onClick={updater.cancel}>
                {t("update.cancel")}
              </button>
            </>
          }
        />
      );
  }
}

/**
 * A newer release exists. The Windows installer and an AppImage offer to install it and restart; a
 * package, which never updates itself, links to the release page. The banner can be dismissed;
 * the same notice in Settings, which is not given a way to, is what a check there found.
 */
export function UpdateBanner({
  api,
  update,
  busy,
  onInstall,
  onDismiss,
}: {
  api: Api;
  update: UpdateAction;
  /** An update is under way: nothing else may start. */
  busy: boolean;
  /** The user chose to install `version`, the one shown. */
  onInstall: (version: string) => void;
  /** Without it there is nothing to dismiss: the notice is not a banner but an answer. */
  onDismiss?: () => void;
}) {
  const version = offeredVersion(update);
  if (version === null) return null;
  return (
    <Banner
      kind="info"
      role="status"
      title={t("update.available", { version })}
      actions={
        <>
          {update.type === "install" ? (
            <button type="button" className="bs-btn bs-btn--pri" disabled={busy} onClick={() => onInstall(version)}>
              {t("update.install")}
            </button>
          ) : (
            <a
              className="bs-btn bs-btn--pri"
              href={RELEASES_URL}
              onClick={(e) => {
                // The webview must not navigate away from the app.
                e.preventDefault();
                api.openUrl(RELEASES_URL).catch(() => {});
              }}
            >
              {t("update.releasePage")}
            </a>
          )}
          {onDismiss && (
            <button type="button" disabled={busy} onClick={onDismiss}>
              {t("update.dismiss")}
            </button>
          )}
        </>
      }
    />
  );
}
