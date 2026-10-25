import { useEffect, useRef, useState, type FormEvent } from "react";
import type { Api, Visibility } from "./api";
import { AutostartField } from "./AutostartField";
import { Banner, FieldError, Hint } from "./Banner";
import { DeviceNameField } from "./DeviceNameField";
import { t, type MessageKey } from "./i18n";
import { SaveFolderField } from "./SaveFolderField";
import { saveFolderFailure } from "./saveFolder";
import { VisibilityField } from "./VisibilityField";

const reasonOf = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** What the screen offers before the user has said anything. */
interface Defaults {
  /** The Device's name: the hostname until it is renamed. */
  name: string;
  /** The Device's Visibility; null if it could not be read, which is not a reason to change it. */
  visibility: Visibility | null;
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
    Promise.allSettled([api.deviceName(), api.visibility(), api.saveFolder()]).then(
      ([name, visibility, folder]) => {
        if (!live) return;
        setLoadError([name, visibility, folder].some((r) => r.status === "rejected"));
        setDefaults({
          name: name.status === "fulfilled" ? name.value : "",
          visibility: visibility.status === "fulfilled" ? visibility.value : null,
          folder: folder.status === "fulfilled" ? folder.value : null,
        });
      },
    );
    return () => {
      live = false;
    };
  }, [api]);

  if (defaults === null) return <Hint role="status">{t("firstRun.starting")}</Hint>;
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
  // Offered as "People who have my ID" when the Device's own could not be read.
  const [visibility, setVisibility] = useState<Visibility>(defaults.visibility ?? "id_holders");
  // Whether the user chose one here: a Visibility that was only guessed is not kept.
  const [visibilityChosen, setVisibilityChosen] = useState(false);
  // On, whatever the Device says now: it is what a new install gets, and what "Get started" applies.
  const [autostart, setAutostart] = useState(true);
  // The folder the user picked here; it is kept at "Get started", not before.
  const [picked, setPicked] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [nameError, setNameError] = useState<string | null>(null);
  const [folderError, setFolderError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Set when everything is kept but start at login could not be switched on: said here, once,
  // before going on.
  const [autostartNote, setAutostartNote] = useState<string | null>(null);
  const nameField = useRef<HTMLInputElement>(null);

  // The first field has the focus, so the screen can be filled in from the keyboard at once.
  useEffect(() => nameField.current?.focus(), []);

  const chooseFolder = async () => {
    setFolderError(null);
    try {
      const chosen = await api.pickFolder();
      if (chosen !== null) setPicked(chosen);
    } catch (e) {
      setFolderError(saveFolderFailure(e));
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
        try {
          setPicked(await api.setSaveFolder(picked));
        } catch (e) {
          setFolderError(saveFolderFailure(e));
          return;
        }
      }
      const kept = await attempt(setError, "firstRun.failed", async () => {
        if (defaults.visibility !== null || visibilityChosen) await api.setVisibility(visibility);
      });
      if (!kept) return;
      // Not a reason to stop: the Device may not be able to start at login, and nobody is to be
      // stuck here for that.
      let note: string | null = null;
      try {
        await api.setAutostart(autostart);
      } catch (e) {
        note = t("firstRun.autostartFailed", { reason: reasonOf(e) });
      }
      const finished = await attempt(setError, "firstRun.failed", () => api.finishFirstRun());
      if (!finished) return;
      if (note === null) onDone();
      else setAutostartNote(note);
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="first-run" aria-labelledby="first-run-heading">
      <div className="first-run__head">
        <h2 className="bs-h1" id="first-run-heading">
          {t("firstRun.heading")}
        </h2>
        <p>{t("firstRun.intro")}</p>
      </div>
      {loadError && (
        <Banner kind="err" role="alert">
          {t("firstRun.loadFailed")}
        </Banner>
      )}
      <form className="first-run__form" onSubmit={start}>
        <div className="sbody">
          <div className="srow">
            <DeviceNameField ref={nameField} value={name} invalid={nameError !== null} error={nameError} onChange={setName} />
          </div>
          <div className="srow">
            <VisibilityField
              value={visibility}
              onChange={(value) => {
                setVisibility(value);
                setVisibilityChosen(true);
              }}
            />
          </div>
          <AutostartField checked={autostart} onChange={setAutostart} />
          <div className="srow">
            <SaveFolderField folder={picked ?? defaults.folder} disabled={busy} onChange={chooseFolder} />
            {folderError !== null && <FieldError>{folderError}</FieldError>}
          </div>
        </div>
        <Hint>{t("firstRun.privacy")}</Hint>
        {error !== null && (
          <Banner kind="err" role="alert">
            {error}
          </Banner>
        )}
        {autostartNote !== null && (
          <Banner kind="warn" role="status">
            {autostartNote}
          </Banner>
        )}
        <div className="bs-btns">
          {autostartNote === null ? (
            <button type="submit" className="bs-btn bs-btn--pri" disabled={busy}>
              {t("firstRun.start")}
            </button>
          ) : (
            <button type="button" className="bs-btn bs-btn--pri" onClick={onDone}>
              {t("firstRun.continue")}
            </button>
          )}
        </div>
      </form>
    </section>
  );
}
