import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import type { Api, Contact } from "./api";
import { isDeviceId } from "./contacts";
import { t } from "./i18n";
import { QrScanner } from "./QrScanner";
import { parseIdOrLink, parseShareLink, type Shared } from "./shareLink";
import { fingerprint } from "./transfers";

/**
 * "Add Contact…": paste a Device ID or share link, or scan its QR code (and give a name, if
 * known), then check the Fingerprint with the owner before it is saved. A Device ID pasted from
 * anywhere could be someone else's, and so could the name that comes with a link: the name is
 * only a suggestion, and the Fingerprint is what identifies the Device.
 */
export function AddContactDialog({
  api,
  prefilled,
  suggested,
  badLink = false,
  onAdded,
  onClose,
}: {
  api: Api;
  /** A Device already in front of the user (a Nearby tile): its ID is not typed, so the dialog
   * starts at the Fingerprint check. */
  prefilled?: Shared;
  /** What a link or QR code opened the dialog with: the fields start filled in, and the user
   * can still change both before going on. */
  suggested?: Shared;
  /** The dialog was opened by a link that is not a share link: it says so, with nothing filled in. */
  badLink?: boolean;
  onAdded: (contact: Contact) => void;
  onClose: () => void;
}) {
  const [id, setId] = useState(prefilled?.id ?? suggested?.id ?? "");
  const [name, setName] = useState(prefilled?.name ?? suggested?.name ?? "");
  const [checking, setChecking] = useState(prefilled !== undefined);
  // The name that came from a link or QR code, while the field still holds it: a link for another
  // Device replaces it, but a name the user typed or changed is theirs.
  const [suggestedName, setSuggestedName] = useState(prefilled?.name ?? suggested?.name ?? null);
  // The webcam is on, looking for a QR code; `notLink` is set when it read something else.
  const [scanning, setScanning] = useState(false);
  const [notLink, setNotLink] = useState(false);
  const [wasBadLink, setWasBadLink] = useState(badLink);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const idInput = useRef<HTMLInputElement>(null);
  const shownFingerprint = useRef<HTMLElement>(null);
  const trimmed = id.trim();
  const valid = isDeviceId(trimmed);

  useEffect(() => {
    // Hand focus back to whatever opened the dialog when it goes away.
    const opener = document.activeElement;
    return () => {
      if (opener instanceof HTMLElement) opener.focus();
    };
  }, []);

  // Each step starts with focus on its first control: the ID field, then the Fingerprint.
  useEffect(() => {
    (checking ? shownFingerprint : idInput).current?.focus();
  }, [checking]);

  /** Takes a Device ID or share link, from the ID field or a QR code, into the fields. A name
   * the user typed is kept; one from an earlier link goes when the Device is another. Returns
   * false for anything else. */
  const fill = (text: string) => {
    const found = parseIdOrLink(text);
    if (found === null) return false;
    const theirs = name.trim() !== "" && name !== suggestedName;
    if (!theirs && (found.name !== null || found.id !== id)) {
      setName(found.name ?? "");
      setSuggestedName(found.name);
    }
    setId(found.id);
    return true;
  };

  const scanned = (text: string) => {
    const found = fill(text);
    setNotLink(!found);
    if (found) setScanning(false);
  };

  // A pasted link becomes its Device ID (and name); anything else is kept as typed.
  const typed = (text: string) => {
    setWasBadLink(false);
    if (parseShareLink(text) === null) setId(text);
    else fill(text);
  };

  const add = async () => {
    setError(null);
    setBusy(true);
    try {
      const contact = await api.addContact(trimmed, name.trim() === "" ? null : name.trim());
      onAdded(contact);
      onClose();
    } catch (e) {
      setError(t("addContact.failed", { reason: e instanceof Error ? e.message : String(e) }));
    } finally {
      setBusy(false);
    }
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") onClose();
  };

  return (
    <div className="overlay" onKeyDown={onKeyDown}>
      <div role="dialog" aria-modal="true" aria-labelledby="add-contact-heading" className="sheet">
        <h2 id="add-contact-heading">
          {checking ? t("addContact.checkHeading") : t("addContact.heading")}
        </h2>
        {checking ? (
          <>
            <p>{t("addContact.check")}</p>
            <p className="fingerprint">
              <strong ref={shownFingerprint} tabIndex={-1}>
                {fingerprint(trimmed.toUpperCase())}
              </strong>
            </p>
            {error !== null && <p role="alert">{error}</p>}
            <div className="actions">
              <button type="button" onClick={add} disabled={busy}>
                {t("addContact.confirm")}
              </button>
              <button type="button" onClick={() => setChecking(false)} disabled={busy}>
                {t("addContact.back")}
              </button>
            </div>
          </>
        ) : (
          <>
            <label htmlFor="add-contact-id">{t("addContact.idLabel")}</label>
            <input
              id="add-contact-id"
              ref={idInput}
              value={id}
              onChange={(e) => typed(e.target.value)}
              aria-describedby="add-contact-id-hint"
              aria-invalid={trimmed !== "" && !valid}
              spellCheck={false}
              autoComplete="off"
            />
            <p id="add-contact-id-hint" className="note">
              {t("addContact.idHint")}
            </p>
            {trimmed !== "" && !valid && <p role="alert">{t("addContact.idInvalid")}</p>}
            {wasBadLink && <p role="alert">{t("addContact.badLink")}</p>}
            {scanning ? (
              <>
                <QrScanner onDecoded={scanned} />
                {notLink && <p role="alert">{t("addContact.scanNotLink")}</p>}
                <button type="button" onClick={() => setScanning(false)}>
                  {t("addContact.scanStop")}
                </button>
              </>
            ) : (
              <button
                type="button"
                onClick={() => {
                  setNotLink(false);
                  setScanning(true);
                }}
              >
                {t("addContact.scan")}
              </button>
            )}
            <label htmlFor="add-contact-name">{t("addContact.nameLabel")}</label>
            <input
              id="add-contact-name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              aria-describedby="add-contact-name-hint"
              maxLength={64}
              autoComplete="off"
            />
            <p id="add-contact-name-hint" className="note">
              {t("addContact.nameHint")}
            </p>
            <div className="actions">
              <button
                type="button"
                onClick={() => {
                  setScanning(false);
                  setChecking(true);
                }}
                disabled={!valid}
              >
                {t("addContact.next")}
              </button>
              <button type="button" onClick={onClose}>
                {t("addContact.cancel")}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
