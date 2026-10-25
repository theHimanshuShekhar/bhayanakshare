import { useEffect, useState } from "react";
import type { Api } from "./api";
import { Banner, Hint } from "./Banner";
import { BUG_REPORT_URL, diagnosticsFileName } from "./diagnostics";
import { t } from "./i18n";
import { SwitchRow } from "./SwitchRow";

const reasonOf = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** Settings → Diagnostics: the debug logging switch, and exporting the log as a zip to hand over. */
export function DiagnosticsSettings({ api }: { api: Api }) {
  // null until read.
  const [debug, setDebug] = useState<boolean | null>(null);
  const [debugError, setDebugError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [exportError, setExportError] = useState<string | null>(null);
  const [savedTo, setSavedTo] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    api.debugLogging().then(
      (on) => live && setDebug(on),
      () => live && setDebugError(t("diagnostics.debugLoadFailed")),
    );
    return () => {
      live = false;
    };
  }, [api]);

  const chooseDebug = (on: boolean) => {
    setDebugError(null);
    api.setDebugLogging(on).then(
      () => setDebug(on),
      (e) => setDebugError(t("diagnostics.debugFailed", { reason: reasonOf(e) })),
    );
  };

  const exportDiagnostics = async () => {
    setExportError(null);
    setSavedTo(null);
    setBusy(true);
    try {
      const path = await api.pickDiagnosticsSavePath(diagnosticsFileName(new Date()));
      // Cancelling the save dialog changes nothing.
      if (path === null) return;
      await api.exportDiagnostics(path);
      setSavedTo(path);
    } catch (e) {
      setExportError(t("diagnostics.exportFailed", { reason: reasonOf(e) }));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="sect" aria-labelledby="diagnostics-heading">
      <h3 className="bs-h2" id="diagnostics-heading">
        {t("diagnostics.heading")}
      </h3>
      <div className="sbody">
        <div className="srow">
          <p>{t("diagnostics.body")}</p>
        </div>
        <SwitchRow
          id="debug-logging"
          label={t("diagnostics.debug")}
          hint={t("diagnostics.debugHint")}
          checked={debug === true}
          disabled={debug === null}
          error={debugError}
          onChange={chooseDebug}
        />
        <div className="srow">
          <div className="bs-btns">
            <button type="button" disabled={busy} onClick={exportDiagnostics}>
              {t("diagnostics.export")}
            </button>
          </div>
          {savedTo !== null && (
            <Banner kind="ok" role="status">
              {t("diagnostics.exported", { path: savedTo })}
            </Banner>
          )}
          {exportError !== null && (
            <Banner kind="err" role="alert">
              {exportError}
            </Banner>
          )}
          <Hint>
            <a
              href={BUG_REPORT_URL}
              aria-describedby="report-hint"
              onClick={(e) => {
                // The webview must not navigate away from the app.
                e.preventDefault();
                api.openUrl(BUG_REPORT_URL).catch(() => {});
              }}
            >
              {t("diagnostics.report")}
            </a>{" "}
            <span id="report-hint">{t("diagnostics.reportHint")}</span>
          </Hint>
        </div>
      </div>
    </section>
  );
}
