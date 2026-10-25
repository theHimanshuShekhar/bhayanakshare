import type { Visibility } from "./api";
import { t, type MessageKey } from "./i18n";

const OPTIONS = [
  { value: "everyone", label: "visibility.everyone", hint: "visibility.everyoneHint" },
  { value: "id_holders", label: "visibility.idHolders", hint: "visibility.idHoldersHint" },
  { value: "hidden", label: "visibility.hidden", hint: "visibility.hiddenHint" },
] as const satisfies readonly { value: Visibility; label: MessageKey; hint: MessageKey }[];

/** Who can see this Device as a Nearby Device: three cards, each a radio with a title and a line
 * saying what it means. Used by Settings and by first run. */
export function VisibilityField({
  value,
  onChange,
}: {
  /** The setting shown as chosen; null until it has been read, and nothing can be chosen before. */
  value: Visibility | null;
  onChange: (value: Visibility) => void;
}) {
  return (
    <fieldset className="rogroup">
      <legend className="bs-label">{t("visibility.legend")}</legend>
      {OPTIONS.map((option) => (
        <label key={option.value} className="rocard" htmlFor={`visibility-${option.value}`}>
          <input
            id={`visibility-${option.value}`}
            type="radio"
            name="visibility"
            checked={value === option.value}
            disabled={value === null}
            // The card holds the line too, so the title alone names the radio, as before.
            aria-labelledby={`visibility-${option.value}-title`}
            aria-describedby={`visibility-${option.value}-hint`}
            onChange={() => onChange(option.value)}
          />
          <span className="bs-opt__t">
            <b id={`visibility-${option.value}-title`}>{t(option.label)}</b>
            <span id={`visibility-${option.value}-hint`} className="bs-hint">
              {t(option.hint)}
            </span>
          </span>
        </label>
      ))}
    </fieldset>
  );
}
