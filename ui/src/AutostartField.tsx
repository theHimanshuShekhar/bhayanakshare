import { SwitchRow } from "./SwitchRow";
import { t } from "./i18n";

/** Start at login: a switch with a line saying what it does. Used by Settings and by first run. */
export function AutostartField({
  checked,
  disabled,
  error,
  onChange,
}: {
  checked: boolean;
  /** Nothing can be chosen yet, as the setting has not been read. */
  disabled?: boolean;
  /** Why it could not be changed, said under the line. */
  error?: string | null;
  onChange: (on: boolean) => void;
}) {
  return (
    <SwitchRow
      id="autostart"
      label={t("autostart.label")}
      hint={t("autostart.hint")}
      checked={checked}
      disabled={disabled}
      error={error}
      onChange={onChange}
    />
  );
}
