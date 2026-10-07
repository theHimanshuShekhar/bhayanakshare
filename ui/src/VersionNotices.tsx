import type { Contact } from "./api";
import { peerName } from "./contacts";
import { t } from "./i18n";
import type { VersionNotice } from "./versions";

interface VersionNoticesProps {
  contacts: Contact[];
  notices: VersionNotice[];
  /** An update is under way: "Update now" waits for it. */
  updating: boolean;
  onUpdateNow: () => void;
  onDismiss: (peer: string) => void;
}

/** Devices that were refused for their version, and who has to update. */
export function VersionNotices({ contacts, notices, updating, onUpdateNow, onDismiss }: VersionNoticesProps) {
  return (
    <>
      {notices.map((notice) => {
        const name = peerName(notice.peer, contacts, notice.peerName);
        const thisDevice = notice.outdated === "this_device";
        return (
          <p key={notice.peer} role="status" className="hint">
            {t(thisDevice ? "version.thisOlder" : "version.peerOlder", { name })}{" "}
            {thisDevice && (
              <button type="button" disabled={updating} onClick={onUpdateNow}>
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
