import { useState } from "react";
import type { Api } from "./api";
import { t } from "./i18n";
import { baseName } from "./transfers";

/** The checkbox on a Device's tile that adds it to, or takes it out of, a send to several. */
export function SelectBox({
  name,
  checked,
  onChange,
}: {
  name: string;
  checked: boolean;
  onChange: () => void;
}) {
  return (
    <label className="select">
      <input
        type="checkbox"
        checked={checked}
        onChange={onChange}
        aria-label={t("selection.selectLabel", { name })}
      />{" "}
      {t("selection.select")}
    </label>
  );
}

/**
 * What to do with the Devices chosen on Home: send them the same files or folder, which makes
 * a Batch (one Transfer each) when there are several. With `files` already chosen (a second
 * launch, or the tray's "Send files…"), they go out as they are, each as a Batch of its own.
 */
export function SelectionBar({
  api,
  ids,
  files,
  onClear,
  onSent,
  onFilesSent,
}: {
  api: Api;
  /** The Device IDs chosen, at least one. */
  ids: string[];
  /** Files and folders waiting to be sent to someone. */
  files: string[];
  onClear: () => void;
  /** Called once what was asked for has been offered. */
  onSent: () => void;
  /** Called once every one of `files` has been offered. */
  onFilesSent: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const offer = (paths: string[]) =>
    ids.length === 1 ? api.sendFiles(ids[0], paths) : api.sendBatch(ids, paths);

  /** Sends what `pick` asks the user for, or the files already chosen if there are some. */
  const send = async (pick?: () => Promise<string[] | null>) => {
    setError(null);
    setBusy(true);
    try {
      if (pick !== undefined) {
        const paths = await pick();
        if (paths === null) return;
        await offer(paths);
      } else {
        for (const path of files) await offer([path]);
        onFilesSent();
      }
      onSent();
    } catch (e) {
      setError(t("send.failed", { reason: e instanceof Error ? e.message : String(e) }));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="selection">
      <p role="status">
        {ids.length === 1 ? t("selection.one") : t("selection.many", { count: ids.length })}
      </p>
      <div className="actions">
        {files.length > 0 ? (
          <button type="button" onClick={() => send()} disabled={busy}>
            {t("selection.sendQueued", { names: files.map(baseName).join(", ") })}
          </button>
        ) : (
          <>
            <button type="button" onClick={() => send(api.pickFiles)} disabled={busy}>
              {t("send.chooseFiles")}
            </button>
            <button
              type="button"
              onClick={() => send(() => api.pickFolder().then((folder) => (folder === null ? null : [folder])))}
              disabled={busy}
            >
              {t("send.chooseFolder")}
            </button>
          </>
        )}
        <button type="button" onClick={onClear} disabled={busy}>
          {t("selection.clear")}
        </button>
      </div>
      {error !== null && <p role="alert">{error}</p>}
    </div>
  );
}
