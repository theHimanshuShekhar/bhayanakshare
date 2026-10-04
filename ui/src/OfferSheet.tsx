import { useEffect, useRef, useState } from "react";
import type { Api } from "./api";
import { t } from "./i18n";
import { fingerprint, formatSize, type TransferView } from "./transfers";

/** The Receiver's view of an Offer: who, what, how big and where it will go. */
export function OfferSheet({
  api,
  offer,
  saveFolder,
}: {
  api: Api;
  offer: TransferView;
  saveFolder: string | null;
}) {
  const [error, setError] = useState<string | null>(null);
  const heading = useRef<HTMLHeadingElement>(null);

  // Each new Offer starts with its heading focused, so a screen reader reads the sheet.
  useEffect(() => {
    setError(null);
    heading.current?.focus();
  }, [offer.id]);

  const answer = (command: (id: string) => Promise<unknown>) =>
    command(offer.id).catch((e) =>
      setError(t("offer.failed", { reason: e instanceof Error ? e.message : String(e) })),
    );

  return (
    <div className="overlay">
      <div role="dialog" aria-modal="true" aria-labelledby="offer-heading" className="sheet">
        <h2 id="offer-heading" tabIndex={-1} ref={heading}>
          {t("offer.heading")}
        </h2>
        <dl>
          <dt>{t("offer.from")}</dt>
          <dd>{fingerprint(offer.peer)}</dd>
          <dt>{t("offer.file")}</dt>
          <dd>{offer.name}</dd>
          <dt>{t("offer.size")}</dt>
          <dd>{formatSize(offer.size)}</dd>
          <dt>{t("offer.saveTo")}</dt>
          <dd>
            <code>{saveFolder ?? ""}</code>
          </dd>
        </dl>
        {error !== null && <p role="alert">{error}</p>}
        <div className="actions">
          <button type="button" onClick={() => answer(api.acceptOffer)}>
            {t("offer.accept")}
          </button>
          <button type="button" onClick={() => answer(api.declineOffer)}>
            {t("offer.decline")}
          </button>
        </div>
      </div>
    </div>
  );
}
