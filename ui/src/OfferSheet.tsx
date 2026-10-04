import { useEffect, useRef, useState } from "react";
import type { Api, Contact, SpaceCheck } from "./api";
import { contactName, findContact } from "./contacts";
import { t } from "./i18n";
import { fingerprint, formatCountdown, formatSize, type TransferView } from "./transfers";

/** The time left to answer, counting down each second. Not announced: it changes constantly. */
function Countdown({ expiresAt }: { expiresAt: number }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const tick = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(tick);
  }, []);
  return <p role="timer">{t("offer.expiresIn", { time: formatCountdown(expiresAt - now) })}</p>;
}

const reasonOf = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** The Receiver's view of an Offer: who, what, how big and where it will go. */
export function OfferSheet({
  api,
  offer,
  contacts,
  saveFolder,
}: {
  api: Api;
  offer: TransferView;
  contacts: Contact[];
  saveFolder: string | null;
}) {
  const [error, setError] = useState<string | null>(null);
  // The folder chosen for this Offer only; null means the save folder. The sheet is keyed
  // by Offer, so each Offer starts without one.
  const [folder, setFolder] = useState<string | null>(null);
  const [space, setSpace] = useState<SpaceCheck | null>(null);
  const [folderError, setFolderError] = useState<string | null>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const contact = findContact(contacts, offer.peer);

  // Each new Offer starts with its heading focused, so a screen reader reads the sheet.
  useEffect(() => {
    setError(null);
    heading.current?.focus();
  }, [offer.id]);

  // The checks run again whenever the folder changes.
  useEffect(() => {
    let live = true;
    setSpace(null);
    setFolderError(null);
    api.checkOffer(offer.id, folder).then(
      (check) => live && setSpace(check),
      (e) => live && setFolderError(t("offer.folderFailed", { reason: reasonOf(e) })),
    );
    return () => {
      live = false;
    };
  }, [api, offer.id, folder]);

  const short =
    space !== null && space.free !== null && space.free < space.needed
      ? { needed: space.needed, free: space.free }
      : null;

  const answer = (command: (id: string) => Promise<unknown>) =>
    command(offer.id).catch((e) => setError(t("offer.failed", { reason: reasonOf(e) })));

  const changeFolder = () =>
    api.pickFolder().then(
      (picked) => picked !== null && setFolder(picked),
      (e) => setError(t("offer.folderFailed", { reason: reasonOf(e) })),
    );

  return (
    <div className="overlay">
      <div role="dialog" aria-modal="true" aria-labelledby="offer-heading" className="sheet">
        <h2 id="offer-heading" tabIndex={-1} ref={heading}>
          {t("offer.heading")}
        </h2>
        <dl>
          <dt>{t("offer.from")}</dt>
          <dd>
            {contact ? (
              <>
                {contactName(contact) ?? offer.peerName ?? fingerprint(offer.peer)}{" "}
                <span className="badge">{t("contacts.badge")}</span>
              </>
            ) : (
              <>
                {offer.peerName !== null && <>{offer.peerName} · </>}
                {t("offer.notContact")}
              </>
            )}
          </dd>
          <dt>{t("offer.fingerprint")}</dt>
          <dd>{fingerprint(offer.peer)}</dd>
          <dt>{t("offer.file")}</dt>
          <dd>{offer.name}</dd>
          <dt>{t("offer.size")}</dt>
          <dd>{formatSize(offer.size)}</dd>
          <dt>{t("offer.saveTo")}</dt>
          <dd>
            <code>{folder ?? saveFolder ?? ""}</code>{" "}
            <button type="button" aria-label={t("offer.changeFolderLabel")} onClick={changeFolder}>
              {t("offer.changeFolder")}
            </button>
          </dd>
        </dl>
        <Countdown expiresAt={offer.expiresAt} />
        {short && (
          <p role="alert">
            {t("offer.noRoom", { needed: formatSize(short.needed), free: formatSize(short.free) })}
          </p>
        )}
        {folderError !== null && <p role="alert">{folderError}</p>}
        {error !== null && <p role="alert">{error}</p>}
        <div className="actions">
          <button
            type="button"
            disabled={short !== null || folderError !== null}
            onClick={() => answer((id) => api.acceptOffer(id, folder))}
          >
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
