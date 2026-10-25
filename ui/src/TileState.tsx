import { Icon } from "./Icon";
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
      {print !== null && <span className="bs-fp">{print}</span>}
      {(badge || nearby || picked) && (
        <span className="bs-tags">
          {badge && <span className="bs-badge">{t("contacts.badge")}</span>}
          {nearby && (
            <span>
              <Icon name="dot" size={12} />
              {t("home.nearby")}
            </span>
          )}
          {picked && (
            <span>
              <Icon name="check" size={14} />
              {t("selection.selected")}
            </span>
          )}
        </span>
      )}
    </span>
  );
}
