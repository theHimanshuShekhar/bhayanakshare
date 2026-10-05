import { useEffect, useState } from "react";
import type { Api, MyId } from "./api";
import { t } from "./i18n";
import { QrCode } from "./QrCode";
import { shareLink } from "./shareLink";

type Copy = "idle" | "copied" | "failed";

/**
 * "My ID": this Device's Fingerprint, Device ID, and the share link and its QR code, with
 * buttons to copy the ID and the link.
 */
export function MyDeviceId({ api }: { api: Api }) {
  const [state, setState] = useState<"loading" | "error" | { myId: MyId; link: string }>("loading");
  const [copy, setCopy] = useState<Copy>("idle");

  useEffect(() => {
    let live = true;
    // The link carries the Device Name; without it (it could not be read) the link still works.
    Promise.all([api.myId(), api.deviceName().catch(() => null)]).then(
      ([myId, name]) => live && setState({ myId, link: shareLink(myId.id, name) }),
      () => live && setState("error"),
    );
    return () => {
      live = false;
    };
  }, [api]);

  if (state === "loading") return <p role="status">{t("home.loading")}</p>;
  if (state === "error") return <p role="alert">{t("home.error")}</p>;

  const { myId, link } = state;
  const copyText = (text: string) =>
    api.copyText(text).then(
      () => setCopy("copied"),
      () => setCopy("failed"),
    );

  return (
    <section aria-labelledby="my-id-heading">
      <h2 id="my-id-heading">{t("myId.heading")}</h2>
      <p>
        {t("myId.fingerprint")}: <strong>{myId.fingerprint}</strong>
      </p>
      <p>
        <span id="device-id-label">{t("myId.deviceId")}: </span>
        <code aria-labelledby="device-id-label">{myId.id}</code>
      </p>
      <p>
        <span id="share-link-label">{t("myId.link")}: </span>
        <code aria-labelledby="share-link-label">{link}</code>
      </p>
      <QrCode text={link} label={t("myId.qr")} />
      <p>
        <button type="button" onClick={() => copyText(myId.id)}>
          {t("myId.copy")}
        </button>{" "}
        <button type="button" onClick={() => copyText(link)}>
          {t("myId.copyLink")}
        </button>
        <span role="status" className="note">
          {copy === "copied" && t("myId.copied")}
          {copy === "failed" && t("myId.copyFailed")}
        </span>
      </p>
    </section>
  );
}
