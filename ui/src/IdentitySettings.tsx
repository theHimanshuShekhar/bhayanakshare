import { useEffect, useState } from "react";
import type { Api, MyId } from "./api";
import { Banner } from "./Banner";
import { ExportIdentityDialog } from "./ExportIdentityDialog";
import { t } from "./i18n";
import { ImportIdentityDialog } from "./ImportIdentityDialog";
import { identityFailure } from "./identity";

/** Settings → Identity: take this Device's ID to another install, or take another one up. */
export function IdentitySettings({ api }: { api: Api }) {
  // null until read; the buttons wait for it, as the dialogs need the Fingerprint.
  const [me, setMe] = useState<MyId | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [dialog, setDialog] = useState<{ kind: "export" } | { kind: "import"; path: string } | null>(
    null,
  );

  useEffect(() => {
    let live = true;
    api.myId().then(
      (id) => live && setMe(id),
      () => live && setError(t("identity.loadFailed")),
    );
    return () => {
      live = false;
    };
  }, [api]);

  const chooseFile = async () => {
    setError(null);
    try {
      const path = await api.pickIdentityFile();
      if (path !== null) setDialog({ kind: "import", path });
    } catch (e) {
      setError(identityFailure(e));
    }
  };

  return (
    <section className="sect" aria-labelledby="identity-heading">
      <h3 className="bs-h2" id="identity-heading">
        {t("identity.heading")}
      </h3>
      <div className="sbody">
        <div className="srow">
          <p>{t("identity.body")}</p>
          {me !== null && (
            <p>
              {t("identity.current")}: <strong className="bs-mono">{me.fingerprint}</strong>
            </p>
          )}
          {error !== null && (
            <Banner kind="err" role="alert">
              {error}
            </Banner>
          )}
          <div className="bs-btns">
            <button type="button" disabled={me === null} onClick={() => setDialog({ kind: "export" })}>
              {t("identity.export")}
            </button>
            <button type="button" disabled={me === null} onClick={chooseFile}>
              {t("identity.import")}
            </button>
          </div>
        </div>
      </div>
      {me !== null && dialog?.kind === "export" && (
        <ExportIdentityDialog api={api} fingerprint={me.fingerprint} onClose={() => setDialog(null)} />
      )}
      {me !== null && dialog?.kind === "import" && (
        <ImportIdentityDialog
          api={api}
          path={dialog.path}
          current={me}
          onClose={() => setDialog(null)}
        />
      )}
    </section>
  );
}
