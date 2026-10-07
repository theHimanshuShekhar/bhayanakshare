import { useEffect, useRef, useState, type FormEvent } from "react";
import type { Api, Visibility } from "./api";
import { MAX_DEVICE_NAME_CHARS } from "./DeviceNameSettings";
import { t, type MessageKey } from "./i18n";
import { SaveFolderField } from "./SaveFolderField";
import { VisibilityField } from "./VisibilityField";

const reasonOf = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** What the screen offers before the user has said anything. */
interface Defaults {
  /** The Device's name: the hostname until it is renamed. */
  name: string;
  visibility: Visibility;
  /** Whether the Device starts at login now. */
  autostart: boolean;
  /** The save folder now: `~/Downloads/BhayanakShare` unless the install is set up otherwise. */
  folder: string | null;
}

/**
 * First run: the one screen a new install shows instead of the tabs. Device Name (the hostname),
 * Visibility ("People who have my ID"), start at login (on) and the save folder
 * (`~/Downloads/BhayanakShare`) are all there to accept as they are; "Get started" keeps them
 * and goes on.
 */
export function FirstRunScreen({ api, onDone }: { api: Api; onDone: () => void }) {
  // null until the Device has said what it has.
  const [defaults, setDefaults] = useState<Defaults | null>(null);
  const [loadError, setLoadError] = useState(false);

  useEffect(() => {
    let live = true;
    Promise.allSettled([api.deviceName(), api.visibility(), api.autostartEnabled(), api.saveFolder()]).then(
      ([name, visibility, autostart, folder]) => {
        if (!live) return;
        setLoadError([name, visibility, autostart, folder].some((r) => r.status === "rejected"));
        setDefaults({
          name: name.status === "fulfilled" ? name.value : "",
          visibility: visibility.status === "fulfilled" ? visibility.value : "id_holders",
          // The shell switches it on the first time it runs: that is what is shown, but what
          // is true, if it cannot be read, is not known.
          autostart: autostart.status === "fulfilled" ? autostart.value : true,
          folder: folder.status === "fulfilled" ? folder.value : null,
        });
      },
    );
    return () => {
      live = false;
    };
  }, [api]);

  if (defaults === null) return <p role="status">{t("firstRun.starting")}</p>;
  return <FirstRunForm api={api} defaults={defaults} loadError={loadError} onDone={onDone} />;
}

function FirstRunForm({
  api,
  defaults,
  loadError,
  onDone,
}: {
  api: Api;
  defaults: Defaults;
  loadError: boolean;
  onDone: () => void;
}) {
  const [name, setName] = useState(defaults.name);
  const [visibility, setVisibility] = useState<Visibility>(defaults.visibility);
  const [autostart, setAutostart] = useState(defaults.autostart);
  // The folder the user picked here; it is kept at "Get started", not before.
  const [picked, setPicked] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [nameError, setNameError] = useState<string | null>(null);
  const [folderError, setFolderError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const nameField = useRef<HTMLInputElement>(null);

  // The first field has the focus, so the screen can be filled in from the keyboard at once.
  useEffect(() => nameField.current?.focus(), []);

  const chooseFolder = async () => {
    setFolderError(null);
    try {
      const chosen = await api.pickFolder();
      if (chosen !== null) setPicked(chosen);
    } catch (e) {
      setFolderError(t("saveFolder.failed", { reason: reasonOf(e) }));
    }
  };

  /** Runs one step of keeping the choices; a failure is shown where `show` puts it. */
  const attempt = async (
    show: (message: string | null) => void,
    failed: MessageKey,
    step: () => Promise<unknown>,
  ) => {
    try {
      await step();
      return true;
    } catch (e) {
      show(t(failed, { reason: reasonOf(e) }));
      return false;
    }
  };

  const start = async (e: FormEvent) => {
    e.preventDefault();
    setNameError(null);
    setFolderError(null);
    setError(null);
    if (name.trim() === "") {
      setNameError(t("deviceName.empty"));
      nameField.current?.focus();
      return;
    }
    setBusy(true);
    try {
      const keptName = await attempt(setNameError, "deviceName.failed", async () => setName(await api.setDeviceName(name)));
      if (!keptName) return nameField.current?.focus();
      if (picked !== null) {
        const keptFolder = await attempt(setFolderError, "saveFolder.failed", async () => {
          setPicked(await api.setSaveFolder(picked));
        });
        if (!keptFolder) return;
      }
      const rest = await attempt(setError, "firstRun.failed", async () => {
        await api.setVisibility(visibility);
        // Left alone when it is as it was: the Device may not be one that starts at login.
        if (autostart !== defaults.autostart) await api.setAutostart(autostart);
        await api.finishFirstRun();
      });
      if (rest) onDone();
    } finally {
      setBusy(false);
    }
  };

  return (
    <section aria-labelledby="first-run-heading">
      <h2 id="first-run-heading">{t("firstRun.heading")}</h2>
      <p>{t("firstRun.intro")}</p>
      {loadError && <p role="alert">{t("firstRun.loadFailed")}</p>}
      <form onSubmit={start}>
        <label htmlFor="first-run-name">{t("deviceName.label")}</label>
        <input
          id="first-run-name"
          ref={nameField}
          value={name}
          maxLength={MAX_DEVICE_NAME_CHARS}
          autoComplete="off"
          aria-describedby="first-run-name-hint"
          aria-invalid={nameError !== null ? true : undefined}
          onChange={(e) => setName(e.target.value)}
        />
        <p id="first-run-name-hint" className="note">
          {t("deviceName.hint")}
        </p>
        {nameError !== null && <p role="alert">{nameError}</p>}
        <VisibilityField value={visibility} onChange={setVisibility} />
        <p>
          <input
            id="first-run-autostart"
            type="checkbox"
            checked={autostart}
            aria-describedby="first-run-autostart-hint"
            onChange={(e) => setAutostart(e.target.checked)}
          />{" "}
          <label htmlFor="first-run-autostart" className="inline">
            {t("autostart.label")}
          </label>
          <span id="first-run-autostart-hint" className="note">
            {t("autostart.hint")}
          </span>
        </p>
        <SaveFolderField folder={picked ?? defaults.folder} disabled={busy} onChange={chooseFolder} />
        {folderError !== null && <p role="alert">{folderError}</p>}
        <p className="note">{t("firstRun.privacy")}</p>
        {error !== null && <p role="alert">{error}</p>}
        <p>
          <button type="submit" disabled={busy}>
            {t("firstRun.start")}
          </button>
        </p>
      </form>
    </section>
  );
}
