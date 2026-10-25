import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import axe from "axe-core";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import "./heavyTests";
import type { Contact } from "./api";
import type { HistoryEntry, TransferRecord, TransferState } from "./bindings";
import { BATCH, EXPIRES_AT, PEER_ID, TRANSFER, contact, fakeApi } from "./testApi";

// The camera cannot be tested here: this stand-in stays a button, as in App.test.tsx.
vi.mock("./QrScanner", () => ({
  QrScanner: () => <p role="status">Hold the other Device's QR code in front of the camera.</p>,
}));

afterEach(cleanup);

// axe tries a canvas for some checks; jsdom has none and prints a complaint for every try.
beforeAll(() => {
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
});

const DAD = "Q2WERTYU" + "C".repeat(44);
const STRANGER = "ZZZZ7777" + "D".repeat(44);
const SAVED = "/home/me/Downloads/BhayanakShare/photo.jpg";

/**
 * Fails, listing every rule broken and where, if axe finds a violation on the page as it is now.
 * Colour contrast is switched off: jsdom does no layout or painting, so axe cannot work out the
 * colours on screen and would either skip the check or report nonsense. Contrast is on the
 * manual checklist in docs/accessibility.md instead.
 */
async function expectNoViolations() {
  const { violations } = await axe.run(document.body, { rules: { "color-contrast": { enabled: false } } });
  const report = violations
    .map(
      (v) =>
        `${v.id} (${v.impact}): ${v.help}\n` +
        v.nodes.map((n) => `    ${n.target.join(" ")}\n    ${n.html}`).join("\n"),
    )
    .join("\n");
  // Thrown rather than compared, so that the whole report is the message.
  if (report !== "") throw new Error(report);
  expectNamesHoldVisibleText();
}

/**
 * A button or link given an `aria-label` must still be named by the words it shows (WCAG 2.5.3,
 * for people who speak "click Show in folder"). axe has a rule for this, but it only marks the
 * labels it cannot judge as "needs review", so it is checked here. A tile is the one exception:
 * it shows its name with words about its state, which it carries as its description instead.
 */
function expectNamesHoldVisibleText() {
  const odd = [...document.querySelectorAll<HTMLElement>("button[aria-label], a[aria-label]")]
    .filter((el) => !el.classList.contains("tile"))
    .filter((el) => {
      const shown = (el.textContent ?? "").replace(/…/g, "").trim().toLowerCase();
      return shown !== "" && !el.getAttribute("aria-label")!.toLowerCase().includes(shown);
    })
    .map((el) => `"${el.getAttribute("aria-label")}" does not hold "${el.textContent}"`);
  if (odd.length > 0) throw new Error(odd.join("\n"));
}

/** Renders the app and waits until it is listening for events (and My ID, on Home, is shown). */
async function start(device = fakeApi()) {
  render(<App api={device.api} />);
  await screen.findByRole("heading", { name: /^(My ID|Welcome to BhayanakShare)$/ });
  return device;
}

const contacts = (): Contact[] => [
  contact({ id: PEER_ID, nickname: "Mum", added_at: 1 }),
  contact({ id: DAD, device_name: "Dad's PC", added_at: 2 }),
];

const open = (tab: string) => fireEvent.click(screen.getByRole("button", { name: tab }));

/** A Transfer of the sender's or receiver's, with its own ID. */
function row(
  device: ReturnType<typeof fakeApi>,
  n: number,
  role: "sender" | "receiver",
  state: TransferState,
  over: Record<string, unknown> = {},
) {
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
    state: { kind: "completed", saved_to: SAVED },
    created_at: 1_700_000_000_000,
    accepted_at: 1_700_000_005_000,
    updated_at: 1_700_000_060_000,
    ...over,
  };
}

describe("accessibility: the check itself", () => {
  it("finds a control with no name, so a clean result means something", async () => {
    render(<button type="button" />);
    await expect(expectNoViolations()).rejects.toThrow(/button-name/);
  });

  it("finds a label that does not hold the words the control shows", async () => {
    render(
      <main>
        <h1>Page</h1>
        <button type="button" aria-label="Show photo.jpg in folder">
          Show in folder
        </button>
      </main>,
    );
    await expect(expectNoViolations()).rejects.toThrow(/does not hold/);
  });

  it("finds content outside every landmark", async () => {
    render(<p>Lost</p>);
    await expect(expectNoViolations()).rejects.toThrow(/region/);
  });
});

describe("accessibility: first run", () => {
  const firstRun = async (overrides = {}) => {
    await start(fakeApi({ needsFirstRun: vi.fn(() => Promise.resolve(true)), ...overrides }));
    await screen.findByLabelText("Device Name");
  };

  it("has no violations on the first-run screen", async () => {
    await firstRun();
    await expectNoViolations();
  });

  it("has none while it says a setting could not be read, or the name is missing", async () => {
    await firstRun({ visibility: () => Promise.reject(new Error("no")) });
    fireEvent.change(screen.getByLabelText("Device Name"), { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: "Get started" }));
    await screen.findByText("Enter a name for this Device.");
    await expectNoViolations();
  });

  it("has none while it says start at login could not be switched on", async () => {
    await firstRun({ setAutostart: () => Promise.reject(new Error("no")) });
    fireEvent.click(screen.getByRole("button", { name: "Get started" }));
    await screen.findByRole("button", { name: "Continue" });
    await expectNoViolations();
  });
});

describe("accessibility: Home", () => {
  it("has no violations with Contacts, Nearby Devices and a selection", async () => {
    const device = await start(fakeApi({}, contacts()));
    await screen.findByRole("button", { name: "Send to Mum" });
    await device.nearby({ id: PEER_ID, name: "Mum's phone" }, { id: STRANGER, name: "Sam's laptop" });
    fireEvent.click(screen.getByRole("checkbox", { name: /Select Mum/ }));
    await expectNoViolations();
  });

  it("has none with files waiting for a Device to be chosen", async () => {
    const device = await start(fakeApi({}, contacts()));
    await device.shell({ type: "send_files", paths: ["/tmp/a.txt", "/tmp/b.txt"] });
    fireEvent.click(await screen.findByRole("checkbox", { name: /Select Dad/ }));
    await expectNoViolations();
  });

  it("has none with every kind of Transfer row", async () => {
    const device = await start(fakeApi({}, contacts()));
    await row(device, 1, "sender", { kind: "offered" });
    await row(device, 2, "receiver", { kind: "transferring" });
    await device.push({ type: "progress", transfer_id: "2".repeat(32), bytes: 1024, total: 2048 });
    await row(device, 3, "receiver", { kind: "completed", saved_to: SAVED });
    await row(device, 4, "sender", { kind: "failed", reason: "The connection dropped." });
    await row(device, 5, "sender", { kind: "expired" });
    await row(device, 6, "receiver", { kind: "completed", saved_to: null }, { kind: "text", text: "hello", name: "", items: [] });
    await expectNoViolations();
  });

  it("has none with the Hidden hint, an update and a version notice", async () => {
    const device = await start(fakeApi({ visibility: () => Promise.resolve("hidden") }, contacts()));
    await device.shell({ type: "update_available", action: { type: "install", version: "0.3.0" } });
    await device.push({
      type: "version_mismatch",
      peer: PEER_ID,
      peer_name: "Mum's phone",
      peer_app_version: "0.0.1",
      outdated: "this_device",
    });
    await screen.findByText(/You're Hidden/);
    await expectNoViolations();
  });
});

describe("accessibility: Home, redesigned", () => {
  it("has none with no Devices and no Transfers: both empty boxes, and My ID before Transfers", async () => {
    await start();
    await screen.findByText("Your Contacts and Nearby Devices will appear here.");
    await screen.findByText("No Transfers yet.");
    await expectNoViolations();
  });

  it("has none in the selection bar while a text is written to several Devices", async () => {
    const device = await start(fakeApi({}, contacts()));
    await device.nearby({ id: STRANGER, name: "Sam's laptop" });
    fireEvent.click(await screen.findByRole("checkbox", { name: /Select Mum/ }));
    fireEvent.click(screen.getByRole("checkbox", { name: /Select Sam/ }));
    fireEvent.click(screen.getByRole("button", { name: "Write text…" }));
    await expectNoViolations();
  });

  it("has none with the firewall hint, and with files waiting", async () => {
    const real = globalThis.setTimeout;
    vi.spyOn(globalThis, "setTimeout").mockImplementation(((fn: () => void, ms?: number, ...args: unknown[]) =>
      real(fn, ms === 30_000 ? 0 : ms, ...args)) as unknown as typeof setTimeout);
    const device = await start();
    await device.shell({ type: "send_files", paths: ["/tmp/a.txt"] });
    await screen.findByText(/No Devices found on this network yet/);
    await screen.findByText(/Choose a Device to send a\.txt to/);
    await expectNoViolations();
    vi.restoreAllMocks();
  });

  it("has none with a package's update banner, which links to the release page", async () => {
    const device = await start();
    await device.shell({ type: "update_available", action: { type: "open_page", version: "0.3.0" } });
    await screen.findByRole("link", { name: "Open the release page" });
    await expectNoViolations();
  });

  it("has none with a Transfer reconnecting, declined and cancelled, and a text copied", async () => {
    const device = await start(fakeApi({}, contacts()));
    await row(device, 1, "receiver", { kind: "reconnecting" });
    await device.push({ type: "progress", transfer_id: "1".repeat(32), bytes: 1024, total: 2048 });
    await row(device, 2, "sender", { kind: "declined" });
    await row(device, 3, "receiver", { kind: "cancelled", by: "sender" });
    await row(device, 4, "receiver", { kind: "completed", saved_to: null }, { kind: "text", text: "hello", name: "", items: [] });
    fireEvent.click(await screen.findByRole("button", { name: /^Copy the text/ }));
    await expectNoViolations();
  });
});

describe("accessibility: the hint that local discovery could not start", () => {
  it("has no violations on Home, for a Device that looks for others and for a Hidden one", async () => {
    const device = await start(fakeApi({}, contacts()));
    await device.discoveryChanges({ state: "unavailable", reason: "port_in_use" });
    await screen.findByText(/Local discovery could not start, so Nearby Devices/);
    await expectNoViolations();

    cleanup();
    const hidden = await start(fakeApi({ visibility: () => Promise.resolve("hidden") }, contacts()));
    await hidden.discoveryChanges({ state: "unavailable", reason: "no_interface" });
    await screen.findByText(/people who have your ID can't find this Device/);
    await expectNoViolations();
  });
});

describe("accessibility: the Offer sheet", () => {
  it("has no violations for files, with room to save them", async () => {
    const device = await start(fakeApi({}, contacts()));
    await row(device, 1, "receiver", { kind: "offered" }, { items: ["a.txt", "b.txt"], name: "a.txt", file_count: 2 });
    await screen.findByRole("dialog", { name: "Incoming files" });
    await screen.findByText("/home/me/Downloads/BhayanakShare");
    await expectNoViolations();
  });

  it("has none for a text", async () => {
    const device = await start(fakeApi());
    await row(device, 1, "receiver", { kind: "offered" }, { kind: "text", text: "Meet at 5", name: "", items: [] });
    await screen.findByRole("dialog", { name: "Incoming text" });
    await expectNoViolations();
  });

  it("has none while it says there is no room", async () => {
    const checkOffer = vi.fn(() => Promise.resolve({ folder: "/f", needed: 2048, free: 10, paths_too_long: true }));
    const device = await start(fakeApi({ checkOffer }));
    await row(device, 1, "receiver", { kind: "offered" });
    await screen.findByText(/only .* free/);
    await expectNoViolations();
  });
});

describe("accessibility: the Offer sheet, with problems", () => {
  it("has none with adjusted names and many items, and Accept still available", async () => {
    const device = await start(fakeApi({}, contacts()));
    const items = Array.from({ length: 12 }, (_, i) => `file-${i}.txt`);
    await row(device, 1, "receiver", { kind: "offered" }, { items, name: "file-0.txt", file_count: 12, adjusted_names: 3 });
    await screen.findByRole("dialog", { name: "Incoming files" });
    await screen.findAllByText("3 names adjusted");
    await screen.findByText("and 2 more");
    expect((screen.getByRole("button", { name: "Accept" }) as HTMLButtonElement).disabled).toBe(false);
    await expectNoViolations();
  });

  it("has none while the folder cannot be used: Accept is off and says why", async () => {
    const checkOffer = vi.fn(() => Promise.reject(new Error("drive not connected")));
    const device = await start(fakeApi({ checkOffer }));
    await row(device, 1, "receiver", { kind: "offered" });
    await screen.findByText(/Could not use that folder/);
    const accept = screen.getByRole("button", { name: "Accept" }) as HTMLButtonElement;
    expect(accept.disabled).toBe(true);
    expect(document.getElementById(accept.getAttribute("aria-describedby")!)?.textContent).toMatch(
      /Accept is not available until the problems above are solved/,
    );
    await expectNoViolations();
  });
});

describe("accessibility: Batch rows", () => {
  const many = async (device: ReturnType<typeof fakeApi>) => {
    await row(device, 1, "sender", { kind: "completed", saved_to: null }, { batch_id: BATCH });
    await row(device, 2, "sender", { kind: "transferring" }, { batch_id: BATCH, peer: DAD });
    await row(device, 3, "sender", { kind: "failed", reason: "Gone." }, { batch_id: BATCH, peer: STRANGER });
  };

  it("has no violations while closed", async () => {
    const device = await start(fakeApi({}, contacts()));
    await many(device);
    await screen.findByText(/to 3 Devices/);
    await expectNoViolations();
  });

  it("has none once opened to show each Device", async () => {
    const device = await start(fakeApi({}, contacts()));
    await many(device);
    fireEvent.click(await screen.findByRole("button", { name: /Show each Device/ }));
    await expectNoViolations();
  });
});

describe("accessibility: History", () => {
  const entries = (): HistoryEntry[] => [
    { kind: "transfer", transfer: { record: record(), saved_present: true } },
    { kind: "transfer", transfer: { record: record({ id: "cd".repeat(16), state: { kind: "failed", reason: "Gone." } }), saved_present: null } },
    { kind: "transfer", transfer: { record: record({ id: "ee".repeat(16) }), saved_present: false } },
    {
      kind: "batch",
      batch_id: BATCH,
      transfers: [1, 2].map((i) => ({
        saved_present: null,
        record: record({
          id: String(i).repeat(32),
          role: "sender",
          peer: i === 1 ? PEER_ID : DAD,
          peer_name: `Device ${i}`,
          batch_id: BATCH,
          state: i === 1 ? { kind: "completed", saved_to: null } : { kind: "declined" },
        }),
      })),
    },
    { kind: "transfer", transfer: { record: record({ id: "ff".repeat(16), kind: "text", text: "hi", items: [], name: "" }), saved_present: null } },
  ];

  it("has no violations with entries, a Batch and the filters", async () => {
    const device = fakeApi({}, contacts());
    device.setHistory(entries());
    await start(device);
    open("History");
    await screen.findAllByText(/Offered/);
    await expectNoViolations();
  });

  it("has none with a Batch opened", async () => {
    const device = fakeApi({}, contacts());
    device.setHistory(entries());
    await start(device);
    open("History");
    fireEvent.click(await screen.findByRole("button", { name: /Show each Device/ }));
    await expectNoViolations();
  });

  it("has none when it is empty or cannot be read", async () => {
    await start(fakeApi({ history: () => Promise.reject(new Error("db")) }));
    open("History");
    await screen.findByText("Could not load your History.");
    await expectNoViolations();
  });

  it("has none when a text was copied, could not be copied, or a row could not be deleted", async () => {
    const copyText = vi.fn().mockResolvedValueOnce(undefined).mockRejectedValue(new Error("no"));
    const device = fakeApi({ copyText, deleteHistoryTransfer: () => Promise.reject(new Error("db")) }, contacts());
    device.setHistory(entries());
    await start(device);
    open("History");
    const copy = await screen.findByRole("button", { name: /^Copy the text from / });
    fireEvent.click(copy);
    await screen.findByText("Copied");
    await expectNoViolations();
    fireEvent.click(copy);
    await screen.findByText("Could not copy. Select the text and copy it by hand.");
    fireEvent.click(screen.getAllByRole("button", { name: /^Delete .* from History$/ })[0]);
    await screen.findByText(/^Could not delete it\./);
    await expectNoViolations();
  });

  it("has none with the filters narrowed to nothing", async () => {
    const device = fakeApi({}, contacts());
    device.setHistory([]);
    await start(device);
    open("History");
    fireEvent.change(screen.getByLabelText("Search by item name"), { target: { value: "zzz" } });
    await screen.findByText("No Transfers match.");
    await expectNoViolations();
  });
});

describe("accessibility: Contacts", () => {
  it("has no violations with Contacts", async () => {
    await start(fakeApi({}, contacts()));
    open("Contacts");
    await screen.findByRole("heading", { name: /Mum/ });
    await expectNoViolations();
  });

  it("has none with no Contacts", async () => {
    await start();
    open("Contacts");
    await screen.findByText(/No Contacts yet/);
    await expectNoViolations();
  });

  it("has none while a change failed", async () => {
    const setAutoAccept = vi.fn(() => Promise.reject(new Error("db")));
    await start(fakeApi({ setAutoAccept }, contacts()));
    open("Contacts");
    fireEvent.click((await screen.findAllByRole("checkbox", { name: "Auto-accept" }))[0]);
    await screen.findAllByRole("alert");
    await expectNoViolations();
  });
});

describe("accessibility: Settings", () => {
  it("has no violations on the whole screen", async () => {
    await start(fakeApi());
    open("Settings");
    await screen.findByRole("heading", { name: "Diagnostics" });
    await screen.findByText("Version 0.1.0");
    await waitFor(() => expect((screen.getByLabelText("Everyone") as HTMLInputElement).disabled).toBe(false));
    await expectNoViolations();
  });

  it("has none with a newer version found and a save folder refused", async () => {
    const device = await start(
      fakeApi({ pendingUpdate: () => Promise.resolve({ type: "open_page", version: "0.4.0" }) }),
    );
    open("Settings");
    await screen.findByText(/Update available/);
    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    await act(async () => {});
    expect(device.api.checkForUpdate).toHaveBeenCalled();
    await expectNoViolations();
  });

  it("has none while a switch, the Visibility, the save folder and the export each say they failed", async () => {
    await start(
      fakeApi({
        setPublicDht: () => Promise.reject(new Error("no")),
        setVisibility: () => Promise.reject(new Error("no")),
        setSaveFolder: () => Promise.reject(new Error("no")),
        pickDiagnosticsSavePath: () => Promise.resolve("/tmp/d.zip"),
        exportDiagnostics: () => Promise.reject(new Error("no")),
      }),
    );
    open("Settings");
    await waitFor(() => expect((screen.getByLabelText("Everyone") as HTMLInputElement).disabled).toBe(false));
    await waitFor(() =>
      expect((screen.getByRole("checkbox", { name: "Public DHT" }) as HTMLInputElement).disabled).toBe(false),
    );
    fireEvent.click(screen.getByRole("checkbox", { name: "Public DHT" }));
    fireEvent.click(screen.getByLabelText("Hidden"));
    fireEvent.click(screen.getByRole("button", { name: "Change the folder received files are saved to" }));
    fireEvent.click(screen.getByRole("button", { name: "Export diagnostics…" }));
    await screen.findByText(/Could not change the public DHT/);
    await screen.findByText(/Could not use that folder/);
    await screen.findByText(/Could not export diagnostics/);
    await expectNoViolations();
  });

  it("has none with a new Device Name saved and the save folder changed", async () => {
    await start();
    open("Settings");
    const name = await screen.findByLabelText("Device Name");
    await waitFor(() => expect((name as HTMLInputElement).disabled).toBe(false));
    fireEvent.change(name, { target: { value: "Kitchen PC" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText(/Kitchen PC/, { selector: "[role=status] span" });
    fireEvent.click(screen.getByRole("button", { name: "Change the folder received files are saved to" }));
    await screen.findByText(/Saved\. Files you receive/);
    await expectNoViolations();
  });
});

describe("accessibility: dialogs", () => {
  it("has no violations in Send to ID, and in its text step", async () => {
    await start();
    fireEvent.click(screen.getByRole("button", { name: "Send to ID…" }));
    await expectNoViolations();
    fireEvent.change(screen.getByLabelText("Device ID"), { target: { value: PEER_ID } });
    fireEvent.click(screen.getByRole("button", { name: "Write text…" }));
    await screen.findByLabelText("Text");
    await expectNoViolations();
  });

  it("has none in Send with files already chosen", async () => {
    const device = await start(fakeApi({}, contacts()));
    await device.shell({ type: "send_files", paths: ["/tmp/a.txt"] });
    fireEvent.click(await screen.findByRole("button", { name: "Send to Mum" }));
    await screen.findByRole("button", { name: "Send" });
    await expectNoViolations();
  });

  it("has none in Add Contact: the form, a refused ID, scanning, and the Fingerprint check", async () => {
    await start();
    open("Contacts");
    fireEvent.click(screen.getByRole("button", { name: "Add Contact…" }));
    await expectNoViolations();
    fireEvent.change(screen.getByLabelText("Device ID"), { target: { value: "nonsense" } });
    await screen.findByText(/not a Device ID/);
    await expectNoViolations();
    fireEvent.click(screen.getByRole("button", { name: "Scan QR code…" }));
    await expectNoViolations();
    fireEvent.change(screen.getByLabelText("Device ID"), { target: { value: DAD } });
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    await screen.findByRole("dialog", { name: "Check the Fingerprint" });
    await expectNoViolations();
  });

  it("has none in Add Contact opened by a link that is not a share link", async () => {
    const device = await start();
    await device.openLink("https://example.com/");
    await screen.findByText(/not a BhayanakShare share link/);
    await expectNoViolations();
  });

  it("has none in Remove Contact and Clear History", async () => {
    const device = fakeApi({}, contacts());
    device.setHistory([{ kind: "transfer", transfer: { record: record(), saved_present: null } }]);
    await start(device);
    open("Contacts");
    fireEvent.click(await screen.findByRole("button", { name: "Remove Mum from Contacts" }));
    await screen.findByRole("alertdialog", { name: "Remove Mum?" });
    await expectNoViolations();
    fireEvent.click(screen.getByRole("button", { name: "Keep" }));
    open("History");
    fireEvent.click(await screen.findByRole("button", { name: "Clear History…" }));
    await screen.findByRole("alertdialog", { name: "Clear History?" });
    await expectNoViolations();
  });

  it("has none in Quit, asking and saving", async () => {
    const device = await start();
    await device.shell({ type: "confirm_quit", active: 2 });
    await screen.findByRole("dialog", { name: "Quit BhayanakShare?" });
    await expectNoViolations();
    await device.shell({ type: "quitting" });
    await screen.findByRole("dialog", { name: "Saving progress…" });
    await expectNoViolations();
  });

  it("has none in Export identity and Import identity, in each step", async () => {
    await start();
    open("Settings");
    await waitFor(() =>
      expect((screen.getByRole("button", { name: "Export identity…" }) as HTMLButtonElement).disabled).toBe(false),
    );
    fireEvent.click(screen.getByRole("button", { name: "Export identity…" }));
    await screen.findByRole("dialog", { name: "Export identity" });
    await expectNoViolations();
    fireEvent.click(screen.getByRole("button", { name: "Save as…" }));
    expect((await screen.findByRole("alert")).textContent).toMatch(/at least/i);
    await expectNoViolations();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));

    fireEvent.click(screen.getByRole("button", { name: "Import identity…" }));
    await screen.findByLabelText("Password of this file");
    await expectNoViolations();
    fireEvent.change(screen.getByLabelText("Password of this file"), { target: { value: "correct horse" } });
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    await screen.findByRole("alertdialog", { name: "Replace this Device's identity?" });
    await expectNoViolations();
  });

  it("has none in My ID inside Settings, once copied and when it could not be copied", async () => {
    const copyText = vi.fn().mockResolvedValueOnce(undefined).mockRejectedValue(new Error("no"));
    await start(fakeApi({ copyText }));
    open("Settings");
    await screen.findByRole("img", { name: "QR code of the share link" });
    fireEvent.click(screen.getByRole("button", { name: "Copy ID" }));
    await screen.findByText("Copied");
    await expectNoViolations();
    fireEvent.click(screen.getByRole("button", { name: "Copy link" }));
    await screen.findByText("Could not copy. Select the text and copy it by hand.");
    await expectNoViolations();
  });

  it("has none in Export identity: a mismatch under the field, a save that failed, and once saved", async () => {
    const exportIdentity = vi.fn().mockRejectedValueOnce(new Error("disk full")).mockResolvedValue(null);
    await start(fakeApi({ exportIdentity }));
    open("Settings");
    await waitFor(() =>
      expect((screen.getByRole("button", { name: "Export identity…" }) as HTMLButtonElement).disabled).toBe(false),
    );
    fireEvent.click(screen.getByRole("button", { name: "Export identity…" }));
    await screen.findByRole("dialog", { name: "Export identity" });
    fireEvent.change(screen.getByLabelText("Password"), { target: { value: "long enough" } });
    fireEvent.change(screen.getByLabelText("Type the password again"), { target: { value: "different" } });
    fireEvent.click(screen.getByRole("button", { name: "Save as…" }));
    await screen.findByRole("alert");
    await expectNoViolations();
    fireEvent.change(screen.getByLabelText("Type the password again"), { target: { value: "long enough" } });
    fireEvent.click(screen.getByRole("button", { name: "Save as…" }));
    await screen.findByText(/disk full/);
    await expectNoViolations();
    fireEvent.click(screen.getByRole("button", { name: "Save as…" }));
    await screen.findByText(/Saved to/);
    await expectNoViolations();
  });

  it("has none in Import identity: a wrong password, a file with this Device's own identity, and the warnings", async () => {
    const checkIdentityImport = vi
      .fn()
      .mockRejectedValueOnce({ kind: "wrong_password", message: "" })
      .mockResolvedValueOnce({ id: "A".repeat(52), fingerprint: "AAAA-AAAA" })
      .mockResolvedValue({ id: "B".repeat(52), fingerprint: "BBBB-BBBB" });
    const transfersInProgress = vi.fn(() => Promise.resolve(2));
    await start(fakeApi({ checkIdentityImport, transfersInProgress }));
    open("Settings");
    fireEvent.click(await screen.findByRole("button", { name: "Import identity…" }));
    const password = await screen.findByLabelText("Password of this file");
    fireEvent.change(password, { target: { value: "wrong" } });
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    await screen.findByRole("alert");
    expect(password.getAttribute("aria-invalid")).toBe("true");
    await expectNoViolations();
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    await screen.findByText(/Nothing needs to change/);
    await expectNoViolations();
    fireEvent.click(screen.getByRole("button", { name: "Close" }));

    fireEvent.click(screen.getByRole("button", { name: "Import identity…" }));
    fireEvent.change(await screen.findByLabelText("Password of this file"), { target: { value: "right" } });
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    await screen.findByRole("alertdialog", { name: "Replace this Device's identity?" });
    await screen.findByText(/2 Transfers are in progress/);
    await expectNoViolations();
  });

  it("has none in the camera's own refusal", async () => {
    const { QrScanner } = await vi.importActual<typeof import("./QrScanner")>("./QrScanner");
    vi.stubGlobal("navigator", { ...navigator, mediaDevices: undefined });
    try {
      render(
        <main>
          <QrScanner onDecoded={() => {}} />
        </main>,
      );
      await screen.findByRole("alert");
      await expectNoViolations();
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("has none in My ID's QR code", async () => {
    await start();
    expect(within(document.body).getByRole("img", { name: "QR code of the share link" })).toBeTruthy();
    await expectNoViolations();
  });
});

describe("accessibility: live regions", () => {
  /** Live regions that already hold words when they are first on the page: some screen readers read them out. */
  const talking = () => screen.queryAllByRole("status").filter((el) => el.textContent !== "");

  it("does not make a standing hint a live region: Hidden is a state, reached by changing tab", async () => {
    await start(fakeApi({ visibility: () => Promise.resolve("hidden") }));
    const hint = await screen.findByText(/You're Hidden/);
    expect(hint.closest("[role=status]")).toBeNull();
    expect(talking()).toHaveLength(0);
  });

  it("does not read out a History of rows whose files are gone", async () => {
    const device = fakeApi({}, contacts());
    device.setHistory(
      [1, 2, 3].map((i) => ({
        kind: "transfer" as const,
        transfer: { record: record({ id: String(i).repeat(32) }), saved_present: false },
      })),
    );
    await start(device);
    open("History");
    expect(await screen.findAllByText("File no longer at saved location")).toHaveLength(3);
    expect(talking()).toHaveLength(0);
  });

  it("keeps the words of a hint that appears because something happened: an update", async () => {
    const device = await start();
    await device.shell({ type: "update_available", action: { type: "open_page", version: "0.3.0" } });
    expect((await screen.findByText(/Update available/)).closest("[role=status]")).not.toBeNull();
  });
});
