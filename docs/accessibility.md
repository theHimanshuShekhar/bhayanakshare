# Accessibility

The spec ([`docs/spec/v1.md`](spec/v1.md), section 10) asks for: full keyboard operation, visible focus, screen-reader labels on every control, progress announced through ARIA live regions, and no information conveyed by colour alone. This page says how that is built, what is checked by the tests, what has to be checked by hand before a release, and what is not covered.

## How it is built

- **Sheets.** Every dialog and sheet (Send, Add Contact, Remove Contact, Clear History, Quit, Export and Import identity, and the Offer sheet) is one `Sheet` (`ui/src/Sheet.tsx`): it moves focus inside when it opens, keeps Tab and Shift+Tab inside, closes on Escape where it may, and gives focus back to what opened it. The Offer sheet is not closed by Escape: an Offer is answered, not dismissed. The page behind a sheet is `inert` where it can be; the Tab trap is what holds for the two sheets inside Settings and for one sheet over another (an Offer arriving over Send, whose sheet is made inert).
- **Focus is never dropped on the page.** A step that replaces the control that had focus puts it on the next one (Scan and Stop scanning, Write text and Back, the selection bar going away). When a control is taken out of the page (Cancel on a Transfer that is now cancelled, Delete in History, a removed Contact's row), `focusRescue.ts` puts focus on the heading of its section.
- **Visible focus.** One `:focus-visible` rule in `styles.css` draws a ring in the text colour with a ring in the page colour round it, so it shows on any background in light and dark. Nothing in the stylesheet removes the outline.
- **Names.** Every control has a name: a visible label, or an `aria-label` from `i18n.ts` that contains the visible words (the axe tests check that, for people who speak the words they see). A tile is named "Send to Mum", and described by what else it says (Contact, the Fingerprint, Nearby, Selected). The QR code is an `img` with a label.
- **One announcer.** `ui/src/Announcer.tsx` is the one place the app speaks to a screen reader from on its own: a visually hidden `role="log"` (polite, additions only) that sits outside everything the app makes inert. What goes in it is decided by `announcements()` in `ui/src/announce.ts`, a pure function of the Transfers before and after:
  - a change of what a Transfer's row says is said once (Offer received, accepted, receiving, received, declined, expired, cancelled, preparing, reconnecting, and failed with its reason); Saving is not, as Received follows at once;
  - progress is said at 25%, 50% and 75% (never 100%: Received says that) and not twice within 10 seconds for one Transfer; a step reached too soon is said, as the step then reached, by the first report after the 10 seconds;
  - a Batch is said once, when its last Receiver is done ("2 of 3 delivered, 1 declined"), not Receiver by Receiver;
  - a copy that worked or failed is said through it too.

  Rows and Batch rows have no live region of their own. Errors that need the user to do something stay `role="alert"` where they are shown. A few messages that answer something the user just did stay `role="status"` beside it (a saved setting, a check for updates, the selection count); so do the hints that appear because something happened (an update, a version mismatch, the firewall hint after 30 seconds). A standing hint (Hidden) and a History row that says its file is gone are plain text.
- **Not by colour alone.** The stylesheet sets no colour for any state. States are words (Contact, Nearby, Selected, each Transfer state, Hidden, "Expires in"), a native control that shows its own state (checkbox, radio, progress with its percentage), or a border (a selected tile is thick and double, a refused field dashed, a Contact's badge boxed, the current tab bold and underlined).

## What the tests check

All in `ui`, run by `pnpm --filter ui test` (and so by `pnpm test` and CI).

- **axe** (`a11y.test.tsx`, `axe-core` run directly: `vitest-axe` has had no release since January 2025). It runs every rule axe applies by default (WCAG 2 A and AA, and its best practices) on: first run; Home with Contacts, Nearby Devices, a selection, queued files, every kind of Transfer row, the Hidden hint, an update and a version notice; the Offer sheet for files, for a text and with no room; Batch rows, closed and open; History with entries, a Batch, and when it cannot be read; Contacts, empty and with a failure; Settings; and each dialog in each of its steps. Two canary tests plant a violation to show that a clean result means something.
  - **Colour contrast is switched off** in these tests: jsdom does no layout or painting, so axe cannot work out the colours. It is on the manual checklist below.
- **Keyboard** (`keyboard.test.tsx`, `@testing-library/user-event`): moving between tabs; selecting tiles with Space and sending the selection; opening a tile and sending from its dialog; opening, trapping Tab in and closing every dialog with focus returned; accepting, declining and not escaping an Offer, with focus returned; opening and cancelling a Batch; first run filled in and submitted; History filters and delete; Contacts Nickname, Auto-accept and Remove; Settings with arrow keys on the Visibility radios; and each place focus used to be dropped.
- **Announcements** (`announce.test.ts` for the decision function; `announcer.test.tsx` through the app): what is said for each state, the progress steps and gap, Batches, and that there is one live region and the words leave it after a while.

## Before a release, by hand

Automated checks cannot say whether it is usable. On a build of each platform released (Linux AppImage at least):

1. **Keyboard only.** Unplug or ignore the mouse. From a fresh start: finish first run; reach every tab; send a file to a Contact and to a pasted ID; select two tiles and send to both; accept and decline an Offer; open each dialog and leave it with Escape (focus must return to what opened it, or to a heading when that is gone); add a Contact; change a setting in each Settings section; open the Export and Import dialogs. Tab must never reach anything hidden behind a dialog, and a focus ring must always be visible.
2. **Screen reader.** Orca on Linux (this is the webview there: WebKitGTK) and VoiceOver on macOS. Check: the tab bar and the page heading are announced when changing tab; a tile is read as its name and its state; a dialog is announced with its name when it opens; an incoming Offer is announced, and Accept and Decline are reachable; progress is announced at coarse steps and not on every update; a failed Transfer says why; a Batch is announced once when done; the QR code is read as an image with its label; nothing is read twice.
3. **Zoom to 200%** (and a window about 640 CSS pixels wide): nothing is cut off or needs scrolling sideways, and the sheets can be scrolled to their buttons.
4. **Contrast.** The automated check is off, so look: text 4.5:1 (including the dimmed hints, which are at 80% opacity), the focus ring and the borders of controls 3:1, in the light and the dark theme, and with the system's forced-colours or high-contrast mode on.
5. **Reduced motion** is not needed: the UI has no animation. If any is added, honour `prefers-reduced-motion`.

## Known gaps

- Nothing here has been run with a real screen reader. The tests use jsdom, and the stylesheet was looked at once in headless Chromium (light theme: focus ring, selected tile, reflow at 640 pixels wide) and not in WebKitGTK, WebView2 or WKWebView. How each reads a `log` region and a sheet over an inert page is on the manual list above.
- The dark theme's focus ring was not looked at; it uses the system colours (`CanvasText`, `Canvas`), which follow the theme.
- Contrast has not been measured (see above).
- Each Transfer's progress bar is a native `progress` with a label; a screen reader moving through the page reads its value when it reaches it. It is not announced on each change on purpose.
- A Contact's or History entry's removal puts focus on its section's heading, not on the neighbouring row. That is the choice that cannot be wrong, not the best one.
- A Batch's Receivers are not announced one at a time. A sender who wants to know who declined opens the row (or History) once the Batch is announced as done.
- The Offer countdown (`role="timer"`) is deliberately not live; it is not announced as it counts down.
