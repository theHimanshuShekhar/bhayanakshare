import type { ReactNode } from "react";
import { FieldError, Hint } from "./Banner";

/**
 * A setting that is on or off, as a row of Settings: its label and a line saying what it does on
 * the left, the switch on the right. What went wrong with changing it is said under the line.
 * The switch is a checkbox drawn as a switch, so it is read as a checkbox.
 */
export function SwitchRow({
  id,
  label,
  hint,
  checked,
  disabled,
  error,
  onChange,
}: {
  /** The switch's id; the hint's is `<id>-hint`. */
  id: string;
  label: ReactNode;
  hint: ReactNode;
  checked: boolean;
  /** Nothing can be chosen yet, as the setting has not been read. */
  disabled?: boolean;
  error?: string | null;
  onChange: (on: boolean) => void;
}) {
  return (
    <div className="srow srow--split">
      <div className="bs-opt__t">
        <label className="bs-label" htmlFor={id}>
          {label}
        </label>
        <Hint id={`${id}-hint`}>{hint}</Hint>
        {error != null && <FieldError>{error}</FieldError>}
      </div>
      <input
        id={id}
        className="bs-switch"
        type="checkbox"
        checked={checked}
        disabled={disabled}
        aria-describedby={`${id}-hint`}
        onChange={(e) => onChange(e.target.checked)}
      />
    </div>
  );
}
