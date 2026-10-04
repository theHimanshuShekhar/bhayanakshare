import { useEffect, useState } from "react";
import type { Api, MyId } from "./api";
import { t } from "./i18n";

type Copy = "idle" | "copied" | "failed";

/** "My ID": this Device's ID and Fingerprint, with a button to copy the ID. */
export function MyDeviceId({ api }: { api: Api }) {
  const [state, setState] = useState<"loading" | "error" | MyId>("loading");
  const [copy, setCopy] = useState<Copy>("idle");

  useEffect(() => {
    let live = true;
    api.myId().then(
      (id) => live && setState(id),
      () => live && setState("error"),
    );
    return () => {
      live = false;
    };
  }, [api]);

  if (state === "loading") return <p role="status">{t("home.loading")}</p>;
  if (state === "error") return <p role="alert">{t("home.error")}</p>;

  const copyId = () =>
    api.copyText(state.id).then(
      () => setCopy("copied"),
      () => setCopy("failed"),
    );

  return (
    <section aria-labelledby="my-id-heading">
      <h2 id="my-id-heading">{t("myId.heading")}</h2>
      <p>
        {t("myId.fingerprint")}: <strong>{state.fingerprint}</strong>
      </p>
      <p>
        <span id="device-id-label">{t("myId.deviceId")}: </span>
        <code aria-labelledby="device-id-label">{state.id}</code>
      </p>
      <button type="button" onClick={copyId}>
        {t("myId.copy")}
      </button>
      <span role="status" className="note">
        {copy === "copied" && t("myId.copied")}
        {copy === "failed" && t("myId.copyFailed")}
      </span>
    </section>
  );
}
