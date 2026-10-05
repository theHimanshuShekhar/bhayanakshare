import type { Api, Contact } from "./api";
import { peerName } from "./contacts";
import { t } from "./i18n";
import { RELEASES_URL, type VersionNotice } from "./versions";

interface VersionNoticesProps {
  api: Api;
  contacts: Contact[];
  notices: VersionNotice[];
  onDismiss: (peer: string) => void;
}

/** Devices that were refused for their version, and who has to update. */
export function VersionNotices({ api, contacts, notices, onDismiss }: VersionNoticesProps) {
  return (
    <>
      {notices.map((notice) => {
        const name = peerName(notice.peer, contacts, notice.peerName);
        const thisDevice = notice.outdated === "this_device";
        return (
          <p key={notice.peer} role="status" className="hint">
            {t(thisDevice ? "version.thisOlder" : "version.peerOlder", { name })}{" "}
            {thisDevice && (
              <button
                type="button"
                onClick={() => {
                  // Opens the releases page until the updater (#44) is wired to this button.
                  api.openUrl(RELEASES_URL).catch(() => {});
                }}
              >
                {t("version.update")}
              </button>
            )}{" "}
            <button type="button" onClick={() => onDismiss(notice.peer)}>
              {t("version.dismiss")}
            </button>
          </p>
        );
      })}
    </>
  );
}
