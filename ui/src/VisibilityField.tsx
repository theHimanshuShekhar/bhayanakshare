import type { Visibility } from "./api";
import { t, type MessageKey } from "./i18n";

const OPTIONS = [
  { value: "everyone", label: "visibility.everyone", hint: "visibility.everyoneHint" },
  { value: "id_holders", label: "visibility.idHolders", hint: "visibility.idHoldersHint" },
  { value: "hidden", label: "visibility.hidden", hint: "visibility.hiddenHint" },
] as const satisfies readonly { value: Visibility; label: MessageKey; hint: MessageKey }[];

/** Who can see this Device as a Nearby Device: three radios, each with a line saying what it means.
 * Used by Settings and by first run. */
export function VisibilityField({
  value,
  onChange,
}: {
  /** The setting shown as chosen; null until it has been read, and nothing can be chosen before. */
  value: Visibility | null;
  onChange: (value: Visibility) => void;
}) {
  return (
    <fieldset>
      <legend>{t("visibility.legend")}</legend>
      {OPTIONS.map((option) => (
        <p key={option.value}>
          <input
            id={`visibility-${option.value}`}
            type="radio"
            name="visibility"
            checked={value === option.value}
            disabled={value === null}
            aria-describedby={`visibility-${option.value}-hint`}
            onChange={() => onChange(option.value)}
          />{" "}
          <label htmlFor={`visibility-${option.value}`} className="inline">
            {t(option.label)}
          </label>
          <span id={`visibility-${option.value}-hint`} className="note">
            {t(option.hint)}
          </span>
        </p>
      ))}
    </fieldset>
  );
}
