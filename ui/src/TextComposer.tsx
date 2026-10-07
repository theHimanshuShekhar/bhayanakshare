import { useEffect, useRef, useState } from "react";
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
 */
export function TextComposer({
  send,
  onSent,
  onBack,
  disabled = false,
}: {
  send: (text: string) => Promise<unknown>;
  onSent: () => void;
  onBack: () => void;
  /** The caller is not ready for text to go out (no Device chosen). */
  disabled?: boolean;
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

  return (
    <>
      <label htmlFor="text-to-send">{t("text.label")}</label>
      <textarea
        id="text-to-send"
        ref={field}
        value={text}
        onChange={(e) => setText(e.target.value)}
        aria-describedby="text-to-send-hint"
        rows={6}
        spellCheck
      />
      <p id="text-to-send-hint" className="note">
        {t("text.hint")}
      </p>
      {error !== null && <p role="alert">{error}</p>}
      <div className="actions">
        <button type="button" onClick={submit} disabled={busy || disabled || text.trim() === ""}>
          {t("text.send")}
        </button>
        <button type="button" onClick={onBack} disabled={busy}>
          {t("text.back")}
        </button>
      </div>
    </>
  );
}
