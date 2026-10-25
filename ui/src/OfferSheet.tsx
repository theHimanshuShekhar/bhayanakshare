import { useEffect, useRef, useState } from "react";
import type { Api, Contact, SpaceCheck } from "./api";
import { Banner, Hint } from "./Banner";
import { contactName, findContact, peerName } from "./contacts";
import { Icon } from "./Icon";
import { t } from "./i18n";
import { PlainText } from "./PlainText";
import { Sheet } from "./Sheet";
import {
  OFFER_WARNINGS,
  adjustedNamesText,
  fingerprint,
  formatCountdown,
  formatSize,
  type TransferView,
} from "./transfers";

/** How many top-level items the sheet lists before it says how many more there are: an Offer
 * can name any number of them, and the sheet has to stay usable. */
const SHOWN_ITEMS = 10;

/** The time left to answer, counting down each second. Not announced: it changes constantly. */
function Countdown({ expiresAt }: { expiresAt: number }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const tick = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(tick);
  }, []);
  return (
    <p className="offer-expiry" role="timer">
      <Icon name="clock" />
      {t("offer.expiresIn", { time: formatCountdown(expiresAt - now) })}
    </p>
  );
}

const reasonOf = (e: unknown) => (e instanceof Error ? e.message : String(e));

/**
 * The Receiver's view of an Offer: who, what, how big and where it will go. It is one sheet for
 * as long as there are Offers to answer: what is in it is made again for each (so each starts
 * without an answer, a folder or an error of the last), but the sheet is not, so the focus it
 * gives back is to where it was before the first Offer, not to the last one's heading, which is
 * gone by then.
 */
export function OfferSheet(props: { api: Api; offer: TransferView; contacts: Contact[] }) {
  return (
    <Sheet labelledBy="offer-heading">
      <Offer key={props.offer.id} {...props} />
    </Sheet>
  );
}

function Offer({
  api,
  offer,
  contacts,
}: {
  api: Api;
  offer: TransferView;
  contacts: Contact[];
}) {
  const [error, setError] = useState<string | null>(null);
  // The folder chosen for this Offer only; null means the save folder. This is keyed by Offer,
  // so each Offer starts without one.
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

  const isText = offer.kind === "text";

  // The checks run again whenever the folder changes. A text is kept by the app, not saved in
  // a folder, so it has none.
  useEffect(() => {
    if (isText) return;
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
  }, [api, offer.id, folder, isText]);

  const short =
    space !== null && space.free !== null && space.free < space.needed
      ? { needed: space.needed, free: space.free }
      : null;

  const tooLong = space !== null && space.paths_too_long;

  const answer = (command: (id: string) => Promise<unknown>) =>
    command(offer.id).catch((e) =>
      setError(t("offer.failed", { peer: peerName(offer.peer, contacts, offer.peerName), reason: reasonOf(e) })),
    );

  const changeFolder = () =>
    api.pickFolder().then(
      (picked) => picked !== null && setFolder(picked),
      (e) => setError(t("offer.folderFailed", { reason: reasonOf(e) })),
    );

  // What stops Accept: the sheet says so under the problems, and Accept points to them.
  const blocked = short !== null || tooLong || folderError !== null;
  const problems = blocked || error !== null || offer.adjustedNames > 0;

  return (
    <>
      <h2 id="offer-heading" className="bs-sheet__t" tabIndex={-1} ref={heading}>
        {t(isText ? "offer.textHeading" : "offer.heading")}
      </h2>
      <div className="bs-sheet__b">
        <dl className="bs-facts">
          <div>
            <dt>{t("offer.from")}</dt>
            <dd>
              {contact ? (
                <>
                  {contactName(contact) ?? offer.peerName ?? fingerprint(offer.peer)}{" "}
                  <span className="bs-badge">{t("contacts.badge")}</span>
                </>
              ) : (
                <>
                  {offer.peerName !== null && <>{offer.peerName} · </>}
                  {t("offer.notContact")}
                </>
              )}
            </dd>
          </div>
          <div>
            <dt>{t("offer.fingerprint")}</dt>
            <dd>
              <span className="bs-mono">{fingerprint(offer.peer)}</span>
            </dd>
          </div>
          {isText ? (
            <div>
              <dt>{t("offer.size")}</dt>
              <dd>{formatSize(offer.size)}</dd>
            </div>
          ) : (
            <>
              <div className="w2">
                <dt>{t("offer.items")}</dt>
                <dd>
                  <div>
                    <ul className="bs-items">
                      {offer.items.slice(0, SHOWN_ITEMS).map((item) => (
                        <li key={item}>{item}</li>
                      ))}
                    </ul>
                    {offer.items.length > SHOWN_ITEMS && (
                      <p className="offer-more">{t("offer.moreItems", { count: offer.items.length - SHOWN_ITEMS })}</p>
                    )}
                  </div>
                </dd>
              </div>
              <div>
                <dt>{t("offer.files")}</dt>
                <dd>{offer.fileCount}</dd>
              </div>
              <div>
                <dt>{t("offer.size")}</dt>
                <dd>{formatSize(offer.size)}</dd>
              </div>
              <div className="w2">
                <dt>{t("offer.saveTo")}</dt>
                <dd>
                  <code className="bs-mono">{folder ?? space?.folder ?? ""}</code>
                  <button
                    type="button"
                    className="bs-btn bs-btn--quiet"
                    aria-label={t("offer.changeFolderLabel")}
                    onClick={changeFolder}
                  >
                    {t("offer.changeFolder")}
                  </button>
                </dd>
              </div>
            </>
          )}
        </dl>
        {isText && <PlainText text={offer.text ?? ""} />}
        <Countdown expiresAt={offer.expiresAt} />
        {problems && (
          <div className="stack" id="offer-problems">
            {short && (
              <Banner kind="err" role="alert">
                {t(OFFER_WARNINGS.no_room[0], { needed: formatSize(short.needed), free: formatSize(short.free) })}
              </Banner>
            )}
            {tooLong && (
              <Banner kind="err" role="alert">
                {t(OFFER_WARNINGS.paths_too_long[0])}
              </Banner>
            )}
            {offer.adjustedNames > 0 && <Banner kind="warn">{adjustedNamesText(offer.adjustedNames)}</Banner>}
            {folderError !== null && (
              <Banner kind="err" role="alert">
                {folderError}
              </Banner>
            )}
            {error !== null && (
              <Banner kind="err" role="alert">
                {error}
              </Banner>
            )}
            {blocked && <Hint icon="info">{t("offer.acceptBlocked")}</Hint>}
          </div>
        )}
      </div>
      <div className="bs-sheet__a">
        <button
          type="button"
          className="bs-btn bs-btn--pri"
          disabled={blocked}
          aria-describedby={blocked ? "offer-problems" : undefined}
          onClick={() => answer((id) => api.acceptOffer(id, folder))}
        >
          {t("offer.accept")}
        </button>
        <button type="button" className="bs-btn" onClick={() => answer(api.declineOffer)}>
          {t("offer.decline")}
        </button>
      </div>
    </>
  );
}
