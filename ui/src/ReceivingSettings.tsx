import { useEffect, useState } from "react";
import type { Api } from "./api";
import { t } from "./i18n";
import { SaveFolderField } from "./SaveFolderField";
import { saveFolderFailure } from "./saveFolder";

/** Settings → Receiving: the folder received files are saved to, which applies from the next
 * Offer on. */
export function ReceivingSettings({ api }: { api: Api }) {
  // null until read.
  const [folder, setFolder] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    let live = true;
    api.saveFolder().then(
      (path) => live && setFolder(path),
      () => live && setError(t("saveFolder.loadFailed")),
    );
    return () => {
      live = false;
    };
  }, [api]);

  const change = async () => {
    setError(null);
    setSaved(false);
    try {
      const picked = await api.pickFolder();
      // Cancelling the dialog changes nothing.
      if (picked === null) return;
      setFolder(await api.setSaveFolder(picked));
      setSaved(true);
    } catch (e) {
      setError(saveFolderFailure(e));
    }
  };

  return (
    <>
      <SaveFolderField folder={folder} onChange={change} />
      <p className="note">{t("saveFolder.hint")}</p>
      {error !== null && <p role="alert">{error}</p>}
      {saved && <p role="status">{t("saveFolder.saved")}</p>}
    </>
  );
}
