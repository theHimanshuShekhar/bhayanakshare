import { t } from "./i18n";

/** The id of the words a tile says besides its name, which the tile is described by. */
export const tileStateId = (deviceId: string) => `tile-${deviceId}-state`;

/**
 * What a Device's tile says besides its name, which its label ("Send to Mum") leaves out: that it
 * is a Contact, its Fingerprint, that it is Nearby, that it is ticked. These words are shown on
 * the tile and are also its description, so that a screen reader gives them with the name.
 */
export function TileState({
  deviceId,
  badge,
  print,
  nearby,
  picked,
}: {
  deviceId: string;
  /** A Contact, so marked. */
  badge: boolean;
  /** The Fingerprint to show, if it is to be. */
  print: string | null;
  nearby: boolean;
  picked: boolean;
}) {
  return (
    <span id={tileStateId(deviceId)} className="tile-state">
      {badge && <span className="badge">{t("contacts.badge")}</span>}
      {print !== null && <span className="note">{print}</span>}
      {nearby && <span className="note">{t("home.nearby")}</span>}
      {picked && <span className="note">{t("selection.selected")}</span>}
    </span>
  );
}
