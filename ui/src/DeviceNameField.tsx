import type { ReactNode, Ref } from "react";
import { FieldError, Hint } from "./Banner";
import { t } from "./i18n";

/** What a Device Name may be at the most, in characters: the core cuts a longer one. */
export const MAX_DEVICE_NAME_CHARS = 64;

/** The Device Name's label, edit field and hint. Used by Settings and by first run. */
export function DeviceNameField({
  value,
  disabled,
  invalid,
  error,
  onChange,
  children,
  ref,
}: {
  value: string;
  disabled?: boolean;
  /** The name was refused: the field is marked so. */
  invalid?: boolean;
  /** Why it was refused, said under the field. */
  error?: string | null;
  onChange: (value: string) => void;
  /** Beside the field, as Settings' Save button is. */
  children?: ReactNode;
  /** The edit field, for giving it the focus. */
  ref?: Ref<HTMLInputElement>;
}) {
  return (
    <div className="bs-field">
      <label className="bs-label" htmlFor="device-name">
        {t("deviceName.label")}
      </label>
      <div className="bs-inline">
        <input
          id="device-name"
          className="bs-input"
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
      <Hint id="device-name-hint">{t("deviceName.hint")}</Hint>
      {error != null && <FieldError>{error}</FieldError>}
    </div>
  );
}
