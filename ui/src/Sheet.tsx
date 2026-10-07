import { useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";

const TABBABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/**
 * The frame every dialog and sheet shares, so that they all behave the same way from the
 * keyboard: focus is inside when it opens (a dialog that moves it to a field of its own does so
 * first; otherwise it goes to the first control), Tab and Shift+Tab go round inside it, Escape
 * closes it where `onEscape` says it may, and focus goes back to whatever had it when it goes
 * away. The page behind a sheet is made inert by the app where it can be; the trap is what holds
 * for a sheet that sits inside the page (in Settings) or on top of another sheet.
 */
export function Sheet({
  role = "dialog",
  labelledBy,
  describedBy,
  onEscape,
  children,
}: {
  /** An `alertdialog` asks the user to confirm something before going on. */
  role?: "dialog" | "alertdialog";
  /** The id of the heading that names the sheet. */
  labelledBy: string;
  /** The id of the text that says what is being asked. */
  describedBy?: string;
  /** Closes the sheet. Leave it out for one that must be answered, such as an Offer. */
  onEscape?: () => void;
  children: ReactNode;
}) {
  const sheet = useRef<HTMLDivElement>(null);
  // Read while rendering, before any effect moves focus into the sheet.
  const [opener] = useState(() => document.activeElement);

  useEffect(() => {
    const inside = sheet.current;
    if (inside && !inside.contains(document.activeElement)) {
      (inside.querySelector<HTMLElement>(TABBABLE) ?? inside).focus();
    }
    return () => {
      if (opener instanceof HTMLElement && opener.isConnected) opener.focus();
    };
  }, [opener]);

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape" && onEscape) {
      onEscape();
      return;
    }
    if (e.key !== "Tab" || !sheet.current) return;
    // Hidden controls need no care here: the sheets show or remove what they do not offer.
    const tabbable = [...sheet.current.querySelectorAll<HTMLElement>(TABBABLE)];
    const active = document.activeElement;
    if (tabbable.length === 0) {
      e.preventDefault();
      sheet.current.focus();
    } else if (e.shiftKey && (active === tabbable[0] || !tabbable.includes(active as HTMLElement))) {
      // Also from the sheet itself or a heading given focus, where the browser would go back
      // out of the sheet.
      e.preventDefault();
      tabbable[tabbable.length - 1].focus();
    } else if (!e.shiftKey && active === tabbable[tabbable.length - 1]) {
      e.preventDefault();
      tabbable[0].focus();
    }
  };

  return (
    <div className="overlay" onKeyDown={onKeyDown}>
      <div
        ref={sheet}
        role={role}
        aria-modal="true"
        aria-labelledby={labelledBy}
        aria-describedby={describedBy}
        tabIndex={-1}
        className="sheet"
      >
        {children}
      </div>
    </div>
  );
}
