import { useEffect, useState } from "react";
import type { Api, MyId } from "./api";
import { Icon } from "./Icon";
import { t } from "./i18n";
import { QrCode } from "./QrCode";
import { shareLink } from "./shareLink";
import { useCopy } from "./useCopy";

/**
 * "My ID": this Device's Fingerprint, Device ID, and the share link and its QR code, with
 * buttons to copy the ID and the link. On Home it is a card under the page title, with an h3; in
 * a section of Settings (`nested`) it is the same layout without the card, with an h4.
 */
export function MyDeviceId({ api, nested = false }: { api: Api; nested?: boolean }) {
  const Heading = nested ? "h4" : "h3";
  const [state, setState] = useState<"loading" | "error" | { myId: MyId; link: string }>("loading");
  const { outcome, copy } = useCopy(api);

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

  return (
    <section aria-labelledby="my-id-heading" className={nested ? "myid" : "card myid"}>
      <Heading id="my-id-heading" className={nested ? "bs-h3" : "bs-h2"}>
        {t("myId.heading")}
      </Heading>
      <div className="myid__b">
        <QrCode text={link} label={t("myId.qr")} />
        <dl className="myid__d">
          <div>
            <dt>{t("myId.fingerprint")}</dt>
            <dd className="bs-mono fpb">{myId.fingerprint}</dd>
          </div>
          <div>
            <dt id="device-id-label">
              {t("myId.deviceId")}
              <span className="visually-hidden">:</span>
            </dt>
            <dd className="bs-mono sm">
              <code aria-labelledby="device-id-label">{myId.id}</code>
            </dd>
          </div>
          <div>
            <dt id="share-link-label">
              {t("myId.link")}
              <span className="visually-hidden">:</span>
            </dt>
            <dd className="bs-mono sm">
              <code aria-labelledby="share-link-label">{link}</code>
            </dd>
          </div>
        </dl>
      </div>
      <p className="bs-btns myid__a">
        <button type="button" onClick={() => copy(myId.id)}>
          {t("myId.copy")}
        </button>
        <button type="button" onClick={() => copy(link)}>
          {t("myId.copyLink")}
        </button>
        {outcome === "copied" && (
          <span className="copystat copystat--ok">
            <Icon name="ok" />
            {t("myId.copied")}
          </span>
        )}
        {outcome === "failed" && (
          <span role="alert" className="copystat copystat--err">
            <Icon name="err" />
            {t("myId.copyFailed")}
          </span>
        )}
      </p>
    </section>
  );
}
