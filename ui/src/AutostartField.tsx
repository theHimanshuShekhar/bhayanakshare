import { t } from "./i18n";

/** Start at login: a checkbox with a line saying what it does. Used by Settings and by first run. */
export function AutostartField({
  checked,
  disabled,
  onChange,
}: {
  checked: boolean;
  /** Nothing can be chosen yet, as the setting has not been read. */
  disabled?: boolean;
  onChange: (on: boolean) => void;
}) {
  return (
    <p>
      <input
        id="autostart"
        type="checkbox"
        checked={checked}
        disabled={disabled}
        aria-describedby="autostart-hint"
        onChange={(e) => onChange(e.target.checked)}
      />{" "}
      <label htmlFor="autostart" className="inline">
        {t("autostart.label")}
      </label>
      <span id="autostart-hint" className="note">
        {t("autostart.hint")}
      </span>
    </p>
  );
}
