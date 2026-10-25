import { useEffect, useRef, useState, type ReactNode } from "react";
import { Banner, Hint } from "./Banner";
import { t } from "./i18n";

/**
 * For the button that opens the composer: once the composer is left, its own buttons are gone
 * and focus would be lost to the page, so it goes back to this one.
 */
export function useFocusWhenLeft(composing: boolean) {
  const target = useRef<HTMLButtonElement>(null);
  const was = useRef(false);
  useEffect(() => {
    if (was.current && !composing) target.current?.focus();
    was.current = composing;
  }, [composing]);
  return target;
}

/**
 * Where text to send is written. Sending is up to `send`, which offers it to whoever the caller
 * has chosen; `onSent` follows once it has been offered. Text that is only blanks is not sent,
 * but what is sent is exactly what was typed.
 *
 * On its own it is a field with its buttons under it. With `inSheet` it is the whole of a
 * sheet's body and button bar, for the dialog to put under its heading: anything it is given as
 * `children` goes at the top of the body, above the text.
 */
export function TextComposer({
  send,
  onSent,
  onBack,
  disabled = false,
  inSheet = false,
  children,
}: {
  send: (text: string) => Promise<unknown>;
  onSent: () => void;
  onBack: () => void;
  /** The caller is not ready for text to go out (no Device chosen). */
  disabled?: boolean;
  /** Lay out as the body and button bar of a sheet. */
  inSheet?: boolean;
  /** Shown above the text, in the body of the sheet. */
  children?: ReactNode;
}) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const field = useRef<HTMLTextAreaElement>(null);

  useEffect(() => field.current?.focus(), []);

  const submit = async () => {
    setError(null);
    setBusy(true);
    try {
      await send(text);
      onSent();
    } catch (e) {
      setError(t("send.failed", { reason: e instanceof Error ? e.message : String(e) }));
    } finally {
      setBusy(false);
    }
  };

  const fields = (
    <>
      {children}
      <div className="bs-field">
        <label className="bs-label" htmlFor="text-to-send">
          {t("text.label")}
        </label>
        <textarea
          id="text-to-send"
          className="bs-input bs-ta"
          ref={field}
          value={text}
          onChange={(e) => setText(e.target.value)}
          aria-describedby="text-to-send-hint"
          rows={6}
          spellCheck
        />
        <Hint id="text-to-send-hint">{t("text.hint")}</Hint>
      </div>
      {error !== null && (
        <Banner kind="err" role="alert">
          {error}
        </Banner>
      )}
    </>
  );
  const buttons = (
    <>
      <button
        type="button"
        className="bs-btn bs-btn--pri"
        onClick={submit}
        disabled={busy || disabled || text.trim() === ""}
      >
        {t("text.send")}
      </button>
      <button type="button" onClick={onBack} disabled={busy}>
        {t("text.back")}
      </button>
    </>
  );

  return inSheet ? (
    <>
      <div className="bs-sheet__b">{fields}</div>
      <div className="bs-sheet__a">{buttons}</div>
    </>
  ) : (
    <div className="stack">
      {fields}
      <div className="bs-btns">{buttons}</div>
    </div>
  );
}
