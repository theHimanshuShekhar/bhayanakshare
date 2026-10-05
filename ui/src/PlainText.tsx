import { t } from "./i18n";

/**
 * Text that came from another Device, shown as what it is: characters. It goes in as a React
 * child, which is escaped, never as markup; line breaks and spacing are kept by the style. It
 * can be long, so it scrolls, and can be reached with the keyboard to scroll it.
 */
export function PlainText({ text }: { text: string }) {
  return (
    <pre className="plain-text" tabIndex={0} role="region" aria-label={t("text.label")}>
      {text}
    </pre>
  );
}
