import { useEffect, useState, type FormEvent } from "react";
import type { Api } from "./api";
import { DeviceNameField } from "./DeviceNameField";
import { t } from "./i18n";

const reasonOf = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** Settings → Device Name: an edit field with a Save button. A new name is announced at once. */
export function DeviceNameSettings({ api, onSaved }: { api: Api; onSaved: () => void }) {
  // The name as the Device has it; null until read.
  const [saved, setSaved] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    api.deviceName().then(
      (name) => {
        if (!live) return;
        setSaved(name);
        setDraft(name);
      },
      () => live && setError(t("deviceName.loadFailed")),
    );
    return () => {
      live = false;
    };
  }, [api]);

  const save = (e: FormEvent) => {
    e.preventDefault();
    setError(null);
    setStatus(null);
    if (draft.trim() === "") {
      setError(t("deviceName.empty"));
      return;
    }
    api.setDeviceName(draft).then(
      (kept) => {
        // The Device may have trimmed or cut it: show what the others will see.
        setSaved(kept);
        setDraft(kept);
        setStatus(t("deviceName.saved", { name: kept }));
        onSaved();
      },
      (err) => setError(t("deviceName.failed", { reason: reasonOf(err) })),
    );
  };

  return (
    <form onSubmit={save}>
      <DeviceNameField
        value={draft}
        disabled={saved === null}
        invalid={error !== null && draft.trim() === ""}
        onChange={(value) => {
          setDraft(value);
          setStatus(null);
        }}
      >
        <button type="submit" disabled={saved === null || draft.trim() === saved}>
          {t("deviceName.save")}
        </button>
      </DeviceNameField>
      {error !== null && <p role="alert">{error}</p>}
      {status !== null && <p role="status">{status}</p>}
    </form>
  );
}
