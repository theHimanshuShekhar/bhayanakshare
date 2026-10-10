// Every view of the screen previews: a name, and how to bring the real app to that state. The
// app is driven the way the UI tests drive it: through the stand-in shell's events (push,
// nearby, shell) and by clicking what the user would click. No component is changed for this.

import { fireEvent, screen, waitFor } from "@testing-library/dom";
import type { Api, Contact, HistoryEntry } from "../src/api";
import { NEARBY_WAIT_MS } from "../src/nearby";
import { fakeApi } from "../src/testApi";
import {
  ASHA,
  BATCH_ID,
  CONTACTS,
  DAD,
  GUEST,
  HISTORY,
  ME,
  MEERA,
  MUM,
  MY_DEVICE_NAME,
  NEARBY,
  SAVE_FOLDER,
  TRIP,
  WORK,
  gib,
  mib,
  narrowed,
  transfer,
  transferId,
  who,
} from "./sample";

/** The stand-in shell, with the events and helpers the tests use. */
export type Device = ReturnType<typeof fakeApi>;

export interface View {
  /** The `?view=` of its URL, and the name of its capture. */
  id: string;
  title: string;
  /** The heading it is listed under on the index. */
  group: string;
  /** Also captured at 640 px wide, which is 1280 px at 200% zoom. */
  narrow?: boolean;
  /** A CSS selector: capture only this part of the page. */
  clip?: string;
  contacts?: Contact[];
  history?: HistoryEntry[];
  /** What the shell answers instead of its defaults. */
  overrides?: Partial<Api>;
  /** Runs before the app is drawn. */
  before?: () => void;
  /** Brings the app, once it is listening, to the state. */
  setup?: (device: Device) => Promise<void>;
}

// ---- Steps the views share ------------------------------------------------------------------

const click = (role: string, name: string | RegExp) =>
  screen.findByRole(role, { name }).then((el) => fireEvent.click(el));
const button = (name: string | RegExp) => click("button", name);
const openTab = (name: string) => button(name);
const type = async (label: string, value: string) =>
  fireEvent.change(await screen.findByLabelText(label), { target: { value } });
/** Waits for the control to be there and not disabled (Settings reads its values first). */
const enabled = (name: string | RegExp) =>
  waitFor(async () => {
    const el = await screen.findByRole("button", { name });
    if ((el as HTMLButtonElement).disabled) throw new Error(`${name} is disabled`);
    return el;
  });
/** The title of the History row of the report Work desktop sent (a sentence, so unlike a Delete button's label). */
const REPORT_ROW = "Quarterly report.pdf from Work desktop";
const pause = (ms = 150) => new Promise((resolve) => setTimeout(resolve, ms));

/** The Contacts' tiles and the Nearby Devices, once Home shows them. */
async function nearby(device: Device) {
  await device.nearby(...NEARBY);
  await screen.findByRole("button", { name: /^Send to Meera's tablet/ });
}

/** Home as a Device with Contacts, and others on the network, shows it. */
const withPeople = { contacts: CONTACTS };
const home = nearby;

const hidden: Partial<Api> = { visibility: () => Promise.resolve("hidden") };

const select = (name: string | RegExp) =>
  click("checkbox", typeof name === "string" ? `Select ${name} to send to several Devices at once` : name);

// ---- The Transfers the list views show ------------------------------------------------------

/** Two files arriving, with the speed they come at. */
async function receiving(device: Device) {
  const quarterly = { name: "Quarterly report.pdf", items: ["Quarterly report.pdf"], size: mib(48) };
  await device.push(transfer(1, "receiver", { kind: "transferring" }, who(WORK), quarterly));
  await device.push({ type: "progress", transfer_id: transferId(1), bytes: mib(25), total: mib(48), at: 100_000 });
  await device.push({ type: "progress", transfer_id: transferId(1), bytes: mib(29.2), total: mib(48), at: 101_000 });
  await device.push(transfer(2, "receiver", { kind: "transferring" }, who(ASHA), TRIP));
  await device.push({ type: "progress", transfer_id: transferId(2), bytes: gib(0.5), total: TRIP.size, at: 100_000 });
  await device.push({ type: "progress", transfer_id: transferId(2), bytes: gib(0.53), total: TRIP.size, at: 101_000 });
}

async function offeredAndPreparing(device: Device) {
  await device.push(transfer(3, "sender", { kind: "offered" }, who(MUM), { name: "Lecture slides.key", items: ["Lecture slides.key"], size: mib(86) }));
  await device.push({ type: "preparing", transfer_id: transferId(3), preparing: true });
  await device.push(transfer(4, "sender", { kind: "offered" }, who(MEERA), { name: "Recipe scans", items: ["Recipe scans"], file_count: 9, size: mib(31) }));
}

const waiting = (device: Device) =>
  device.push(transfer(5, "sender", { kind: "waiting" }, who(MUM), { name: "Lecture slides.key", items: ["Lecture slides.key"], size: mib(86) }));

async function reconnecting(device: Device) {
  await device.push(transfer(6, "receiver", { kind: "reconnecting" }, who(ASHA), TRIP));
  await device.push({ type: "progress", transfer_id: transferId(6), bytes: gib(0.9), total: TRIP.size, at: 100_000 });
}

async function completed(device: Device) {
  await device.push(
    transfer(7, "receiver", { kind: "completed", saved_to: `${SAVE_FOLDER}/Quarterly report.pdf` }, who(WORK), {
      name: "Quarterly report.pdf",
      items: ["Quarterly report.pdf"],
      size: mib(48),
    }),
  );
  await device.push(transfer(8, "sender", { kind: "completed", saved_to: null }, who(ASHA), { name: "Flat keys.jpg", items: ["Flat keys.jpg"], size: mib(3.4) }));
  await device.push(
    transfer(9, "receiver", { kind: "completed", saved_to: null }, who(MUM), {
      kind: "text",
      text: "The wifi password at the cafe is blue-heron-2210",
      name: "",
      items: [],
      file_count: 0,
      size: 52,
    }),
  );
}

async function ended(device: Device) {
  await device.push(
    transfer(10, "sender", { kind: "failed", reason: "The connection to Work desktop was lost and could not be restored." }, who(WORK), {
      name: "build-2026.10.1.zip",
      items: ["build-2026.10.1.zip"],
      size: mib(212),
      skipped_links: 2,
    }),
  );
  await device.push(transfer(11, "sender", { kind: "declined" }, who(MEERA), { name: "Recipe scans", items: ["Recipe scans"], file_count: 9, size: mib(31) }));
  await device.push(transfer(12, "sender", { kind: "expired" }, who(MUM), { name: "Lecture slides.key", items: ["Lecture slides.key"], size: mib(86) }));
  await device.push(transfer(13, "receiver", { kind: "cancelled", by: "sender" }, who(DAD), { name: "Holiday itinerary.docx", items: ["Holiday itinerary.docx"], size: mib(0.2), adjusted_names: 1 }));
}

/** A Batch to four Devices, each at a different point. */
async function batch(device: Device) {
  const agenda = { name: "Team offsite agenda.pdf", items: ["Team offsite agenda.pdf"], size: mib(0.6), batch_id: BATCH_ID };
  await device.push(transfer(21, "sender", { kind: "completed", saved_to: null }, who(ASHA), agenda));
  await device.push(transfer(22, "sender", { kind: "transferring" }, who(WORK), agenda));
  await device.push(transfer(23, "sender", { kind: "declined" }, who(MUM), agenda));
  await device.push(transfer(24, "sender", { kind: "failed", reason: "Meera's tablet could not be reached." }, who(MEERA), agenda));
  await device.push(transfer(25, "sender", { kind: "waiting" }, who(GUEST), agenda));
}

const mismatch = async (device: Device) => {
  await device.push({ type: "version_mismatch", peer: ASHA, peer_name: "Asha's laptop", peer_app_version: "0.2.0", outdated: "this_device" });
  await device.push({ type: "version_mismatch", peer: MEERA, peer_name: "Meera's tablet", peer_app_version: "0.0.9", outdated: "peer" });
};

// ---- The Offers -----------------------------------------------------------------------------

const offerDialog = async (name: string) => {
  await screen.findByRole("dialog", { name });
};

/** The Offer sheet checks the save folder before it says how much room there is. */
const folder = (free: number | null, tooLong = false): Partial<Api> => ({
  checkOffer: (_id, chosen) =>
    Promise.resolve({ folder: chosen ?? SAVE_FOLDER, needed: TRIP.size, free, paths_too_long: tooLong }),
});

const offerOf = (device: Device, over = {}) => device.push(transfer(30, "receiver", { kind: "offered" }, who(ASHA), { ...TRIP, ...over }));

// ---- Settings, History and dialogs ----------------------------------------------------------

async function settings() {
  await openTab("Settings");
  await screen.findByRole("heading", { name: "Diagnostics" });
  await screen.findByText("Version 0.1.0");
  await enabled("Export identity…");
}

const onContacts = () => openTab("Contacts");
const addContact = async () => {
  await onContacts();
  await button("Add Contact…");
};

/** The Add Contact dialog's second step, for a Device that is not a Contact yet. */
async function fingerprintCheck() {
  await addContact();
  await type("Device ID", MEERA);
  await type("Name (optional)", "Meera's tablet");
  await button("Next");
  await offerDialog("Check the Fingerprint");
}

const goodPassword = "correct horse battery";

const importFailure = (kind: string): Partial<Api> => ({
  checkIdentityImport: () => Promise.reject({ kind, message: "details" }),
});

/** Camera access is refused, as when the user has said no in the system settings. */
const refuseCamera = () =>
  Object.defineProperty(navigator, "mediaDevices", {
    configurable: true,
    value: { getUserMedia: () => Promise.reject(new DOMException("denied", "NotAllowedError")) },
  });

/** Home with the firewall hint, which Home shows only after waiting 30 s for a Nearby Device:
 * the wait is cut to a moment. */
const skipNearbyWait = () => {
  const setTimeoutOnce = window.setTimeout.bind(window);
  window.setTimeout = ((handler: TimerHandler, delay?: number, ...args: unknown[]) =>
    setTimeoutOnce(handler, delay === NEARBY_WAIT_MS ? 50 : delay, ...args)) as typeof window.setTimeout;
};

// ---- The views ------------------------------------------------------------------------------

export const VIEWS: View[] = [
  {
    id: "first-run",
    group: "First run",
    title: "First run: Device Name, Visibility and start at login",
    narrow: true,
    overrides: { needsFirstRun: () => Promise.resolve(true) },
    setup: async () => {
      await screen.findByLabelText("Device Name");
    },
  },
  {
    id: "first-run-name-missing",
    group: "First run",
    title: "First run: the name was left empty",
    overrides: { needsFirstRun: () => Promise.resolve(true) },
    setup: async () => {
      await type("Device Name", "");
      await button("Get started");
      await screen.findByText("Enter a name for this Device.");
    },
  },

  {
    id: "home-empty",
    group: "Home",
    title: "Empty: no Contacts and nobody Nearby (yet)",
    setup: async () => {
      await screen.findByText("No Transfers yet.");
    },
  },
  {
    id: "home-empty-firewall-hint",
    group: "Home",
    title: "Empty: nobody Nearby after waiting, with the hint about the firewall",
    before: skipNearbyWait,
    setup: async () => {
      await screen.findByText(/How to allow local discovery/);
    },
  },
  {
    id: "home-empty-hidden",
    group: "Home",
    title: "Empty while Hidden",
    overrides: hidden,
    setup: async () => {
      await screen.findByText(/You're Hidden/);
    },
  },
  {
    id: "home-discovery-unavailable",
    group: "Home",
    title: "Local discovery could not start (port in use)",
    contacts: CONTACTS,
    setup: async (device) => {
      await device.discoveryChanges({ state: "unavailable", reason: "port_in_use" });
      await screen.findByText(/Local discovery could not start/);
    },
  },
  {
    id: "home-discovery-unavailable-hidden",
    group: "Home",
    title: "Local discovery could not start, while Hidden (no network interface)",
    overrides: hidden,
    setup: async (device) => {
      await device.discoveryChanges({ state: "unavailable", reason: "no_interface" });
      await screen.findByText(/people who have your ID can't find this Device/);
    },
  },
  {
    id: "home-populated",
    group: "Home",
    title: "Populated: Contacts and Nearby tiles, two selected, the selection bar",
    narrow: true,
    ...withPeople,
    setup: async (device) => {
      await home(device);
      await select("Asha's laptop");
      await select(/Select Meera's tablet/);
      await screen.findByText("2 Devices selected");
    },
  },
  {
    id: "home-populated-unselected",
    group: "Home",
    title: "Populated: Contacts and Nearby tiles, none selected",
    ...withPeople,
    setup: home,
  },
  {
    id: "home-files-waiting",
    group: "Home",
    title: "Files waiting for a Device to be chosen (second launch or tray)",
    ...withPeople,
    setup: async (device) => {
      await home(device);
      await device.shell({ type: "send_files", paths: ["/home/priya/Pictures/IMG_2041.jpg", "/home/priya/Documents/Lease 2026.pdf"] });
      await select("Work desktop");
      await screen.findByText(/Send IMG_2041.jpg, Lease 2026.pdf/);
    },
  },

  {
    id: "transfers-in-progress",
    group: "Transfer list",
    title: "In progress: progress bars, preparing and offered",
    ...withPeople,
    setup: async (device) => {
      await home(device);
      await offeredAndPreparing(device);
      await receiving(device);
    },
  },
  {
    id: "transfers-waiting",
    group: "Transfer list",
    title: "Waiting for a turn to send",
    ...withPeople,
    setup: async (device) => {
      await home(device);
      await waiting(device);
    },
  },
  {
    id: "transfers-reconnecting",
    group: "Transfer list",
    title: "Reconnecting",
    ...withPeople,
    setup: async (device) => {
      await home(device);
      await reconnecting(device);
    },
  },
  {
    id: "transfers-completed",
    group: "Transfer list",
    title: "Completed: a received file, a sent file and a received text",
    ...withPeople,
    setup: async (device) => {
      await home(device);
      await completed(device);
    },
  },
  {
    id: "transfers-failed",
    group: "Transfer list",
    title: "Failed with a reason, declined, expired (Send again) and cancelled",
    ...withPeople,
    setup: async (device) => {
      await home(device);
      await ended(device);
    },
  },
  {
    id: "transfers-version-mismatch",
    group: "Transfer list",
    title: "Version mismatch notices: this Device is older (Update now) and the other is",
    ...withPeople,
    setup: async (device) => {
      await home(device);
      await mismatch(device);
      await screen.findByRole("button", { name: "Update now" });
    },
  },
  {
    id: "transfers-all",
    group: "Transfer list",
    title: "Everything at once, as a busy day on Home looks",
    narrow: true,
    ...withPeople,
    setup: async (device) => {
      await home(device);
      await mismatch(device);
      await device.shell({ type: "update_available", action: { type: "install", version: "0.2.0" } });
      await ended(device);
      await completed(device);
      await reconnecting(device);
      await offeredAndPreparing(device);
      await receiving(device);
    },
  },

  {
    id: "update-banner",
    group: "Update banner",
    title: "A newer version: Install and restart (Windows installer, AppImage)",
    ...withPeople,
    setup: async (device) => {
      await home(device);
      await device.shell({ type: "update_available", action: { type: "install", version: "0.2.0" } });
      await screen.findByText(/Update available \(version 0.2.0\)/);
    },
  },
  {
    id: "update-banner-package",
    group: "Update banner",
    title: "A newer version: link to the release page (deb, rpm)",
    ...withPeople,
    setup: async (device) => {
      await home(device);
      await device.shell({ type: "update_available", action: { type: "open_page", version: "0.2.0" } });
      await screen.findByText(/Update available \(version 0.2.0\)/);
    },
  },
  {
    id: "update-checking",
    group: "Update banner",
    title: "Update now pressed: looking for the update",
    ...withPeople,
    overrides: { checkForUpdate: () => new Promise(() => {}) },
    setup: async (device) => {
      await home(device);
      await mismatch(device);
      await click("button", "Update now");
      await screen.findByText("Checking for updates…");
    },
  },

  {
    id: "batch-collapsed",
    group: "Batch row",
    title: "A Batch to five Devices, collapsed",
    ...withPeople,
    setup: async (device) => {
      await home(device);
      await batch(device);
      await screen.findByRole("button", { name: /Show each Device/ });
    },
  },
  {
    id: "batch-expanded",
    group: "Batch row",
    title: "A Batch to five Devices, expanded",
    ...withPeople,
    setup: async (device) => {
      await home(device);
      await batch(device);
      await button(/Show each Device/);
      await screen.findByRole("button", { name: /Hide each Device/ });
    },
  },

  {
    id: "offer-files",
    group: "Offer sheet",
    title: "Plain Offer of a folder from a Contact",
    ...withPeople,
    overrides: folder(gib(212)),
    setup: async (device) => {
      await offerOf(device);
      await offerDialog("Incoming files");
      await screen.findByText(SAVE_FOLDER);
    },
  },
  {
    id: "offer-several-items",
    group: "Offer sheet",
    title: "Offer of several files from a Device that is not a Contact",
    overrides: folder(gib(212)),
    setup: async (device) => {
      await device.push(
        transfer(31, "receiver", { kind: "offered" }, who(GUEST), {
          name: "Lease 2026.pdf",
          items: ["Lease 2026.pdf", "Photos of the flat", "Inventory.xlsx"],
          file_count: 38,
          size: mib(122),
          peer_name: "Guest-ThinkPad",
        }),
      );
      await offerDialog("Incoming files");
      await screen.findByText(SAVE_FOLDER);
    },
  },
  {
    id: "offer-text",
    group: "Offer sheet",
    title: "Offer of a text",
    ...withPeople,
    setup: async (device) => {
      await device.push(
        transfer(32, "receiver", { kind: "offered" }, who(MUM), {
          kind: "text",
          text: "Flat 4B, 12 Nehru Road.\nThe doorbell is broken, so knock twice.\nCall me when you're downstairs.",
          name: "",
          items: [],
          file_count: 0,
          size: 96,
        }),
      );
      await offerDialog("Incoming text");
    },
  },
  {
    id: "offer-warnings",
    group: "Offer sheet",
    title: "Offer with warnings: not enough space, paths too long, adjusted names",
    narrow: true,
    ...withPeople,
    overrides: folder(mib(640), true),
    setup: async (device) => {
      await offerOf(device, { adjusted_names: 3 });
      await offerDialog("Incoming files");
      await screen.findByText(/only .* free/);
      await screen.findByText(/too long/i);
    },
  },
  {
    id: "offer-folder-error",
    group: "Offer sheet",
    title: "Offer whose save folder could not be checked",
    ...withPeople,
    overrides: { checkOffer: () => Promise.reject(new Error("The folder is on a drive that is not connected.")) },
    setup: async (device) => {
      await offerOf(device);
      await offerDialog("Incoming files");
      await screen.findByText(/not connected/);
    },
  },

  {
    id: "history-empty",
    group: "History",
    title: "Empty",
    ...withPeople,
    setup: async () => {
      await openTab("History");
      await screen.findByText("No Transfers in your History yet.");
    },
  },
  {
    id: "history-populated",
    group: "History",
    title: "Populated: received, sent, every ending, a Batch and a text",
    narrow: true,
    ...withPeople,
    history: HISTORY,
    setup: async () => {
      await openTab("History");
      await screen.findByText(REPORT_ROW);
    },
  },
  {
    id: "history-batch-expanded",
    group: "History",
    title: "Populated, with the Batch opened",
    ...withPeople,
    history: HISTORY,
    setup: async () => {
      await openTab("History");
      await button(/Show each Device/);
      await screen.findByRole("button", { name: /Hide each Device/ });
    },
  },
  {
    id: "history-filtered",
    group: "History",
    title: "Filtered to one Device and to what was sent",
    ...withPeople,
    history: HISTORY,
    setup: async () => {
      await openTab("History");
      await screen.findByText(REPORT_ROW);
      fireEvent.change(await screen.findByLabelText("Device"), { target: { value: ASHA } });
      fireEvent.change(await screen.findByLabelText("Direction"), { target: { value: "sender" } });
      await waitFor(() => {
        if (screen.queryByText(REPORT_ROW) !== null) throw new Error("not filtered yet");
      });
    },
  },
  {
    id: "history-no-match",
    group: "History",
    title: "A search that matches nothing",
    ...withPeople,
    history: HISTORY,
    setup: async () => {
      await openTab("History");
      await type("Search by item name", "tax return");
      await screen.findByText("No Transfers match.");
    },
  },
  {
    id: "history-load-failed",
    group: "History",
    title: "History could not be read",
    ...withPeople,
    overrides: { history: () => Promise.reject(new Error("db")) },
    setup: async () => {
      await openTab("History");
      await screen.findByText("Could not load your History.");
    },
  },

  {
    id: "contacts-empty",
    group: "Contacts",
    title: "Empty",
    setup: async () => {
      await onContacts();
      await screen.findByText(/No Contacts yet/);
    },
  },
  {
    id: "contacts-populated",
    group: "Contacts",
    title: "Populated: a Nickname, a Device Name, Auto-accept and an unnamed Contact",
    narrow: true,
    ...withPeople,
    setup: async () => {
      await onContacts();
      await screen.findByRole("heading", { name: /Work desktop/ });
    },
  },

  {
    id: "settings",
    group: "Settings",
    title: "All sections",
    narrow: true,
    setup: settings,
  },
  {
    id: "settings-update-found",
    group: "Settings",
    title: "All sections, with a newer version found",
    overrides: { pendingUpdate: () => Promise.resolve({ type: "install", version: "0.2.0" }) },
    setup: async () => {
      await settings();
      await screen.findByText(/Update available/);
    },
  },
  {
    id: "settings-hidden",
    group: "Settings",
    title: "All sections, Visibility set to Hidden",
    overrides: hidden,
    setup: async () => {
      await settings();
      await waitFor(() => {
        if (!(screen.getByLabelText("Hidden") as HTMLInputElement).checked) throw new Error("not read yet");
      });
    },
  },

  {
    id: "my-device-id",
    group: "My Device ID",
    title: "My ID: Fingerprint, Device ID, share link and QR code",
    clip: "section[aria-labelledby=my-id-heading]",
    setup: async () => {
      await screen.findByRole("img", { name: "QR code of the share link" });
    },
  },
  {
    id: "my-device-id-copied",
    group: "My Device ID",
    title: "My ID after copying the link",
    clip: "section[aria-labelledby=my-id-heading]",
    setup: async () => {
      await button("Copy link");
      await screen.findByText("Copied");
    },
  },

  {
    id: "dialog-add-contact",
    group: "Dialogs",
    title: "Add Contact, step 1: the Device ID",
    ...withPeople,
    setup: addContact,
  },
  {
    id: "dialog-add-contact-invalid",
    group: "Dialogs",
    title: "Add Contact, step 1: an ID that is refused",
    ...withPeople,
    setup: async () => {
      await addContact();
      await type("Device ID", "nonsense");
      await screen.findByText(/not a Device ID/);
    },
  },
  {
    id: "dialog-add-contact-filled",
    group: "Dialogs",
    title: "Add Contact, step 1: a valid ID and a name",
    ...withPeople,
    setup: async () => {
      await addContact();
      await type("Device ID", MEERA);
      await type("Name (optional)", "Meera's tablet");
    },
  },
  {
    id: "dialog-add-contact-check",
    group: "Dialogs",
    title: "Add Contact, step 2: check the Fingerprint",
    narrow: true,
    ...withPeople,
    setup: fingerprintCheck,
  },
  {
    id: "dialog-add-contact-link",
    group: "Dialogs",
    title: "Add Contact opened by a share link, with the name it suggests",
    ...withPeople,
    setup: async (device) => {
      await device.openLink(`bhayanakshare://add/${MEERA}?name=${encodeURIComponent("Meera's tablet")}`);
      await offerDialog("Add Contact");
    },
  },
  {
    id: "dialog-add-contact-bad-link",
    group: "Dialogs",
    title: "Add Contact opened by a link that is not a share link",
    ...withPeople,
    setup: async (device) => {
      await device.openLink("https://example.com/");
      await screen.findByText(/not a BhayanakShare share link/);
    },
  },
  {
    id: "dialog-add-contact-nearby",
    group: "Dialogs",
    title: "Save as Contact from a Nearby tile: straight to the Fingerprint",
    ...withPeople,
    setup: async (device) => {
      await home(device);
      await button("Save as Contact: Meera's tablet · Y3LC-7HGA");
      await offerDialog("Check the Fingerprint");
    },
  },
  {
    id: "dialog-qr-scanner",
    group: "Dialogs",
    title: "Add Contact scanning a QR code (the camera the browser gives it; none without one)",
    ...withPeople,
    setup: async () => {
      await addContact();
      await button("Scan QR code…");
      await screen.findByRole("button", { name: "Stop scanning" });
      await pause(500);
    },
  },
  {
    id: "dialog-qr-scanner-refused",
    group: "Dialogs",
    title: "Add Contact scanning, with the camera refused",
    ...withPeople,
    before: refuseCamera,
    setup: async () => {
      await addContact();
      await button("Scan QR code…");
      await screen.findByText(/not allowed to use the camera/);
    },
  },
  {
    id: "dialog-send-id",
    group: "Dialogs",
    title: "Send to ID",
    ...withPeople,
    setup: async () => {
      await button("Send to ID…");
      await offerDialog("Send to ID");
    },
  },
  {
    id: "dialog-send-contact",
    group: "Dialogs",
    title: "Send to a Contact",
    ...withPeople,
    setup: async (device) => {
      await home(device);
      await button("Send to Asha's laptop");
      await offerDialog("Send to Asha's laptop");
    },
  },
  {
    id: "dialog-send-files-waiting",
    group: "Dialogs",
    title: "Send to a Contact, with files already chosen",
    ...withPeople,
    setup: async (device) => {
      await home(device);
      await device.shell({ type: "send_files", paths: ["/home/priya/Pictures/IMG_2041.jpg"] });
      await button("Send to Asha's laptop");
      await screen.findByRole("button", { name: "Send" });
    },
  },
  {
    id: "dialog-send-text",
    group: "Dialogs",
    title: "Write text to send",
    ...withPeople,
    setup: async (device) => {
      await home(device);
      await button("Send to Mum");
      await button("Write text…");
      await type("Text", "Running 10 minutes late, start dinner without me.");
    },
  },
  {
    id: "dialog-remove-contact",
    group: "Dialogs",
    title: "Remove Contact",
    ...withPeople,
    setup: async () => {
      await onContacts();
      await button("Remove Mum from Contacts");
      await screen.findByRole("alertdialog", { name: "Remove Mum?" });
    },
  },
  {
    id: "dialog-clear-history",
    group: "Dialogs",
    title: "Clear History",
    ...withPeople,
    history: HISTORY,
    setup: async () => {
      await openTab("History");
      await button("Clear History…");
      await screen.findByRole("alertdialog", { name: "Clear History?" });
    },
  },
  {
    id: "dialog-quit",
    group: "Dialogs",
    title: "Quit with Transfers in progress",
    ...withPeople,
    setup: async (device) => {
      await device.shell({ type: "confirm_quit", active: 2 });
      await offerDialog("Quit BhayanakShare?");
    },
  },
  {
    id: "dialog-quit-saving",
    group: "Dialogs",
    title: "Quit: saving the Transfers' progress",
    ...withPeople,
    setup: async (device) => {
      await device.shell({ type: "confirm_quit", active: 2 });
      await device.shell({ type: "quitting" });
      await offerDialog("Saving progress…");
    },
  },
  {
    id: "dialog-export-identity",
    group: "Dialogs",
    title: "Export identity",
    setup: async () => {
      await settings();
      await button("Export identity…");
      await offerDialog("Export identity");
    },
  },
  {
    id: "dialog-export-identity-too-short",
    group: "Dialogs",
    title: "Export identity: the password is too short",
    setup: async () => {
      await settings();
      await button("Export identity…");
      await button("Save as…");
      await screen.findByText(/at least/i, { selector: "[role=alert]" });
    },
  },
  {
    id: "dialog-export-identity-saved",
    group: "Dialogs",
    title: "Export identity: saved",
    setup: async () => {
      await settings();
      await button("Export identity…");
      await type("Password", goodPassword);
      await type("Type the password again", goodPassword);
      await button("Save as…");
      await screen.findByText(/Keep it somewhere safe/);
    },
  },
  {
    id: "dialog-import-identity-password",
    group: "Dialogs",
    title: "Import identity: the file's password",
    setup: async () => {
      await settings();
      await button("Import identity…");
      await screen.findByLabelText("Password of this file");
    },
  },
  {
    id: "dialog-import-identity-wrong-password",
    group: "Dialogs",
    title: "Import identity: wrong password",
    overrides: importFailure("wrong_password"),
    setup: async () => {
      await settings();
      await button("Import identity…");
      await type("Password of this file", "not the password");
      await button("Continue");
      await screen.findByText(/Wrong password/);
    },
  },
  {
    id: "dialog-import-identity-replace",
    group: "Dialogs",
    title: "Import identity: the warning before the identity is replaced (two Transfers in progress)",
    overrides: {
      transfersInProgress: () => Promise.resolve(2),
      checkIdentityImport: () => Promise.resolve({ id: MEERA, fingerprint: "Y3LC-7HGA" }),
    },
    setup: async () => {
      await settings();
      await button("Import identity…");
      await type("Password of this file", goodPassword);
      await button("Continue");
      await screen.findByRole("alertdialog", { name: "Replace this Device's identity?" });
    },
  },
];

/** The shell every view starts from: this Device's own ID and name, then the view's changes. */
export function shellFor(view: View): Device {
  return fakeApi(
    {
      myId: () => Promise.resolve({ id: ME, fingerprint: `${ME.slice(0, 4)}-${ME.slice(4, 8)}` }),
      deviceName: () => Promise.resolve(MY_DEVICE_NAME),
      saveFolder: () => Promise.resolve(SAVE_FOLDER),
      ...view.overrides,
      ...(view.history !== undefined && !view.overrides?.history
        ? { history: (peer, direction, search) => Promise.resolve(narrowed(view.history!, peer, direction, search)) }
        : {}),
    },
    view.contacts ?? [],
  );
}

/** The views under their headings, in the order they are listed. */
export function groups(): [string, View[]][] {
  const found = new Map<string, View[]>();
  for (const view of VIEWS) found.set(view.group, [...(found.get(view.group) ?? []), view]);
  return [...found];
}
