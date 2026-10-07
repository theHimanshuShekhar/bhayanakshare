import { useEffect } from "react";

/**
 * When the control that has focus is taken out of the page (Cancel on a Transfer that is now
 * cancelled, Delete on a History entry, the row of a Contact that was removed), the browser drops
 * focus on the page itself and the next Tab starts again from the top. This puts it on the
 * heading of the section the control was in instead, so a keyboard or screen reader user carries
 * on from where they were.
 *
 * Only for the keyboard: a control that was reached by key is remembered, and one that was clicked
 * is not. Someone using the mouse has no place to carry on from, and a jump to a heading would only
 * scroll the page under them. And only a control that was removed is dealt with: focus the user
 * moved away themselves is left where it is. A sheet gives focus back to what opened it; if that is what
 * went (the Remove button of a Contact that was removed), this is what catches it.
 */
export function useFocusRescue() {
  useEffect(() => {
    let last: HTMLElement | null = null;
    let section: Element | null = null;
    // Whether the last thing the user did was press a key, not point.
    let keyboard = false;

    const focused = (e: FocusEvent) => {
      const target = e.target;
      // What a sheet takes focus to is not remembered: what opened it is, for when that goes.
      if (!(target instanceof HTMLElement) || target.closest(".sheet, [data-announcer]")) return;
      last = keyboard ? target : null;
      section = target.closest("section");
    };
    const pressed = () => {
      keyboard = true;
    };
    // A click puts focus where the user chose, which is no business of ours.
    const pointed = () => {
      keyboard = false;
      last = null;
    };

    const observer = new MutationObserver(() => {
      if (last === null || last.isConnected) return;
      const stray = document.activeElement === null || document.activeElement === document.body;
      const gone = last;
      last = null;
      if (!stray || gone.isConnected) return;
      const heading = (section?.isConnected ? section : document.querySelector("main"))?.querySelector<HTMLElement>(
        "h2, h3",
      );
      if (heading) {
        // Not in the tab order, but able to take focus from here.
        heading.tabIndex = -1;
        heading.focus();
      }
    });

    document.addEventListener("focusin", focused);
    document.addEventListener("keydown", pressed, true);
    document.addEventListener("pointerdown", pointed, true);
    observer.observe(document.body, { childList: true, subtree: true });
    return () => {
      document.removeEventListener("focusin", focused);
      document.removeEventListener("keydown", pressed, true);
      document.removeEventListener("pointerdown", pointed, true);
      observer.disconnect();
    };
  }, []);
}
