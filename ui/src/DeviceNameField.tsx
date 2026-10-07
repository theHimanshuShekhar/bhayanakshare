import type { ReactNode, Ref } from "react";
import { t } from "./i18n";

/** What a Device Name may be at the most, in characters: the core cuts a longer one. */
export const MAX_DEVICE_NAME_CHARS = 64;

/** The Device Name's label, edit field and hint. Used by Settings and by first run. */
export function DeviceNameField({
  value,
  disabled,
  invalid,
  onChange,
  children,
  ref,
}: {
  value: string;
  disabled?: boolean;
  /** The name was refused: the field is marked so. */
  invalid?: boolean;
  onChange: (value: string) => void;
  /** Beside the field, as Settings' Save button is. */
  children?: ReactNode;
  /** The edit field, for giving it the focus. */
  ref?: Ref<HTMLInputElement>;
}) {
  return (
    <>
      <label htmlFor="device-name">{t("deviceName.label")}</label>
      <div className="row">
        <input
          id="device-name"
          ref={ref}
          value={value}
          disabled={disabled}
          maxLength={MAX_DEVICE_NAME_CHARS}
          autoComplete="off"
          aria-describedby="device-name-hint"
          aria-invalid={invalid ? true : undefined}
          onChange={(e) => onChange(e.target.value)}
        />
        {children}
      </div>
      <p id="device-name-hint" className="note">
        {t("deviceName.hint")}
      </p>
    </>
  );
}
