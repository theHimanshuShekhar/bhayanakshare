import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import "./heavyTests";
import type { Contact } from "./api";
import type { HistoryEntry, TransferRecord, TransferState } from "./bindings";
import { BATCH, EXPIRES_AT, MY_ID, PEER_ID, TRANSFER, contact, fakeApi } from "./testApi";

// The camera is not tested here (QrScanner.test.tsx covers how it is read).
vi.mock("./QrScanner", () => ({ QrScanner: () => <p>Camera</p> }));

afterEach(cleanup);

// user-event walks a radio group with the arrow keys using CSS.escape, which jsdom lacks.
beforeAll(() => {
  if (typeof window.CSS?.escape !== "function") {
    Object.defineProperty(window, "CSS", {
      configurable: true,
      value: { escape: (s: string) => s.replace(/[^\w-]/g, "\\$&") },
    });
  }
});

const DAD = "Q2WERTYU" + "C".repeat(44);
type User = ReturnType<typeof userEvent.setup>;
type Device = ReturnType<typeof fakeApi>;

const contacts = (): Contact[] => [
  contact({ id: PEER_ID, nickname: "Mum", added_at: 1 }),
  contact({ id: DAD, nickname: "Dad", added_at: 2 }),
];

/** Renders the app and waits until it shows My ID, or first run. */
async function start(device: Device = fakeApi({}, contacts())) {
  const user = userEvent.setup({ delay: null });
  render(<App api={device.api} />);
  await screen.findByRole("heading", { name: /^(My ID|Welcome to BhayanakShare)$/ });
  return { user, device };
}

/** Presses Tab until `target` has focus, as a person who cannot point would; fails if it never does. */
async function tabTo(user: User, target: HTMLElement, key = "{Tab}") {
  for (let i = 0; i < 60 && document.activeElement !== target; i++) await user.keyboard(key);
  expect(document.activeElement).toBe(target);
}

const button = (name: string | RegExp) => screen.getByRole("button", { name });

/** Presses Tab (and Shift+Tab) round and round: focus must never leave `dialog`. */
async function expectTrapped(user: User, dialog: HTMLElement) {
  expect(dialog.contains(document.activeElement)).toBe(true);
  for (let i = 0; i < 12; i++) {
    await user.tab();
    expect(dialog.contains(document.activeElement), `Tab ${i + 1} left the dialog`).toBe(true);
  }
  for (let i = 0; i < 12; i++) {
    await user.tab({ shift: true });
    expect(dialog.contains(document.activeElement), `Shift+Tab ${i + 1} left the dialog`).toBe(true);
  }
}

function row(device: Device, n: number, role: "sender" | "receiver", state: TransferState, over: Record<string, unknown> = {}) {
  return device.push({
    type: "transfer",
    transfer_id: String(n).repeat(32),
    role,
    peer: PEER_ID,
    peer_name: null,
    kind: "files",
    text: null,
    name: "photo.jpg",
    items: ["photo.jpg"],
    file_count: 1,
    skipped_links: 0,
    adjusted_names: 0,
    batch_id: null,
    size: 2048,
    expires_at: EXPIRES_AT,
    state,
    ...over,
  });
}

function record(over: Partial<TransferRecord> = {}): TransferRecord {
  return {
    id: TRANSFER,
    role: "receiver",
    peer: PEER_ID,
    peer_name: "Laptop",
    name: "photo.jpg",
    kind: "files",
    size: 2048,
    text: null,
    items: ["photo.jpg"],
    file_count: 1,
    skipped_links: 0,
    adjusted_names: 0,
    batch_id: null,
    state: { kind: "failed", reason: "Gone." },
    created_at: 1_700_000_000_000,
    accepted_at: null,
    updated_at: 1_700_000_060_000,
    ...over,
  };
}

describe("keyboard: tabs", () => {
  it("moves between the tabs with Tab and Enter or Space, keeping focus on the tab", async () => {
    const { user } = await start();
    const history = button("History");
    await tabTo(user, history);
    await user.keyboard("{Enter}");
    expect(history.getAttribute("aria-current")).toBe("page");
    expect(await screen.findByRole("heading", { name: "History" })).toBeTruthy();
    expect(document.activeElement).toBe(history);

    await user.tab({ shift: true });
    expect(document.activeElement).toBe(button("Home"));
    await user.keyboard(" ");
    expect(button("Home").getAttribute("aria-current")).toBe("page");
    expect(screen.getByRole("heading", { name: "Devices" })).toBeTruthy();
  });

  it("puts the tabs before the page, and reaches every tab", async () => {
    const { user } = await start();
    for (const name of ["Home", "History", "Contacts", "Settings"]) await tabTo(user, button(name));
  });
});

describe("keyboard: Home", () => {
  it("selects a tile with Space and sends to the selection from the keyboard", async () => {
    const { user, device } = await start();
    const mum = screen.getByRole("checkbox", { name: /Select Mum/ });
    await tabTo(user, mum);
    await user.keyboard(" ");
    expect((mum as HTMLInputElement).checked).toBe(true);
    expect(screen.getByText("1 Device selected")).toBeTruthy();

    await tabTo(user, screen.getByRole("checkbox", { name: /Select Dad/ }));
    await user.keyboard(" ");
    expect(screen.getByText("2 Devices selected")).toBeTruthy();

    await tabTo(user, button("Choose files…"));
    await user.keyboard("{Enter}");
    await waitFor(() => expect(device.api.sendBatch).toHaveBeenCalledWith([PEER_ID, DAD], ["/tmp/photo.jpg"]));
  });

  it("shows a selected tile as text, not just as a different look", async () => {
    const { user } = await start();
    const tile = button("Send to Mum");
    expect(within(tile).queryByText("Selected")).toBeNull();
    await tabTo(user, screen.getByRole("checkbox", { name: /Select Mum/ }));
    await user.keyboard(" ");
    expect(within(tile).getByText("Selected")).toBeTruthy();
  });

  it("opens a tile with Enter, and sends from its dialog", async () => {
    const { user, device } = await start();
    await tabTo(user, button("Send to Dad"));
    await user.keyboard("{Enter}");
    const dialog = screen.getByRole("dialog", { name: "Send to Dad" });
    expect((screen.getByLabelText("Device ID") as HTMLInputElement).value).toBe(DAD);
    await tabTo(user, within(dialog).getByRole("button", { name: "Choose files…" }));
    await user.keyboard("{Enter}");
    await waitFor(() => expect(device.api.sendFiles).toHaveBeenCalledWith(DAD, ["/tmp/photo.jpg"]));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(document.activeElement).toBe(button("Send to Dad"));
  });

  it("describes a tile by its state as well as its name", async () => {
    const { device } = await start();
    await device.nearby({ id: PEER_ID, name: "Mum's phone" });
    const tile = await screen.findByRole("button", { name: "Send to Mum" });
    const described = (tile.getAttribute("aria-describedby") ?? "")
      .split(" ")
      .map((id) => document.getElementById(id)?.textContent)
      .join(" ");
    expect(described).toContain("Contact");
    expect(described).toContain("Nearby");
    expect(described).toContain("K3QF-7XNA");
  });

  it("lets a Batch be opened and cancelled from the keyboard", async () => {
    const { user, device } = await start();
    await row(device, 1, "sender", { kind: "transferring" }, { batch_id: BATCH });
    await row(device, 2, "sender", { kind: "transferring" }, { batch_id: BATCH, peer: DAD });
    const toggle = await screen.findByRole("button", { name: /Show each Device/ });
    await tabTo(user, toggle);
    await user.keyboard("{Enter}");
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    expect(document.getElementById(toggle.getAttribute("aria-controls")!)).toBeTruthy();

    await user.tab();
    await user.keyboard(" ");
    expect(device.api.cancelBatch).toHaveBeenCalledWith(BATCH);
  });
});

describe("keyboard: focus is never dropped on the page", () => {
  it("goes to the Transfers heading once a selection is sent, and Devices once it is cleared", async () => {
    const { user, device } = await start();
    await tabTo(user, screen.getByRole("checkbox", { name: /Select Mum/ }));
    await user.keyboard(" ");
    await tabTo(user, button("Choose files…"));
    await user.keyboard("{Enter}");
    await waitFor(() => expect(device.api.sendFiles).toHaveBeenCalled());
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("heading", { name: "Transfers" })));

    await tabTo(user, screen.getByRole("checkbox", { name: /Select Dad/ }));
    await user.keyboard(" ");
    await tabTo(user, button("Clear selection"));
    await user.keyboard("{Enter}");
    expect(document.activeElement).toBe(screen.getByRole("heading", { name: "Devices" }));
  });

  it("goes back to Write text… from the text step of the selection bar and of Send", async () => {
    const { user } = await start();
    await tabTo(user, screen.getByRole("checkbox", { name: /Select Mum/ }));
    await user.keyboard(" ");
    await tabTo(user, button("Write text…"));
    await user.keyboard("{Enter}");
    await tabTo(user, button("Back"));
    await user.keyboard("{Enter}");
    expect(document.activeElement).toBe(button("Write text…"));
    await user.click(button("Clear selection"));

    await tabTo(user, button("Send to Dad"));
    await user.keyboard("{Enter}");
    const dialog = screen.getByRole("dialog", { name: "Send to Dad" });
    await user.click(within(dialog).getByRole("button", { name: "Write text…" }));
    await user.click(within(dialog).getByRole("button", { name: "Back" }));
    expect(document.activeElement).toBe(within(dialog).getByRole("button", { name: "Write text…" }));
  });

  it("follows Scan QR code… to Stop scanning and back, with Escape still closing Add Contact", async () => {
    const { user } = await start();
    await user.click(button("Contacts"));
    await tabTo(user, await screen.findByRole("button", { name: "Add Contact…" }));
    await user.keyboard("{Enter}");
    await tabTo(user, button("Scan QR code…"));
    await user.keyboard("{Enter}");
    expect(document.activeElement).toBe(button("Stop scanning"));
    await user.keyboard("{Enter}");
    expect(document.activeElement).toBe(button("Scan QR code…"));
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("starts Import identity's own-identity step on Close, which Escape also leaves", async () => {
    const same = fakeApi({
      checkIdentityImport: () => Promise.resolve({ id: MY_ID, fingerprint: "AAAA-AAAA" }),
    });
    const { user } = await start(same);
    await user.click(button("Settings"));
    const open = await screen.findByRole("button", { name: "Import identity…" });
    await waitFor(() => expect((open as HTMLButtonElement).disabled).toBe(false));
    await tabTo(user, open);
    await user.keyboard("{Enter}");
    await user.type(await screen.findByLabelText("Password of this file"), "correct horse{Enter}");
    const dialog = screen.getByRole("dialog", { name: "Import identity" });
    await waitFor(() => expect(document.activeElement).toBe(within(dialog).getByRole("button", { name: "Close" })));
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(open);
  });
});

describe("keyboard: when the control that has focus goes", () => {
  it("lands on the heading of its section after Cancel on a Transfer, which is then gone", async () => {
    const { user, device } = await start();
    await row(device, 1, "sender", { kind: "offered" });
    const cancel = await screen.findByRole("button", { name: "Cancel photo.jpg" });
    await tabTo(user, cancel);
    await user.keyboard("{Enter}");
    expect(device.api.cancelTransfer).toHaveBeenCalled();
    await row(device, 1, "sender", { kind: "cancelled", by: "sender" });
    expect(screen.queryByRole("button", { name: "Cancel photo.jpg" })).toBeNull();
    expect(document.activeElement).toBe(screen.getByRole("heading", { name: "Transfers" }));
  });

  it("lands on the History heading after a Transfer is deleted from it", async () => {
    const device = fakeApi({}, contacts());
    const entries: HistoryEntry[] = [{ kind: "transfer", transfer: { record: record(), saved_present: null } }];
    device.setHistory(entries);
    const { user } = await start(device);
    await user.click(button("History"));
    await tabTo(user, await screen.findByRole("button", { name: /^Delete .* from History$/ }));
    device.setHistory([]);
    await user.keyboard("{Enter}");
    await waitFor(() => expect(screen.queryByRole("button", { name: /^Delete .* from History$/ })).toBeNull());
    expect(document.activeElement).toBe(screen.getByRole("heading", { name: "History" }));
  });

  it("lands on the Contacts heading after a Contact is removed through its dialog", async () => {
    const { user } = await start();
    await user.click(button("Contacts"));
    await tabTo(user, await screen.findByRole("button", { name: "Remove Mum from Contacts" }));
    await user.keyboard("{Enter}");
    await tabTo(user, button("Remove"));
    await user.keyboard("{Enter}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(document.activeElement).toBe(screen.getByRole("heading", { name: "Contacts" }));
  });

  it("leaves focus alone when the control was clicked with the mouse and then goes", async () => {
    const { user, device } = await start();
    await row(device, 1, "sender", { kind: "offered" });
    const cancel = await screen.findByRole("button", { name: "Cancel photo.jpg" });
    await user.click(cancel);
    expect(device.api.cancelTransfer).toHaveBeenCalled();
    await row(device, 1, "sender", { kind: "cancelled", by: "sender" });
    expect(cancel.isConnected).toBe(false);
    expect(document.activeElement).toBe(document.body);
  });

  it("does the same for a Delete in History that was clicked", async () => {
    const device = fakeApi({}, contacts());
    device.setHistory([{ kind: "transfer", transfer: { record: record(), saved_present: null } }]);
    const { user } = await start(device);
    await user.click(button("History"));
    const remove = await screen.findByRole("button", { name: /^Delete .* from History$/ });
    device.setHistory([]);
    await user.click(remove);
    await waitFor(() => expect(remove.isConnected).toBe(false));
    expect(document.activeElement).toBe(document.body);
  });

  it("still catches focus when the keyboard is used after a click", async () => {
    const { user, device } = await start();
    await row(device, 1, "sender", { kind: "offered" });
    await user.click(screen.getByRole("heading", { name: "Transfers" }));
    await tabTo(user, await screen.findByRole("button", { name: "Cancel photo.jpg" }));
    await user.keyboard("{Enter}");
    await row(device, 1, "sender", { kind: "cancelled", by: "sender" });
    expect(document.activeElement).toBe(screen.getByRole("heading", { name: "Transfers" }));
  });
});

describe("keyboard: dialogs", () => {
  it("opens with focus inside, closes with Escape and gives focus back to the opener", async () => {
    const { user } = await start();
    const opener = button("Send to ID…");
    await tabTo(user, opener);
    await user.keyboard("{Enter}");
    expect(document.activeElement).toBe(screen.getByLabelText("Device ID"));
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(opener);
  });

  it("keeps focus in Send, whatever number of times Tab is pressed", async () => {
    const { user } = await start();
    await tabTo(user, button("Send to ID…"));
    await user.keyboard("{Enter}");
    await user.type(screen.getByLabelText("Device ID"), PEER_ID);
    await expectTrapped(user, screen.getByRole("dialog"));
  });

  it("keeps focus in Add Contact, in both of its steps", async () => {
    const { user } = await start();
    await user.click(button("Contacts"));
    await tabTo(user, await screen.findByRole("button", { name: "Add Contact…" }));
    await user.keyboard("{Enter}");
    await expectTrapped(user, screen.getByRole("dialog"));
    await user.type(screen.getByLabelText("Device ID"), DAD);
    await user.click(button("Next"));
    await expectTrapped(user, screen.getByRole("dialog", { name: "Check the Fingerprint" }));
  });

  it("keeps focus in Remove Contact and Clear History, which start on the safe answer", async () => {
    const { user, device } = await start();
    device.setHistory([{ kind: "transfer", transfer: { record: record(), saved_present: null } }]);
    await user.click(button("Contacts"));
    await tabTo(user, await screen.findByRole("button", { name: "Remove Mum from Contacts" }));
    await user.keyboard("{Enter}");
    const remove = screen.getByRole("alertdialog", { name: "Remove Mum?" });
    expect(document.activeElement).toBe(within(remove).getByRole("button", { name: "Keep" }));
    await expectTrapped(user, remove);
    await user.keyboard("{Escape}");
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Remove Mum from Contacts" }));

    await user.click(button("History"));
    await tabTo(user, await screen.findByRole("button", { name: "Clear History…" }));
    await user.keyboard("{Enter}");
    const clear = screen.getByRole("alertdialog", { name: "Clear History?" });
    expect(document.activeElement).toBe(within(clear).getByRole("button", { name: "Keep" }));
    await expectTrapped(user, clear);
    await user.keyboard("{Escape}");
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Clear History…" }));
  });

  it("keeps focus in Quit, which stays until Quit or Keep running is chosen", async () => {
    const { user, device } = await start();
    await device.shell({ type: "confirm_quit", active: 1 });
    const dialog = screen.getByRole("dialog", { name: "Quit BhayanakShare?" });
    expect(document.activeElement).toBe(within(dialog).getByRole("button", { name: "Keep running" }));
    await expectTrapped(user, dialog);
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("keeps focus in Export and Import identity, which sit inside Settings", async () => {
    const { user } = await start();
    await user.click(button("Settings"));
    const exportButton = await screen.findByRole("button", { name: "Export identity…" });
    await waitFor(() => expect((exportButton as HTMLButtonElement).disabled).toBe(false));
    await tabTo(user, exportButton);
    await user.keyboard("{Enter}");
    const dialog = screen.getByRole("dialog", { name: "Export identity" });
    expect(document.activeElement).toBe(within(dialog).getByLabelText("Password"));
    await expectTrapped(user, dialog);
    await user.keyboard("{Escape}");
    expect(document.activeElement).toBe(exportButton);

    await tabTo(user, button("Import identity…"));
    await user.keyboard("{Enter}");
    await expectTrapped(user, await screen.findByRole("dialog", { name: "Import identity" }));
  });

  it("does not let Tab reach a dialog that an Offer has been put over", async () => {
    const { user, device } = await start();
    await tabTo(user, button("Send to ID…"));
    await user.keyboard("{Enter}");
    const send = screen.getByRole("dialog", { name: "Send to ID" });
    await row(device, 1, "receiver", { kind: "offered" });
    const offer = await screen.findByRole("dialog", { name: "Incoming files" });
    await expectTrapped(user, offer);
    expect(send.contains(document.activeElement)).toBe(false);
  });
});

describe("keyboard: a sheet's first focus", () => {
  /** What took focus, in order, while `open` runs, among what is inside `inside`. */
  async function focusedWhile(open: () => unknown, inside: string) {
    const seen: string[] = [];
    const record = (e: FocusEvent) => {
      const el = e.target as HTMLElement;
      if (el.closest(inside)) seen.push(el.textContent || el.id || el.tagName);
    };
    document.addEventListener("focusin", record);
    try {
      await open();
    } finally {
      document.removeEventListener("focusin", record);
    }
    return seen;
  }

  it("lands on the safe answer of Remove Contact, never passing the destructive one", async () => {
    const { user } = await start();
    await user.click(button("Contacts"));
    const remove = await screen.findByRole("button", { name: "Remove Mum from Contacts" });
    const seen = await focusedWhile(() => user.click(remove), "[role=alertdialog]");
    expect(seen).toEqual(["Keep"]);
  });

  it("lands on the safe answer of Clear History and of Quit, the same way", async () => {
    const { user, device } = await start();
    await user.click(button("History"));
    const clear = await screen.findByRole("button", { name: "Clear History…" });
    expect(await focusedWhile(() => user.click(clear), "[role=alertdialog]")).toEqual(["Keep"]);
    await user.keyboard("{Escape}");
    await user.click(button("Home"));
    const seen = await focusedWhile(() => device.shell({ type: "confirm_quit", active: 1 }), "[role=dialog]");
    expect(seen).toEqual(["Keep running"]);
  });

  it("lands on the Device ID field of Send, and on the first field of Export and Add Contact", async () => {
    const { user } = await start();
    expect(await focusedWhile(() => user.click(button("Send to ID…")), "[role=dialog]")).toEqual(["send-to"]);
    await user.keyboard("{Escape}");
    await user.click(button("Contacts"));
    const add = await screen.findByRole("button", { name: "Add Contact…" });
    expect(await focusedWhile(() => user.click(add), "[role=dialog]")).toEqual(["add-contact-id"]);
  });
});

describe("keyboard: Offers waiting in a queue", () => {
  it("gives focus back to where it was before the first, once the last has been answered", async () => {
    const { user, device } = await start();
    const opener = button("Contacts");
    await tabTo(user, opener);

    await row(device, 1, "receiver", { kind: "offered" });
    await row(device, 2, "receiver", { kind: "offered" }, { name: "b.txt", items: ["b.txt"] });
    const heading = () => screen.getByRole("heading", { name: "Incoming files" });
    expect(document.activeElement).toBe(heading());

    // The first is answered; the second takes its place on the same sheet, heading first.
    await tabTo(user, button("Accept"));
    await user.keyboard("{Enter}");
    expect(device.api.acceptOffer).toHaveBeenLastCalledWith("1".repeat(32), null);
    await row(device, 1, "receiver", { kind: "accepted" });
    expect(screen.getByRole("dialog", { name: "Incoming files" })).toBeTruthy();
    expect(document.activeElement).toBe(heading());

    await tabTo(user, button("Decline"));
    await user.keyboard("{Enter}");
    expect(device.api.declineOffer).toHaveBeenCalled();
    await row(device, 2, "receiver", { kind: "declined" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(opener);
  });
});

describe("keyboard: the Offer sheet", () => {
  it("takes focus on its heading, accepts with Enter, and gives focus back", async () => {
    const { user, device } = await start();
    const contactsTab = button("Contacts");
    await tabTo(user, contactsTab);

    await row(device, 1, "receiver", { kind: "offered" });
    const sheet = await screen.findByRole("dialog", { name: "Incoming files" });
    expect(document.activeElement).toBe(within(sheet).getByRole("heading", { name: "Incoming files" }));

    await user.tab();
    expect(document.activeElement).toBe(within(sheet).getByRole("button", { name: /Change the save folder/ }));
    await user.tab();
    const accept = within(sheet).getByRole("button", { name: "Accept" });
    expect(document.activeElement).toBe(accept);
    await user.keyboard("{Enter}");
    expect(device.api.acceptOffer).toHaveBeenCalledWith("1".repeat(32), null);

    await row(device, 1, "receiver", { kind: "accepted" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(contactsTab);
  });

  it("declines with the keyboard too", async () => {
    const { user, device } = await start();
    await row(device, 1, "receiver", { kind: "offered" });
    await screen.findByRole("dialog", { name: "Incoming files" });
    await tabTo(user, button("Decline"));
    await user.keyboard(" ");
    expect(device.api.declineOffer).toHaveBeenCalled();
  });

  it("is not closed by Escape: an Offer is answered, not dismissed", async () => {
    const { user, device } = await start();
    await row(device, 1, "receiver", { kind: "offered" });
    await screen.findByRole("dialog", { name: "Incoming files" });
    await user.keyboard("{Escape}");
    expect(screen.getByRole("dialog", { name: "Incoming files" })).toBeTruthy();
    expect(device.api.declineOffer).not.toHaveBeenCalled();
  });
});

describe("keyboard: first run", () => {
  it("is filled in and finished without the pointer", async () => {
    const device = fakeApi({ needsFirstRun: vi.fn(() => Promise.resolve(true)) });
    const { user } = await start(device);
    const name = await screen.findByLabelText("Device Name");
    await waitFor(() => expect(document.activeElement).toBe(name));
    await user.clear(name);
    await user.type(name, "Desk");
    // Tab stops on the chosen radio of a group only; the arrow keys move within it.
    await tabTo(user, screen.getByLabelText("People who have my ID"));
    await user.keyboard("{ArrowDown}");
    expect((screen.getByLabelText("Hidden") as HTMLInputElement).checked).toBe(true);
    await tabTo(user, button("Get started"));
    await user.keyboard("{Enter}");
    await waitFor(() => expect(device.api.finishFirstRun).toHaveBeenCalled());
    expect(device.api.setDeviceName).toHaveBeenCalledWith("Desk");
    expect(device.api.setVisibility).toHaveBeenCalledWith("hidden");
    expect(await screen.findByRole("heading", { name: "My ID" })).toBeTruthy();
  });

  it("is submitted by Enter in the name field", async () => {
    const device = fakeApi({ needsFirstRun: vi.fn(() => Promise.resolve(true)) });
    const { user } = await start(device);
    await screen.findByLabelText("Device Name");
    await user.keyboard("{Enter}");
    await waitFor(() => expect(device.api.finishFirstRun).toHaveBeenCalled());
  });
});

describe("keyboard: History", () => {
  it("filters, deletes and clears from the keyboard", async () => {
    const device = fakeApi({}, contacts());
    const entries: HistoryEntry[] = [{ kind: "transfer", transfer: { record: record(), saved_present: null } }];
    device.setHistory(entries);
    const { user } = await start(device);
    await tabTo(user, button("History"));
    await user.keyboard("{Enter}");

    const search = await screen.findByLabelText("Search by item name");
    await tabTo(user, search);
    await user.keyboard("photo");
    await waitFor(() => expect(device.historyReads.at(-1)).toEqual([null, null, "photo"]));

    const retry = await screen.findByRole("button", { name: /^Delete .* from History$/ });
    await tabTo(user, retry);
    await user.keyboard("{Enter}");
    expect(device.api.deleteHistoryTransfer).toHaveBeenCalledWith(TRANSFER);
  });

  it("reaches the filters' selects", async () => {
    const { user } = await start();
    await user.click(button("History"));
    await tabTo(user, await screen.findByLabelText("Device"));
    await tabTo(user, screen.getByLabelText("Direction"));
  });
});

describe("keyboard: Contacts", () => {
  it("saves a Nickname with Enter, flips Auto-accept with Space and removes with a dialog", async () => {
    const { user, device } = await start();
    await tabTo(user, button("Contacts"));
    await user.keyboard("{Enter}");

    const [nickname] = await screen.findAllByLabelText("Nickname");
    await tabTo(user, nickname);
    await user.clear(nickname);
    await user.type(nickname, "Mummy{Enter}");
    await waitFor(() => expect(device.api.setNickname).toHaveBeenCalled());

    const [auto] = await screen.findAllByRole("checkbox", { name: "Auto-accept" });
    await tabTo(user, auto);
    await user.keyboard(" ");
    await waitFor(() => expect(device.api.setAutoAccept).toHaveBeenCalled());
  });
});

describe("keyboard: Settings", () => {
  it("changes Visibility with the arrow keys and the public DHT with Space", async () => {
    const { user, device } = await start();
    await tabTo(user, button("Settings"));
    await user.keyboard("{Enter}");
    const holders = await screen.findByLabelText("People who have my ID");
    await waitFor(() => expect((holders as HTMLInputElement).disabled).toBe(false));
    await tabTo(user, holders);
    await user.keyboard("{ArrowDown}");
    await waitFor(() => expect(device.api.setVisibility).toHaveBeenCalledWith("hidden"));

    const dht = screen.getByLabelText("Public DHT");
    await waitFor(() => expect((dht as HTMLInputElement).disabled).toBe(false));
    await tabTo(user, dht);
    await user.keyboard(" ");
    await waitFor(() => expect(device.api.setPublicDht).toHaveBeenCalledWith(false));
  });

  it("checks for updates with Enter", async () => {
    const { user, device } = await start();
    await user.click(button("Settings"));
    await tabTo(user, await screen.findByRole("button", { name: "Check for updates" }));
    await user.keyboard("{Enter}");
    await waitFor(() => expect(device.api.checkForUpdate).toHaveBeenCalled());
  });
});
