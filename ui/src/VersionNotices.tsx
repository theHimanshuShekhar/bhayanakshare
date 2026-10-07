import type { Api, Contact } from "./api";
import { peerName } from "./contacts";
import { t } from "./i18n";
import { UpdateStatus } from "./UpdateBanner";
import { useUpdater } from "./updates";
import type { VersionNotice } from "./versions";

interface VersionNoticesProps {
  api: Api;
  contacts: Contact[];
  notices: VersionNotice[];
  onDismiss: (peer: string) => void;
}

/** Devices that were refused for their version, and who has to update. */
export function VersionNotices({ api, contacts, notices, onDismiss }: VersionNoticesProps) {
  const updater = useUpdater(api);
  return (
    <>
      {notices.map((notice) => {
        const name = peerName(notice.peer, contacts, notice.peerName);
        const thisDevice = notice.outdated === "this_device";
        return (
          <p key={notice.peer} role="status" className="hint">
            {t(thisDevice ? "version.thisOlder" : "version.peerOlder", { name })}{" "}
            {thisDevice && (
              <button type="button" disabled={updater.busy} onClick={() => updater.updateNow()}>
                {t("version.update")}
              </button>
            )}{" "}
            <button type="button" onClick={() => onDismiss(notice.peer)}>
              {t("version.dismiss")}
            </button>
          </p>
        );
      })}
      {notices.length > 0 && <UpdateStatus progress={updater.progress} />}
    </>
  );
}
