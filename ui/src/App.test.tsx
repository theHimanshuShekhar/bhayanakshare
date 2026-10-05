import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import type { Api, Contact, DeviceEvent, ShellEvent, Visibility } from "./api";
import type { NearbyDevice, TransferState } from "./bindings";
import { NEARBY_WAIT_MS } from "./nearby";

afterEach(cleanup);

const MY_ID = "A".repeat(52);
const PEER_ID = "K3QF7XNA" + "B".repeat(44);
const TRANSFER = "ab".repeat(16);
const BATCH = "ba".repeat(16);
/** When the Offers in these tests lapse: 10 minutes after the stand-in's clock reads 0. */
const EXPIRES_AT = 600_000;

/** A Device event without the stream position and time the stand-in fills in. */
type Unstamped = DeviceEvent extends infer E
  ? E extends unknown
    ? Omit<E, "seq" | "at"> & { at?: number }
    : never
  : never;

function contact(over: Partial<Contact> = {}): Contact {
  return {
    id: PEER_ID,
    nickname: null,
    device_name: null,
    auto_accept: false,
    last_known_address: { relay_url: null, direct: [] },
    added_at: 1,
    ...over,
  };
}

/** A stand-in for the Rust shell: records commands, and lets a test push Device events. */
function fakeApi(overrides: Partial<Api> = {}, initialContacts: Contact[] = []) {
  let handler: (event: DeviceEvent) => void = () => {};
  let shellHandler: (event: ShellEvent) => void = () => {};
  let autostart = true;
  let seq = 0;
  let contacts = initialContacts;
  let visibility: Visibility = "id_holders";
  const change =(id: string, over: (c: Contact) => Partial<Contact>) => {
    const changed = contacts.map((c) => (c.id === id ? { ...c, ...over(c) } : c));
    contacts = changed;
    return Promise.resolve(changed.find((c) => c.id === id)!);
  };
  const api = {
    myId: () => Promise.resolve({ id: MY_ID, fingerprint: "AAAA-AAAA" }),
    saveFolder: () => Promise.resolve("/home/me/Downloads/BhayanakShare"),
    sendFiles: vi.fn((_to: string, _paths: string[]) => Promise.resolve(TRANSFER)),
    sendBatch: vi.fn((_to: string[], _paths: string[]) => Promise.resolve(BATCH)),
    sendText: vi.fn((_to: string, _text: string) => Promise.resolve(TRANSFER)),
    sendTextBatch: vi.fn((_to: string[], _text: string) => Promise.resolve(BATCH)),
    cancelBatch: vi.fn((_id: string) => Promise.resolve(null)),
    retryTransfer: vi.fn((_id: string) => Promise.resolve("ef".repeat(16))),
    checkOffer: vi.fn((_id: string, _folder: string | null) =>
      Promise.resolve({ needed: 2048, free: 1_000_000, paths_too_long: false }),
    ),
    acceptOffer: vi.fn((_id: string, _folder: string | null) => Promise.resolve(null)),
    declineOffer: vi.fn(() => Promise.resolve(null)),
    cancelTransfer: vi.fn(() => Promise.resolve(null)),
    resendTransfer: vi.fn(() => Promise.resolve("cd".repeat(16))),
    visibility: vi.fn(() => Promise.resolve(visibility)),
    setVisibility: vi.fn((v: Visibility) => {
      visibility = v;
      return Promise.resolve(null);
    }),
    autostartEnabled: vi.fn(() => Promise.resolve(autostart)),
    setAutostart: vi.fn((on: boolean) => {
      autostart = on;
      return Promise.resolve(null);
    }),
    quitApp: vi.fn(() => Promise.resolve(null)),
    contacts: vi.fn(() => Promise.resolve(contacts)),
    addContact: vi.fn((id: string, deviceName: string | null) => {
      const added = contact({ id, device_name: deviceName, added_at: contacts.length + 1 });
      contacts = [...contacts, added];
      return Promise.resolve(added);
    }),
    setNickname: vi.fn((id: string, nickname: string | null) =>
      change(id, () => ({ nickname: nickname?.trim() ? nickname.trim() : null })),
    ),
    setAutoAccept: vi.fn((id: string, on: boolean) => change(id, () => ({ auto_accept: on }))),
    removeContact: vi.fn((id: string) => {
      contacts = contacts.filter((c) => c.id !== id);
      return Promise.resolve(null);
    }),
    pickFiles: vi.fn(() => Promise.resolve<string[] | null>(["/tmp/photo.jpg"])),
    pickFolder: vi.fn(() => Promise.resolve<string | null>("/mnt/big")),
    showInFolder: vi.fn(() => Promise.resolve()),
    openUrl: vi.fn((_url: string) => Promise.resolve()),
    copyText: vi.fn(() => Promise.resolve()),
    onDeviceEvent: (h: (event: DeviceEvent) => void) => {
      handler = h;
      return Promise.resolve(() => {});
    },
    onShellEvent: (h: (event: ShellEvent) => void) => {
      shellHandler = h;
      return Promise.resolve(() => {});
    },
    ...overrides,
  } satisfies Api;
  /** The shell says something: a second launch, a clicked notification, Quit. */
  const shell = (event: ShellEvent) => act(() => shellHandler(event));
  const push = (event: Unstamped) =>
    act(() => handler({ seq: seq++, at: 1_000 * seq, ...event } as DeviceEvent));
  /** What an Offer holds when it is not just `photo.jpg`: a folder, several files, links skipped,
   * names adjusted. */
  type Contents = Partial<{
    kind: "files" | "text";
    text: string | null;
    name: string;
    items: string[];
    file_count: number;
    skipped_links: number;
    adjusted_names: number;
  }>;
  const transfer = (
    role: "sender" | "receiver",
    state: TransferState,
    peerName: string | null = null,
    contents: Contents = {},
  ) =>
    push({
      type: "transfer",
      transfer_id: TRANSFER,
      role,
      peer: PEER_ID,
      peer_name: peerName,
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
      ...contents,
    });
  /** The Device reports the Nearby Devices as they are now. */
  const nearby = (...devices: NearbyDevice[]) => push({ type: "nearby", devices });
  return { api, push, transfer, nearby, shell };
}

/** Renders the app and waits until it is listening for events. */
async function start(device = fakeApi()) {
  render(<App api={device.api} />);
  await screen.findByText("AAAA-AAAA");
  return device;
}

async function sendPhoto(device: ReturnType<typeof fakeApi>) {
  fireEvent.click(screen.getByRole("button", { name: "Send to ID…" }));
  fireEvent.change(screen.getByLabelText("Device ID"), { target: { value: ` ${PEER_ID} ` } });
  fireEvent.click(screen.getByRole("button", { name: "Choose files…" }));
  await waitFor(() => expect(device.api.sendFiles).toHaveBeenCalled());
}

describe("My ID", () => {
  it("shows this Device's ID and Fingerprint on Home", async () => {
    await start();
    expect(screen.getByText("AAAA-AAAA")).toBeTruthy();
    expect(screen.getByText(MY_ID)).toBeTruthy();
  });

  it("copies the Device ID and says so", async () => {
    const device = await start();
    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    expect(await screen.findByText("Copied")).toBeTruthy();
    expect(device.api.copyText).toHaveBeenCalledWith(MY_ID);
  });

  it("says when the ID could not be copied", async () => {
    await start(fakeApi({ copyText: () => Promise.reject(new Error("no clipboard")) }));
    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    expect((await screen.findByText(/Could not copy/)).textContent).toContain("by hand");
  });

  it("reports a Device that failed to start", async () => {
    render(<App api={fakeApi({ myId: () => Promise.reject(new Error("no device")) }).api} />);
    expect((await screen.findByRole("alert")).textContent).toContain("could not start");
  });
});

describe("navigation", () => {
  it("switches between the four tabs and marks the current one", async () => {
    await start();
    const nav = screen.getByRole("navigation", { name: "Main" });
    expect(nav.querySelectorAll("button")).toHaveLength(4);

    fireEvent.click(screen.getByRole("button", { name: "History" }));
    expect(screen.getByRole("button", { name: "History" }).getAttribute("aria-current")).toBe("page");
    expect(screen.getByRole("button", { name: "Home" }).getAttribute("aria-current")).toBeNull();
    expect(screen.getByText("Your Transfer History will appear here.")).toBeTruthy();
  });
});

describe("sending", () => {
  it("opens a file picker for the pasted ID, then shows who it is waiting for", async () => {
    const device = await start();
    await sendPhoto(device);

    expect(device.api.pickFiles).toHaveBeenCalled();
    expect(device.api.sendFiles).toHaveBeenCalledWith(PEER_ID, ["/tmp/photo.jpg"]);
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    await device.transfer("sender", { kind: "offered" });
    expect(screen.getByText("photo.jpg to K3QF-7XNA")).toBeTruthy();
    expect(screen.getByText("Waiting for K3QF-7XNA…")).toBeTruthy();
  });

  it("sends several chosen files as one Transfer", async () => {
    const pickFiles = vi.fn(() => Promise.resolve<string[] | null>(["/tmp/a.txt", "/tmp/b.txt"]));
    const device = await start(fakeApi({ pickFiles }));
    await sendPhoto(device);

    expect(device.api.sendFiles).toHaveBeenCalledTimes(1);
    expect(device.api.sendFiles).toHaveBeenCalledWith(PEER_ID, ["/tmp/a.txt", "/tmp/b.txt"]);
  });

  it("lets the user choose a folder to send, and sends it", async () => {
    const device = await start();
    fireEvent.click(screen.getByRole("button", { name: "Send to ID…" }));
    fireEvent.change(screen.getByLabelText("Device ID"), { target: { value: PEER_ID } });
    fireEvent.click(screen.getByRole("button", { name: "Choose folder…" }));

    await waitFor(() => expect(device.api.sendFiles).toHaveBeenCalled());
    expect(device.api.pickFolder).toHaveBeenCalled();
    expect(device.api.sendFiles).toHaveBeenCalledWith(PEER_ID, ["/mnt/big"]);
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("sends nothing when the folder picker is cancelled", async () => {
    const device = await start(fakeApi({ pickFolder: vi.fn(() => Promise.resolve(null)) }));
    fireEvent.click(screen.getByRole("button", { name: "Send to ID…" }));
    fireEvent.change(screen.getByLabelText("Device ID"), { target: { value: PEER_ID } });
    fireEvent.click(screen.getByRole("button", { name: "Choose folder…" }));
    await waitFor(() => expect(device.api.pickFolder).toHaveBeenCalled());
    expect(device.api.sendFiles).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog")).toBeTruthy();
  });

  it("needs an ID before it will pick a folder either", async () => {
    await start();
    fireEvent.click(screen.getByRole("button", { name: "Send to ID…" }));
    expect((screen.getByRole("button", { name: "Choose folder…" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("names a Transfer of several things by the first and how many more", async () => {
    const device = await start();
    await device.transfer("sender", { kind: "offered" }, null, {
      name: "docs",
      items: ["docs", "notes.txt", "photo.jpg"],
    });
    expect(screen.getByText("docs and 2 more to K3QF-7XNA")).toBeTruthy();
  });

  it("shows Preparing on the Sender while it hashes, whether or not the Receiver has answered", async () => {
    const device = await start(fakeApi({}, [contact({ nickname: "Mum" })]));
    await screen.findByRole("button", { name: "Send to Mum" });
    await device.transfer("sender", { kind: "offered" });
    await device.push({ type: "preparing", transfer_id: TRANSFER, preparing: true });
    expect(screen.getByText("Preparing the files for Mum…")).toBeTruthy();
    expect(screen.queryByText("Waiting for Mum…")).toBeNull();

    // Done hashing before the answer: back to waiting for it.
    await device.push({ type: "preparing", transfer_id: TRANSFER, preparing: false });
    expect(screen.getByText("Waiting for Mum…")).toBeTruthy();

    // Or the answer comes first: accepted, and still preparing.
    await device.push({ type: "preparing", transfer_id: TRANSFER, preparing: true });
    await device.transfer("sender", { kind: "accepted" });
    expect(screen.getByText("Preparing the files for Mum…")).toBeTruthy();
    await device.transfer("sender", { kind: "transferring" });
    expect(screen.getByText("Sending…")).toBeTruthy();
  });

  it("shows Preparing on the Receiver only once it has accepted", async () => {
    const device = await start(fakeApi({}, [contact({ nickname: "Mum" })]));
    await screen.findByRole("button", { name: "Send to Mum" });
    await device.transfer("receiver", { kind: "offered" });
    await device.push({ type: "preparing", transfer_id: TRANSFER, preparing: true });
    expect(screen.getByText("Waiting for your answer.")).toBeTruthy();

    await device.transfer("receiver", { kind: "accepted" });
    expect(screen.getByText("Accepted. Mum is preparing the files…")).toBeTruthy();
    await device.push({ type: "preparing", transfer_id: TRANSFER, preparing: false });
    expect(screen.getByText("Accepted. Starting…")).toBeTruthy();
  });

  it("tells the Sender how many links were skipped, and not the Receiver", async () => {
    const device = await start();
    await device.transfer("sender", { kind: "offered" }, null, { skipped_links: 3 });
    expect(screen.getByText("3 links skipped")).toBeTruthy();

    await device.push({
      type: "transfer",
      transfer_id: "cd".repeat(16),
      role: "sender",
      peer: PEER_ID,
      peer_name: null,
      kind: "files",
      text: null,
      name: "pack",
      items: ["pack"],
      file_count: 2,
      skipped_links: 1,
      adjusted_names: 0,
      batch_id: null,
      size: 1,
      expires_at: EXPIRES_AT,
      state: { kind: "offered" },
    });
    expect(screen.getByText("1 link skipped")).toBeTruthy();
    cleanup();

    const received = await start();
    await received.transfer("receiver", { kind: "accepted" }, null, { skipped_links: 3 });
    expect(screen.queryByText(/links skipped/)).toBeNull();
  });

  it("sends nothing when the file picker is cancelled", async () => {
    const device = await start(fakeApi({ pickFiles: vi.fn(() => Promise.resolve(null)) }));
    fireEvent.click(screen.getByRole("button", { name: "Send to ID…" }));
    fireEvent.change(screen.getByLabelText("Device ID"), { target: { value: PEER_ID } });
    fireEvent.click(screen.getByRole("button", { name: "Choose files…" }));
    await waitFor(() => expect(device.api.pickFiles).toHaveBeenCalled());
    expect(device.api.sendFiles).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog")).toBeTruthy();
  });

  it("keeps the dialog open and says why when the Device refuses to send", async () => {
    const device = await start(
      fakeApi({ sendFiles: vi.fn(() => Promise.reject("a Device ID is 52 characters of base32")) }),
    );
    await sendPhoto(device);
    expect((await screen.findByRole("alert")).textContent).toContain("Could not send.");
    expect(screen.getByRole("alert").textContent).toContain("52 characters");
    expect(screen.getByRole("dialog")).toBeTruthy();
  });

  it("needs an ID before it will pick a file", async () => {
    await start();
    fireEvent.click(screen.getByRole("button", { name: "Send to ID…" }));
    expect((screen.getByRole("button", { name: "Choose files…" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("closes the dialog on Escape", async () => {
    await start();
    fireEvent.click(screen.getByRole("button", { name: "Send to ID…" }));
    fireEvent.keyDown(screen.getByLabelText("Device ID"), { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("shows progress with a rate, then how it ended", async () => {
    const device = await start();
    await device.transfer("sender", { kind: "offered" });
    await device.transfer("sender", { kind: "accepted" });
    expect(screen.getByText("K3QF-7XNA accepted. Sending…")).toBeTruthy();

    await device.push({ type: "progress", transfer_id: TRANSFER, bytes: 0, total: 2048, at: 10_000 });
    await device.push({ type: "progress", transfer_id: TRANSFER, bytes: 1024, total: 2048, at: 11_000 });
    const row = screen.getByText("photo.jpg to K3QF-7XNA").closest("li")!;
    expect(within(row).getByRole("progressbar", { name: "Progress of photo.jpg" })).toBeTruthy();
    expect(row.textContent).toContain("50% of 2 KiB · 1 KiB/s");

    await device.transfer("sender", { kind: "completed", saved_to: null });
    expect(within(row).getByText("Sent.")).toBeTruthy();
    expect(within(row).queryByRole("progressbar")).toBeNull();
  });

  it("says when the other Device declined, and when it failed", async () => {
    const device = await start();
    await device.transfer("sender", { kind: "offered" });
    await device.transfer("sender", { kind: "declined" });
    expect(screen.getByText("K3QF-7XNA declined.")).toBeTruthy();

    await device.push({
      type: "transfer",
      transfer_id: "cd".repeat(16),
      role: "sender",
      peer: PEER_ID,
      peer_name: null,
      kind: "files",
      text: null,
      name: "b.bin",
      items: ["b.bin"],
      file_count: 1,
      skipped_links: 0,
      adjusted_names: 0,
      batch_id: null,
      size: 1,
      expires_at: EXPIRES_AT,
      state: { kind: "failed", reason: "The other Device went away." },
    });
    expect(screen.getByText("Could not send. The other Device went away.")).toBeTruthy();
  });
});

describe("receiving", () => {
  it("shows the Offer with the Sender's Fingerprint, name, size and save folder", async () => {
    const device = await start();
    await device.transfer("receiver", { kind: "offered" });

    const sheet = await screen.findByRole("dialog", { name: "Incoming files" });
    expect(sheet.textContent).toContain("K3QF-7XNA");
    expect(sheet.textContent).toContain("photo.jpg");
    expect(sheet.textContent).toContain("2 KiB");
    await waitFor(() => expect(sheet.textContent).toContain("/home/me/Downloads/BhayanakShare"));
  });

  it("lists what is offered: the top-level items, the file count and the total size", async () => {
    const device = await start();
    await device.transfer("receiver", { kind: "offered" }, null, {
      name: "album",
      items: ["album", "notes.txt", "todo.md"],
      file_count: 1_234,
    });

    const sheet = await screen.findByRole("dialog", { name: "Incoming files" });
    const items = within(sheet).getAllByRole("listitem").map((li) => li.textContent);
    expect(items).toEqual(["album", "notes.txt", "todo.md"]);
    expect(sheet.textContent).toContain("1234");
    expect(sheet.textContent).toContain("2 KiB");
  });

  it("lists a few items and says how many more there are, however many are offered", async () => {
    const device = await start();
    const items = Array.from({ length: 500 }, (_, i) => `file-${i}.txt`);
    await device.transfer("receiver", { kind: "offered" }, null, { name: items[0], items });

    const sheet = await screen.findByRole("dialog", { name: "Incoming files" });
    expect(within(sheet).getAllByRole("listitem")).toHaveLength(10);
    expect(sheet.textContent).toContain("and 490 more");
  });

  it("makes the page behind the Offer unreachable until it is answered", async () => {
    const device = await start();
    expect(screen.getByRole("main").hasAttribute("inert")).toBe(false);
    await device.transfer("receiver", { kind: "offered" });
    await screen.findByRole("dialog");
    expect(screen.getByRole("main", { hidden: true }).hasAttribute("inert")).toBe(true);
    await device.transfer("receiver", { kind: "declined" });
    expect(screen.getByRole("main").hasAttribute("inert")).toBe(false);
  });

  it("accepts the Offer, and the sheet goes away once it is accepted", async () => {
    const device = await start();
    await device.transfer("receiver", { kind: "offered" });
    fireEvent.click(await screen.findByRole("button", { name: "Accept" }));
    expect(device.api.acceptOffer).toHaveBeenCalledWith(TRANSFER, null);

    await device.transfer("receiver", { kind: "accepted" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByText("Accepted. Starting…")).toBeTruthy();
  });

  it("shows how much is needed and disables Accept when the Offer does not fit", async () => {
    const device = await start(
      fakeApi({ checkOffer: vi.fn(() => Promise.resolve({ needed: 2048, free: 512, paths_too_long: false })) }),
    );
    await device.transfer("receiver", { kind: "offered" });

    expect((await screen.findByRole("alert")).textContent).toBe("Needs 2 KiB, only 512 B free");
    expect((screen.getByRole("button", { name: "Accept" }) as HTMLButtonElement).disabled).toBe(true);
    // Declining is always possible.
    fireEvent.click(screen.getByRole("button", { name: "Decline" }));
    expect(device.api.declineOffer).toHaveBeenCalledWith(TRANSFER);
  });

  it("disables Accept with a warning when the paths are too long for the folder, until another folder is chosen", async () => {
    const checkOffer = vi.fn((_id: string, folder: string | null) =>
      Promise.resolve({ needed: 2048, free: 1_000_000, paths_too_long: folder === null }),
    );
    const device = await start(fakeApi({ checkOffer }));
    await device.transfer("receiver", { kind: "offered" });

    expect((await screen.findByRole("alert")).textContent).toBe(
      "Some paths are too long for this save folder",
    );
    expect((screen.getByRole("button", { name: "Accept" }) as HTMLButtonElement).disabled).toBe(true);

    fireEvent.click(screen.getByRole("button", { name: "Change the save folder for this file" }));
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect((screen.getByRole("button", { name: "Accept" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("tells the Receiver how many names were adjusted, on the Offer and on the Transfer, and not the Sender", async () => {
    const device = await start();
    await device.transfer("receiver", { kind: "offered" }, null, { adjusted_names: 7 });
    // The Offer sheet, and the Transfer's row behind it. Accept stays possible: it is no error.
    expect(await screen.findAllByText("7 names adjusted")).toHaveLength(2);
    expect((screen.getByRole("button", { name: "Accept" }) as HTMLButtonElement).disabled).toBe(false);

    cleanup();

    const one = await start();
    await one.transfer("receiver", { kind: "completed", saved_to: "/home/me/photo.jpg" }, null, {
      adjusted_names: 1,
    });
    expect(screen.getByText("1 name adjusted")).toBeTruthy();
    cleanup();

    const plain = await start();
    await plain.transfer("receiver", { kind: "offered" });
    await screen.findByRole("dialog");
    expect(screen.queryByText(/adjusted/)).toBeNull();
    cleanup();

    const sent = await start();
    await sent.transfer("sender", { kind: "offered" }, null, { adjusted_names: 3 });
    expect(screen.queryByText(/adjusted/)).toBeNull();
  });

  it("leaves Accept on when the free space is unknown", async () => {
    const device = await start(
      fakeApi({ checkOffer: vi.fn(() => Promise.resolve({ needed: 2048, free: null, paths_too_long: false })) }),
    );
    await device.transfer("receiver", { kind: "offered" });
    await waitFor(() => expect(device.api.checkOffer).toHaveBeenCalled());
    expect(screen.queryByRole("alert")).toBeNull();
    expect((screen.getByRole("button", { name: "Accept" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("checks again in the folder chosen for this Offer, and accepts into it", async () => {
    const checkOffer = vi.fn((_id: string, folder: string | null) =>
      Promise.resolve({ needed: 2048, free: folder === null ? 512 : 1_000_000, paths_too_long: false }),
    );
    const device = await start(fakeApi({ checkOffer }));
    await device.transfer("receiver", { kind: "offered" });
    await screen.findByText(/Needs 2 KiB/);
    expect(checkOffer).toHaveBeenLastCalledWith(TRANSFER, null);

    fireEvent.click(screen.getByRole("button", { name: "Change the save folder for this file" }));
    await waitFor(() => expect(screen.queryByText(/Needs 2 KiB/)).toBeNull());
    expect(checkOffer).toHaveBeenLastCalledWith(TRANSFER, "/mnt/big");
    expect(screen.getByRole("dialog").textContent).toContain("/mnt/big");
    const accept = screen.getByRole("button", { name: "Accept" }) as HTMLButtonElement;
    expect(accept.disabled).toBe(false);

    fireEvent.click(accept);
    expect(device.api.acceptOffer).toHaveBeenCalledWith(TRANSFER, "/mnt/big");
  });

  it("keeps the save folder when the folder picker is cancelled", async () => {
    const device = await start(fakeApi({ pickFolder: vi.fn(() => Promise.resolve(null)) }));
    await device.transfer("receiver", { kind: "offered" });
    fireEvent.click(await screen.findByRole("button", { name: "Change the save folder for this file" }));
    await waitFor(() => expect(device.api.pickFolder).toHaveBeenCalled());
    fireEvent.click(screen.getByRole("button", { name: "Accept" }));
    expect(device.api.acceptOffer).toHaveBeenCalledWith(TRANSFER, null);
  });

  it("starts the next Offer on the save folder, not the last Offer's choice", async () => {
    const device = await start();
    await device.transfer("receiver", { kind: "offered" });
    fireEvent.click(await screen.findByRole("button", { name: "Change the save folder for this file" }));
    await screen.findByText("/mnt/big");
    await device.transfer("receiver", { kind: "declined" });

    await device.push({
      type: "transfer",
      transfer_id: "cd".repeat(16),
      role: "receiver",
      peer: PEER_ID,
      peer_name: null,
      kind: "files",
      text: null,
      name: "b.bin",
      items: ["b.bin"],
      file_count: 1,
      skipped_links: 0,
      adjusted_names: 0,
      batch_id: null,
      size: 1,
      expires_at: EXPIRES_AT,
      state: { kind: "offered" },
    });
    await waitFor(() =>
      expect(screen.getByRole("dialog").textContent).toContain("/home/me/Downloads/BhayanakShare"),
    );
  });

  it("declines the Offer", async () => {
    const device = await start();
    await device.transfer("receiver", { kind: "offered" });
    fireEvent.click(await screen.findByRole("button", { name: "Decline" }));
    expect(device.api.declineOffer).toHaveBeenCalledWith(TRANSFER);

    await device.transfer("receiver", { kind: "declined" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByText("You declined.")).toBeTruthy();
  });

  it("says so when the Offer can no longer be answered", async () => {
    const device = await start(
      fakeApi({ acceptOffer: vi.fn(() => Promise.reject("no pending Offer with Transfer ID")) }),
    );
    await device.transfer("receiver", { kind: "offered" });
    fireEvent.click(await screen.findByRole("button", { name: "Accept" }));
    expect((await screen.findByRole("alert")).textContent).toContain("Could not answer the Offer");
  });

  it("shows progress while receiving and offers Show in folder when done", async () => {
    const device = await start();
    await device.transfer("receiver", { kind: "offered" });
    await device.transfer("receiver", { kind: "accepted" });
    await device.transfer("receiver", { kind: "transferring" });
    await device.push({ type: "progress", transfer_id: TRANSFER, bytes: 512, total: 2048, at: 5_000 });
    expect(screen.getByText("Receiving…")).toBeTruthy();
    expect(screen.getByRole("progressbar").getAttribute("value")).toBe("512");

    await device.transfer("receiver", { kind: "saving" });
    await device.transfer("receiver", { kind: "completed", saved_to: "/home/me/Downloads/BhayanakShare/photo.jpg" });
    expect(screen.getByText("Received.")).toBeTruthy();
    expect(screen.getByText(/Saved to \/home\/me\//)).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Show photo.jpg in folder" }));
    expect(device.api.showInFolder).toHaveBeenCalledWith("/home/me/Downloads/BhayanakShare/photo.jpg");
  });

  it("says so when no file manager can show the folder, and clears it on the next try", async () => {
    const showInFolder = vi
      .fn<Api["showInFolder"]>()
      .mockRejectedValueOnce("ServiceUnknown")
      .mockResolvedValueOnce();
    const device = await start(fakeApi({ showInFolder }));
    await device.transfer("receiver", { kind: "completed", saved_to: "/home/me/Downloads/photo.jpg" });

    const button = screen.getByRole("button", { name: "Show photo.jpg in folder" });
    fireEvent.click(button);
    expect((await screen.findByRole("alert")).textContent).toContain("Could not open the folder");

    fireEvent.click(button);
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
  });
});

describe("cancelling, expiry and the Offer countdown", () => {
  it("shows the time left to answer, counting down", async () => {
    vi.useFakeTimers({ toFake: ["Date", "setInterval", "clearInterval"] });
    try {
      vi.setSystemTime(EXPIRES_AT - 581_000);
      const device = await start();
      await device.transfer("receiver", { kind: "offered" });
      const sheet = await screen.findByRole("dialog", { name: "Incoming files" });
      expect(within(sheet).getByRole("timer").textContent).toBe("Expires in 9:41");

      await act(async () => {
        await vi.advanceTimersByTimeAsync(41_000);
      });
      expect(within(sheet).getByRole("timer").textContent).toBe("Expires in 9:00");
    } finally {
      vi.useRealTimers();
    }
  });

  it("shows the Sender while the content is being fetched, and keeps Cancel available", async () => {
    const device = await start();
    await device.transfer("sender", { kind: "offered" });
    await device.transfer("sender", { kind: "accepted" });
    await device.transfer("sender", { kind: "transferring" });
    expect(screen.getByText("Sending…")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Cancel photo.jpg" })).toBeTruthy();
  });

  it("shows a Receiver that lost the Sender as Reconnecting, keeping its progress and Cancel", async () => {
    const device = await start();
    await device.transfer("receiver", { kind: "offered" });
    await device.transfer("receiver", { kind: "accepted" });
    await device.transfer("receiver", { kind: "transferring" });
    await device.push({ type: "progress", transfer_id: TRANSFER, bytes: 512, total: 2048, at: 5_000 });

    await device.transfer("receiver", { kind: "reconnecting" });
    expect(screen.getByText("Lost contact with K3QF-7XNA. Reconnecting…")).toBeTruthy();
    expect(screen.getByRole("progressbar").getAttribute("value")).toBe("512");
    expect(screen.getByRole("button", { name: "Cancel photo.jpg" })).toBeTruthy();

    await device.transfer("receiver", { kind: "transferring" });
    expect(screen.getByText("Receiving…")).toBeTruthy();
  });

  it("lets the Sender cancel a Transfer and says who cancelled", async () => {
    const device = await start();
    await device.transfer("sender", { kind: "offered" });
    fireEvent.click(screen.getByRole("button", { name: "Cancel photo.jpg" }));
    expect(device.api.cancelTransfer).toHaveBeenCalledWith(TRANSFER);

    await device.transfer("sender", { kind: "cancelled", by: "sender" });
    expect(screen.getByText("You cancelled.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Cancel photo.jpg" })).toBeNull();
  });

  it("shows a Receiver that the Sender cancelled, and lets it cancel once it has accepted", async () => {
    const device = await start();
    await device.transfer("receiver", { kind: "offered" });
    expect(screen.queryByRole("button", { name: "Cancel photo.jpg" })).toBeNull();
    await device.transfer("receiver", { kind: "accepted" });
    await device.transfer("receiver", { kind: "transferring" });
    fireEvent.click(screen.getByRole("button", { name: "Cancel photo.jpg" }));
    expect(device.api.cancelTransfer).toHaveBeenCalledWith(TRANSFER);

    await device.transfer("receiver", { kind: "cancelled", by: "sender" });
    expect(screen.getByText("K3QF-7XNA cancelled.")).toBeTruthy();
  });

  it("says so when a Transfer could no longer be cancelled", async () => {
    const device = await start(
      fakeApi({ cancelTransfer: vi.fn(() => Promise.reject("Transfer is not running")) }),
    );
    await device.transfer("sender", { kind: "offered" });
    fireEvent.click(screen.getByRole("button", { name: "Cancel photo.jpg" }));
    expect((await screen.findByRole("alert")).textContent).toContain("can no longer be cancelled");
  });

  it("shows an expired Offer on both sides, with one-click resend for the Sender", async () => {
    const device = await start();
    await device.transfer("sender", { kind: "offered" });
    await device.transfer("sender", { kind: "expired" });
    expect(screen.getByText("K3QF-7XNA did not answer in time. The Offer expired.")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Send photo.jpg again" }));
    expect(device.api.resendTransfer).toHaveBeenCalledWith(TRANSFER);
    // Once sent again, the expired row no longer offers it.
    await waitFor(() => expect(screen.queryByRole("button", { name: "Send photo.jpg again" })).toBeNull());
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("gives the Receiver of an expired Offer no resend, and closes the sheet", async () => {
    const device = await start();
    await device.transfer("receiver", { kind: "offered" });
    await screen.findByRole("dialog");
    await device.transfer("receiver", { kind: "expired" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByText("The Offer expired before you answered.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /again/ })).toBeNull();
  });

  it("says so when an Offer cannot be sent again", async () => {
    const device = await start(
      fakeApi({ resendTransfer: vi.fn(() => Promise.reject("/tmp/photo.jpg is not a file")) }),
    );
    await device.transfer("sender", { kind: "expired" });
    fireEvent.click(screen.getByRole("button", { name: "Send photo.jpg again" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Could not send it again");
    expect(alert.textContent).toContain("is not a file");
  });

  it("shows the Sender that the Receiver was busy, as a failure with its reason", async () => {
    const device = await start();
    await device.push({
      type: "transfer",
      transfer_id: TRANSFER,
      role: "sender",
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
      state: { kind: "failed", reason: "The other Device already has too many Offers from you." },
    });
    expect(screen.getByText(/Could not send\. The other Device already has too many Offers/)).toBeTruthy();
  });
});

describe("Contacts", () => {
  const open = async (device = fakeApi()) => {
    await start(device);
    fireEvent.click(screen.getByRole("button", { name: "Contacts" }));
    return device;
  };
  const addContact = async (id = PEER_ID, name = "") => {
    fireEvent.click(screen.getByRole("button", { name: "Add Contact…" }));
    fireEvent.change(screen.getByLabelText("Device ID"), { target: { value: ` ${id} ` } });
    if (name) fireEvent.change(screen.getByLabelText("Name (optional)"), { target: { value: name } });
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
  };

  it("says so when there are none", async () => {
    await open();
    expect(await screen.findByText(/No Contacts yet/)).toBeTruthy();
  });

  it("asks the user to check the Fingerprint before it adds anything", async () => {
    const device = await open();
    await addContact(PEER_ID, "Mum's laptop");

    const dialog = screen.getByRole("dialog", { name: "Check the Fingerprint" });
    expect(within(dialog).getByText("K3QF-7XNA")).toBeTruthy();
    expect(dialog.textContent).toContain("read out its Fingerprint");
    expect(device.api.addContact).not.toHaveBeenCalled();

    fireEvent.click(within(dialog).getByRole("button", { name: "It matches, add Contact" }));
    await waitFor(() => expect(device.api.addContact).toHaveBeenCalledWith(PEER_ID, "Mum's laptop"));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(await screen.findByRole("heading", { name: /Mum's laptop/ })).toBeTruthy();
  });

  it("shows the Fingerprint in capitals however the ID was pasted", async () => {
    await open();
    await addContact(PEER_ID.toLowerCase());
    expect(within(screen.getByRole("dialog")).getByText("K3QF-7XNA")).toBeTruthy();
  });

  it("can go back from the Fingerprint check, and adds nothing if cancelled", async () => {
    const device = await open();
    await addContact();
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    expect((screen.getByLabelText("Device ID") as HTMLInputElement).value).toBe(` ${PEER_ID} `);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(device.api.addContact).not.toHaveBeenCalled();
  });

  it("will not go on with something that is not a Device ID", async () => {
    await open();
    fireEvent.click(screen.getByRole("button", { name: "Add Contact…" }));
    fireEvent.change(screen.getByLabelText("Device ID"), { target: { value: "K3QF-7XNA" } });
    expect((screen.getByRole("button", { name: "Next" }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByRole("alert").textContent).toContain("not a Device ID");
  });

  it("keeps the dialog open and says why when the Contact cannot be added", async () => {
    await open(fakeApi({ addContact: vi.fn(() => Promise.reject("K3QF-7XNA is already a Contact")) }));
    await addContact();
    fireEvent.click(screen.getByRole("button", { name: "It matches, add Contact" }));
    expect((await screen.findByRole("alert")).textContent).toContain("already a Contact");
    expect(screen.getByRole("dialog")).toBeTruthy();
  });

  it("lists the Nickname, Device Name, Fingerprint and Auto-accept, off by default", async () => {
    await open(fakeApi({}, [contact({ nickname: "Mum", device_name: "DESKTOP-7" })]));
    const row = (await screen.findByRole("heading", { name: /Mum/ })).closest("li")!;
    expect((within(row).getByLabelText("Nickname") as HTMLInputElement).value).toBe("Mum");
    expect(row.textContent).toContain("DESKTOP-7");
    expect(row.textContent).toContain("K3QF-7XNA");
    expect((within(row).getByLabelText("Auto-accept") as HTMLInputElement).checked).toBe(false);
  });

  it("shows the Nickname instead of the Device Name, and the Device Name again once it is cleared", async () => {
    const device = await open(fakeApi({}, [contact({ device_name: "DESKTOP-7" })]));
    await screen.findByRole("heading", { name: /DESKTOP-7/ });

    fireEvent.change(screen.getByLabelText("Nickname"), { target: { value: "Mum" } });
    fireEvent.click(screen.getByRole("button", { name: "Save the Nickname for DESKTOP-7" }));
    expect(await screen.findByRole("heading", { name: /Mum/ })).toBeTruthy();
    expect(device.api.setNickname).toHaveBeenCalledWith(PEER_ID, "Mum");
    expect(screen.getByText("DESKTOP-7")).toBeTruthy(); // still listed as the Device Name

    fireEvent.change(screen.getByLabelText("Nickname"), { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: "Save the Nickname for Mum" }));
    expect(await screen.findByRole("heading", { name: /DESKTOP-7/ })).toBeTruthy();
  });

  it("turns Auto-accept on and off", async () => {
    const device = await open(fakeApi({}, [contact({ device_name: "DESKTOP-7" })]));
    const box = (await screen.findByLabelText("Auto-accept")) as HTMLInputElement;
    fireEvent.click(box);
    await waitFor(() => expect(device.api.setAutoAccept).toHaveBeenCalledWith(PEER_ID, true));
    await waitFor(() => expect(box.checked).toBe(true));
    fireEvent.click(box);
    await waitFor(() => expect(device.api.setAutoAccept).toHaveBeenLastCalledWith(PEER_ID, false));
  });

  it("asks before removing a Contact, and says its Transfers stay", async () => {
    const device = await open(fakeApi({}, [contact({ nickname: "Mum" })]));
    fireEvent.click(await screen.findByRole("button", { name: "Remove Mum from Contacts" }));

    const dialog = screen.getByRole("alertdialog", { name: "Remove Mum?" });
    expect(dialog.textContent).toContain("stay in your Transfer History");
    expect(document.activeElement).toBe(within(dialog).getByRole("button", { name: "Keep" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Keep" }));
    expect(device.api.removeContact).not.toHaveBeenCalled();
    expect(screen.queryByRole("alertdialog")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Remove Mum from Contacts" }));
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(device.api.removeContact).toHaveBeenCalledWith(PEER_ID));
    expect(await screen.findByText(/No Contacts yet/)).toBeTruthy();
  });

  it("puts Contacts first on Home, badged, ahead of Send to ID", async () => {
    await start(fakeApi({}, [contact({ nickname: "Mum" })]));
    const tiles = await screen.findAllByRole("button", { name: /^Send to / });
    expect(tiles.map((x) => x.getAttribute("aria-label") ?? x.textContent)).toEqual([
      "Send to Mum",
      "Send to ID…",
    ]);
    expect(tiles[0].textContent).toContain("Contact");
    expect(tiles[0].textContent).toContain("K3QF-7XNA");
  });

  it("sends to a Contact's tile with their ID filled in", async () => {
    await start(fakeApi({}, [contact({ nickname: "Mum" })]));
    fireEvent.click(await screen.findByRole("button", { name: "Send to Mum" }));
    expect(screen.getByRole("dialog", { name: "Send to Mum" })).toBeTruthy();
    expect((screen.getByLabelText("Device ID") as HTMLInputElement).value).toBe(PEER_ID);
  });

  it("names a Contact in the Transfer list, and a stranger by Fingerprint", async () => {
    const device = await start(fakeApi({}, [contact({ nickname: "Mum" })]));
    await screen.findByRole("button", { name: "Send to Mum" });
    await device.transfer("sender", { kind: "offered" });
    expect(screen.getByText("photo.jpg to Mum")).toBeTruthy();
    expect(screen.getByText("Waiting for Mum…")).toBeTruthy();

    await device.push({
      type: "transfer",
      transfer_id: "cd".repeat(16),
      role: "sender",
      peer: "Z".repeat(52),
      peer_name: null,
      kind: "files",
      text: null,
      name: "b.bin",
      items: ["b.bin"],
      file_count: 1,
      skipped_links: 0,
      adjusted_names: 0,
      batch_id: null,
      size: 1,
      expires_at: EXPIRES_AT,
      state: { kind: "offered" },
    });
    expect(screen.getByText("b.bin to ZZZZ-ZZZZ")).toBeTruthy();
  });

  it("shows an Offer from a Contact by name, badge and Fingerprint", async () => {
    const device = await start(fakeApi({}, [contact({ nickname: "Mum", device_name: "DESKTOP-7" })]));
    await screen.findByRole("button", { name: "Send to Mum" });
    await device.transfer("receiver", { kind: "offered" });
    const sheet = await screen.findByRole("dialog", { name: "Incoming files" });
    expect(sheet.textContent).toContain("Mum");
    expect(sheet.textContent).not.toContain("DESKTOP-7");
    expect(sheet.textContent).toContain("Contact");
    expect(sheet.textContent).toContain("K3QF-7XNA");
  });

  it("shows an Offer from a non-Contact as one, with its Fingerprint", async () => {
    const device = await start();
    await device.transfer("receiver", { kind: "offered" });
    const sheet = await screen.findByRole("dialog", { name: "Incoming files" });
    expect(sheet.textContent).toContain("Not in your Contacts");
    expect(sheet.textContent).toContain("K3QF-7XNA");
  });

  it("looks at the Contacts again when a Transfer arrives, as a connection may have refreshed one", async () => {
    let name = "old name";
    const api = fakeApi({
      contacts: vi.fn(() => Promise.resolve([contact({ device_name: name })])),
    });
    await start(api);
    await screen.findByRole("button", { name: "Send to old name" });

    name = "new name"; // refreshed by the connection that brought the Offer
    await api.transfer("receiver", { kind: "offered" });
    expect(await screen.findByRole("dialog", { name: "Incoming files" })).toBeTruthy();
    await waitFor(() => expect(screen.getByRole("dialog").textContent).toContain("new name"));
  });

  it("shows an Offer from a non-Contact by the name it announced, plus its Fingerprint", async () => {
    const device = await start();
    await device.transfer("receiver", { kind: "offered" }, "Alice's desktop");
    const sheet = await screen.findByRole("dialog", { name: "Incoming files" });
    expect(within(sheet).getByText(/Alice's desktop/).textContent).toContain("Not in your Contacts");
    expect(sheet.textContent).toContain("K3QF-7XNA");
  });

  it("prefers the Nickname, then the Device Name, over what a Contact announces", async () => {
    const device = await start(
      fakeApi({}, [contact({ nickname: null, device_name: "DESKTOP-7" })]),
    );
    await screen.findByRole("button", { name: "Send to DESKTOP-7" });
    await device.transfer("receiver", { kind: "offered" }, "Someone else");
    const sheet = await screen.findByRole("dialog", { name: "Incoming files" });
    expect(sheet.textContent).toContain("DESKTOP-7");
    expect(sheet.textContent).not.toContain("Someone else");
  });

  it("names a non-Contact in the Transfer list as name · Fingerprint, and keeps the name it learned", async () => {
    const device = await start();
    // A Sender learns the Receiver's name only once it has connected, so the first event has none.
    await device.transfer("sender", { kind: "offered" });
    expect(screen.getByText("photo.jpg to K3QF-7XNA")).toBeTruthy();
    await device.transfer("sender", { kind: "accepted" }, "Bob's laptop");
    expect(screen.getByText("photo.jpg to Bob's laptop · K3QF-7XNA")).toBeTruthy();
    expect(screen.getByText("Bob's laptop · K3QF-7XNA accepted. Sending…")).toBeTruthy();
    await device.transfer("sender", { kind: "completed", saved_to: null });
    expect(screen.getByText("photo.jpg to Bob's laptop · K3QF-7XNA")).toBeTruthy();
  });
});

describe("Nearby Devices", () => {
  const OTHER_ID = "ZZZZ2222" + "C".repeat(44);

  it("shows a Nearby non-Contact as its name and Fingerprint, ahead of Send to ID", async () => {
    const device = await start();
    await device.nearby({ id: PEER_ID, name: "Dad's PC" });
    const tile = await screen.findByRole("button", { name: "Send to Dad's PC · K3QF-7XNA" });
    expect(tile.textContent).toContain("Dad's PC");
    expect(tile.textContent).toContain("K3QF-7XNA");
    expect(tile.textContent).toContain("Nearby");
    expect(tile.textContent).not.toContain("Contact");
    const tiles = screen.getAllByRole("button", { name: /^Send to / });
    expect(tiles.map((x) => x.getAttribute("aria-label") ?? x.textContent)).toEqual([
      "Send to Dad's PC · K3QF-7XNA",
      "Send to ID…",
    ]);
  });

  it("shows a Nearby Device that announced no name by its Fingerprint alone", async () => {
    const device = await start();
    await device.nearby({ id: PEER_ID, name: null });
    expect(await screen.findByRole("button", { name: "Send to K3QF-7XNA" })).toBeTruthy();
  });

  it("matches a Nearby Contact by Device ID: one badged tile, marked Nearby, under its own name", async () => {
    const device = await start(fakeApi({}, [contact({ nickname: "Mum" })]));
    await screen.findByRole("button", { name: "Send to Mum" });
    await device.nearby({ id: PEER_ID, name: "Some other name" }, { id: OTHER_ID, name: "Dad" });

    const tiles = await screen.findAllByRole("button", { name: /^Send to / });
    expect(tiles.map((x) => x.getAttribute("aria-label") ?? x.textContent)).toEqual([
      "Send to Mum",
      "Send to Dad · ZZZZ-2222",
      "Send to ID…",
    ]);
    expect(tiles[0].textContent).toContain("Contact");
    expect(tiles[0].textContent).toContain("Nearby");
    // Only the stranger can be saved.
    expect(screen.getAllByRole("button", { name: /^Save .* as a Contact$/ })).toHaveLength(1);
  });

  it("uses the announced name for a Contact that has none yet, and shows an offline Contact without Nearby", async () => {
    const device = await start(fakeApi({}, [contact({ nickname: null, device_name: null })]));
    const offline = await screen.findByRole("button", { name: "Send to K3QF-7XNA" });
    expect(offline.textContent).not.toContain("Nearby");
    await device.nearby({ id: PEER_ID, name: "Mum's phone" });
    expect(await screen.findByRole("button", { name: "Send to Mum's phone" })).toBeTruthy();
  });

  it("drops a tile when the Device is no longer Nearby", async () => {
    const device = await start();
    await device.nearby({ id: PEER_ID, name: "Dad's PC" });
    await screen.findByRole("button", { name: /^Send to Dad's PC/ });
    await device.nearby();
    expect(screen.queryByRole("button", { name: /^Send to Dad's PC/ })).toBeNull();
  });

  it("sends to a Nearby tile with its ID filled in", async () => {
    const device = await start();
    await device.nearby({ id: PEER_ID, name: "Dad's PC" });
    fireEvent.click(await screen.findByRole("button", { name: /^Send to Dad's PC/ }));
    expect(screen.getByRole("dialog", { name: "Send to Dad's PC · K3QF-7XNA" })).toBeTruthy();
    expect((screen.getByLabelText("Device ID") as HTMLInputElement).value).toBe(PEER_ID);
  });

  it("saves a Nearby tile as a Contact after checking the Fingerprint, with the name it announced", async () => {
    const device = await start();
    await device.nearby({ id: PEER_ID, name: "Dad's PC" });
    fireEvent.click(await screen.findByRole("button", { name: /^Save Dad's PC · K3QF-7XNA as a Contact$/ }));

    // Straight to the check: the ID was not typed, but the Fingerprint is still compared.
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText("K3QF-7XNA")).toBeTruthy();
    expect(device.api.addContact).not.toHaveBeenCalled();

    fireEvent.click(within(dialog).getByRole("button", { name: "It matches, add Contact" }));
    await waitFor(() => expect(device.api.addContact).toHaveBeenCalledWith(PEER_ID, "Dad's PC"));
    // Now a Contact: its tile is badged and the Save button is gone.
    expect(await screen.findByRole("button", { name: "Send to Dad's PC" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /as a Contact$/ })).toBeNull();
  });

  it("saves nothing when the Fingerprint check is cancelled", async () => {
    const device = await start();
    await device.nearby({ id: PEER_ID, name: null });
    fireEvent.click(await screen.findByRole("button", { name: /^Save K3QF-7XNA as a Contact$/ }));
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(device.api.addContact).not.toHaveBeenCalled();
  });
});

describe("the firewall hint", () => {
  const HINT = /No Devices found on this network yet/;
  const DOCS = "https://github.com/theHimanshuShekhar/bhayanakshare/blob/main/docs/firewall.md";

  /**
   * Takes over the 30-second wait and leaves every other timer real (a fake clock would also
   * freeze the ones Testing Library waits on). `elapse` lets the wait run out; `delays` are the
   * waits the app asked for.
   */
  function holdTheWait() {
    const real = globalThis.setTimeout;
    const delays: number[] = [];
    let fire = () => {};
    vi.spyOn(globalThis, "setTimeout").mockImplementation(((
      fn: () => void,
      ms?: number,
      ...args: unknown[]
    ) => {
      if (ms !== NEARBY_WAIT_MS) return real(fn, ms, ...args);
      delays.push(ms);
      fire = fn;
      return 0;
    }) as unknown as typeof setTimeout);
    return { delays, elapse: () => act(() => fire()) };
  }

  afterEach(() => vi.restoreAllMocks());

  it("appears once the 30-second wait is over with nobody Nearby, and links to the docs", async () => {
    const wait = holdTheWait();
    const device = await start();
    expect(wait.delays).toEqual([30_000]);
    expect(screen.queryByText(HINT)).toBeNull();
    await wait.elapse();
    expect(screen.getByText(HINT)).toBeTruthy();

    const link = screen.getByRole("link", { name: "How to allow local discovery" });
    expect(link.getAttribute("href")).toBe(DOCS);
    fireEvent.click(link);
    // Opened in the browser, not in the app's own window.
    expect(device.api.openUrl).toHaveBeenCalledWith(DOCS);
  });

  it("does not appear when a Device was found in time", async () => {
    const wait = holdTheWait();
    const device = await start();
    await device.nearby({ id: PEER_ID, name: "Dad's PC" });
    await wait.elapse();
    expect(screen.queryByText(HINT)).toBeNull();
  });

  it("goes away when a Device turns up, and comes back if they all leave", async () => {
    const wait = holdTheWait();
    const device = await start();
    await wait.elapse();
    expect(screen.getByText(HINT)).toBeTruthy();
    await device.nearby({ id: PEER_ID, name: "Dad's PC" });
    expect(screen.queryByText(HINT)).toBeNull();
    await device.nearby();
    expect(screen.getByText(HINT)).toBeTruthy();
  });
});

describe("Visibility", () => {
  const open = async (device = fakeApi()) => {
    await start(device);
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    return device;
  };

  it("offers the three settings, each with a one-line explanation, and marks the current one", async () => {
    await open();
    const group = await screen.findByRole("group", { name: "Who can see this Device nearby" });
    const radios = within(group).getAllByRole("radio");
    expect(radios.map((r) => r.getAttribute("id"))).toEqual([
      "visibility-everyone",
      "visibility-id_holders",
      "visibility-hidden",
    ]);
    await waitFor(() => expect((radios[1] as HTMLInputElement).checked).toBe(true));
    expect(radios.map((r) => (r as HTMLInputElement).checked)).toEqual([false, true, false]);

    // Named by the setting, and described: the explanation is what a screen reader reads next.
    expect(within(group).getByRole("radio", { name: "Everyone" })).toBeTruthy();
    expect(within(group).getByRole("radio", { name: "People who have my ID" })).toBeTruthy();
    expect(within(group).getByRole("radio", { name: "Hidden" })).toBeTruthy();
    const described = radios.map(
      (r) => document.getElementById(r.getAttribute("aria-describedby") ?? "")?.textContent,
    );
    expect(described).toEqual([
      "Anyone on your network can see this Device and its name.",
      "Only Devices that already have your Device ID can see it and its name.",
      "No one sees this Device on your network. People with your ID can still send to it.",
    ]);
  });

  it("changes the setting and shows the new one", async () => {
    const device = await open();
    const everyone = await screen.findByRole("radio", { name: "Everyone" });
    await waitFor(() => expect((everyone as HTMLInputElement).disabled).toBe(false));
    fireEvent.click(everyone);
    await waitFor(() => expect(device.api.setVisibility).toHaveBeenCalledWith("everyone"));
    await waitFor(() => expect((everyone as HTMLInputElement).checked).toBe(true));
    expect(
      (screen.getByRole("radio", { name: "People who have my ID" }) as HTMLInputElement).checked,
    ).toBe(false);
  });

  it("reads the stored setting when Settings is opened again", async () => {
    const device = await open();
    const hiddenAtFirst = await screen.findByRole("radio", { name: "Hidden" });
    await waitFor(() => expect((hiddenAtFirst as HTMLInputElement).disabled).toBe(false));
    fireEvent.click(hiddenAtFirst);
    await waitFor(() => expect(device.api.setVisibility).toHaveBeenCalledWith("hidden"));
    fireEvent.click(screen.getByRole("button", { name: "Home" }));
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    const hidden = await screen.findByRole("radio", { name: "Hidden" });
    await waitFor(() => expect((hidden as HTMLInputElement).checked).toBe(true));
  });

  it("keeps the old setting and says why when the change is refused", async () => {
    const device = await open(
      fakeApi({ setVisibility: vi.fn(() => Promise.reject(new Error("the Device is shutting down"))) }),
    );
    const everyone = await screen.findByRole("radio", { name: "Everyone" });
    await waitFor(() => expect((everyone as HTMLInputElement).disabled).toBe(false));
    fireEvent.click(everyone);
    expect((await screen.findByRole("alert")).textContent).toContain("the Device is shutting down");
    expect((everyone as HTMLInputElement).checked).toBe(false);
    expect(
      (screen.getByRole("radio", { name: "People who have my ID" }) as HTMLInputElement).checked,
    ).toBe(true);
    expect(device.api.setVisibility).toHaveBeenCalledTimes(1);
  });

  it("says so, and offers no choice, when the setting cannot be read", async () => {
    await open(fakeApi({ visibility: () => Promise.reject(new Error("no")) }));
    expect((await screen.findByRole("alert")).textContent).toContain("Could not read");
    for (const radio of screen.getAllByRole("radio")) {
      expect((radio as HTMLInputElement).disabled).toBe(true);
    }
  });
});

describe("Start at login", () => {
  const open = async (device = fakeApi()) => {
    await start(device);
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    return device;
  };

  it("shows it on by default and switches it off", async () => {
    const device = await open();
    const box = (await screen.findByRole("checkbox", { name: "Start at login" })) as HTMLInputElement;
    await waitFor(() => expect(box.checked).toBe(true));
    fireEvent.click(box);
    await waitFor(() => expect(device.api.setAutostart).toHaveBeenCalledWith(false));
    await waitFor(() => expect(box.checked).toBe(false));
  });

  it("keeps the old setting and says why when the change is refused", async () => {
    await open(fakeApi({ setAutostart: () => Promise.reject(new Error("read-only folder")) }));
    const box = (await screen.findByRole("checkbox", { name: "Start at login" })) as HTMLInputElement;
    await waitFor(() => expect(box.checked).toBe(true));
    fireEvent.click(box);
    expect((await screen.findByRole("alert")).textContent).toContain("read-only folder");
    expect(box.checked).toBe(true);
  });
});

describe("files from a second launch or the tray", () => {
  it("waits for the user to choose a Device, then sends each file without asking for one", async () => {
    const device = await start(fakeApi({}, [contact({ device_name: "Mum's phone" })]));
    await device.shell({ type: "send_files", paths: ["/home/me/a.txt", "/home/me/b.txt"] });
    expect((await screen.findByText(/Choose a Device to send/)).textContent).toContain("a.txt, b.txt");

    fireEvent.click(screen.getByRole("button", { name: "Send to Mum's phone" }));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText(/a\.txt, b\.txt/)).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Send" }));

    await waitFor(() => expect(device.api.sendFiles).toHaveBeenCalledTimes(2));
    expect(device.api.sendFiles).toHaveBeenNthCalledWith(1, PEER_ID, ["/home/me/a.txt"]);
    expect(device.api.sendFiles).toHaveBeenNthCalledWith(2, PEER_ID, ["/home/me/b.txt"]);
    expect(device.api.pickFiles).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(screen.queryByText(/Choose a Device to send/)).toBeNull();
  });

  it("keeps the files that were not sent when one fails", async () => {
    const sendFile = vi
      .fn<Api["sendFiles"]>()
      .mockResolvedValueOnce(TRANSFER)
      .mockRejectedValueOnce(new Error("unreachable"))
      .mockResolvedValue(TRANSFER);
    const device = await start(fakeApi({ sendFiles: sendFile }, [contact()]));
    await device.shell({ type: "send_files", paths: ["/x/a.txt", "/x/b.txt"] });
    fireEvent.click(screen.getByRole("button", { name: "Send to K3QF-7XNA" }));
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    expect((await screen.findByRole("alert")).textContent).toContain("unreachable");
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText(/b\.txt/).textContent).not.toContain("a.txt");

    fireEvent.click(within(dialog).getByRole("button", { name: "Send" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(sendFile).toHaveBeenLastCalledWith(PEER_ID, ["/x/b.txt"]);
  });

  it("forgets the files when asked", async () => {
    const device = await start();
    await device.shell({ type: "send_files", paths: ["/x/a.txt"] });
    fireEvent.click(await screen.findByRole("button", { name: "Forget these files" }));
    expect(screen.queryByText(/Choose a Device to send/)).toBeNull();
  });
});

describe("a clicked notification", () => {
  it("shows that Offer ahead of an older one", async () => {
    const device = await start();
    const other = "cd".repeat(16);
    await device.push({
      type: "transfer",
      transfer_id: other,
      role: "receiver",
      peer: PEER_ID,
      peer_name: null,
      kind: "files",
      text: null,
      name: "older.txt",
      items: ["older.txt"],
      file_count: 1,
      skipped_links: 0,
      adjusted_names: 0,
      batch_id: null,
      size: 1,
      expires_at: EXPIRES_AT,
      state: { kind: "offered" },
    });
    await device.transfer("receiver", { kind: "offered" });
    expect(screen.getByRole("dialog").textContent).toContain("older.txt");

    await device.shell({ type: "open_offer", transfer_id: TRANSFER });
    expect(screen.getByRole("dialog").textContent).toContain("photo.jpg");
  });
});

describe("quitting", () => {
  it("asks first, saying the Transfers resume, and keeps running when told to", async () => {
    const device = await start();
    await device.shell({ type: "confirm_quit", active: 2 });
    const dialog = screen.getByRole("dialog", { name: "Quit BhayanakShare?" });
    expect(dialog.textContent).toContain("2 Transfers are in progress.");
    expect(dialog.textContent).toContain("They'll resume when BhayanakShare next runs.");

    fireEvent.click(within(dialog).getByRole("button", { name: "Keep running" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(device.api.quitApp).not.toHaveBeenCalled();
  });

  it("says one Transfer in the singular", async () => {
    const device = await start();
    await device.shell({ type: "confirm_quit", active: 1 });
    expect(screen.getByRole("dialog").textContent).toContain("A Transfer is in progress.");
    expect(screen.getByRole("dialog").textContent).toContain("It'll resume");
  });

  it("quits on confirmation and shows Saving progress with nothing left to press", async () => {
    const device = await start();
    await device.shell({ type: "confirm_quit", active: 1 });
    fireEvent.click(screen.getByRole("button", { name: "Quit" }));
    expect(device.api.quitApp).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("dialog", { name: "Saving progress…" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Keep running" })).toBeNull();

    // The shell's own word that shutdown began changes nothing.
    await device.shell({ type: "quitting" });
    expect(screen.getByRole("dialog", { name: "Saving progress…" })).toBeTruthy();
  });

  it("shows Saving progress when the shell began the shutdown", async () => {
    const device = await start();
    await device.shell({ type: "quitting" });
    expect(screen.getByRole("dialog", { name: "Saving progress…" })).toBeTruthy();
  });

  it("goes back to the question and says why when quitting fails", async () => {
    const device = await start(fakeApi({ quitApp: () => Promise.reject(new Error("busy")) }));
    await device.shell({ type: "confirm_quit", active: 1 });
    fireEvent.click(screen.getByRole("button", { name: "Quit" }));
    expect((await screen.findByRole("alert")).textContent).toContain("busy");
    expect(screen.getByRole("button", { name: "Keep running" })).toBeTruthy();
  });
});

describe("Batches", () => {
  const MUM = PEER_ID;
  const DAD = "Q2WERTYU" + "C".repeat(44);
  const SIS = "ZZZZ7777" + "D".repeat(44);
  const contacts = [
    contact({ id: MUM, nickname: "Mum", added_at: 1 }),
    contact({ id: DAD, nickname: "Dad", added_at: 2 }),
    contact({ id: SIS, nickname: "Sis", added_at: 3 }),
  ];
  const select = (name: string) =>
    fireEvent.click(screen.getByRole("checkbox", { name: `Select ${name} to send to several Devices at once` }));

  /** One Transfer of the Batch, as the Sender's Device reports it. */
  const member = (device: ReturnType<typeof fakeApi>, n: number, peer: string, state: TransferState) =>
    device.push({
      type: "transfer",
      transfer_id: String(n).repeat(32),
      role: "sender",
      peer,
      peer_name: null,
      kind: "files",
      text: null,
      name: "photo.jpg",
      items: ["photo.jpg"],
      file_count: 1,
      skipped_links: 0,
      adjusted_names: 0,
      batch_id: BATCH,
      size: 2048,
      expires_at: EXPIRES_AT,
      state,
    });

  it("sends the chosen files to every selected Device as one Batch", async () => {
    const device = await start(fakeApi({}, contacts));
    await screen.findByRole("button", { name: "Send to Mum" });
    select("Mum");
    expect(screen.getByText("1 Device selected")).toBeTruthy();
    select("Dad");
    expect(screen.getByText("2 Devices selected")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Choose files…" }));

    await waitFor(() => expect(device.api.sendBatch).toHaveBeenCalled());
    expect(device.api.sendBatch).toHaveBeenCalledWith([MUM, DAD], ["/tmp/photo.jpg"]);
    expect(device.api.sendFiles).not.toHaveBeenCalled();
    // Done: nothing stays selected.
    await waitFor(() => expect(screen.queryByText("2 Devices selected")).toBeNull());
  });

  it("sends a folder to one selected Device as an ordinary Transfer", async () => {
    const device = await start(fakeApi({}, contacts));
    await screen.findByRole("button", { name: "Send to Mum" });
    select("Dad");
    fireEvent.click(screen.getByRole("button", { name: "Choose folder…" }));
    await waitFor(() => expect(device.api.sendFiles).toHaveBeenCalled());
    expect(device.api.sendFiles).toHaveBeenCalledWith(DAD, ["/mnt/big"]);
    expect(device.api.sendBatch).not.toHaveBeenCalled();
  });

  it("can untick a Device and clear the selection, and sends nothing if the picker is cancelled", async () => {
    const pickFiles = vi.fn(() => Promise.resolve<string[] | null>(null));
    const device = await start(fakeApi({ pickFiles }, contacts));
    await screen.findByRole("button", { name: "Send to Mum" });
    select("Mum");
    select("Dad");
    select("Mum");
    expect(screen.getByText("1 Device selected")).toBeTruthy();
    select("Mum");
    fireEvent.click(screen.getByRole("button", { name: "Choose files…" }));
    await waitFor(() => expect(pickFiles).toHaveBeenCalled());
    expect(device.api.sendBatch).not.toHaveBeenCalled();
    expect(screen.getByText("2 Devices selected")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Clear selection" }));
    expect(screen.queryByText("2 Devices selected")).toBeNull();
  });

  it("says why the Batch could not be sent, and keeps the selection", async () => {
    const sendBatch = vi.fn(() => Promise.reject(new Error("no")));
    await start(fakeApi({ sendBatch }, contacts));
    await screen.findByRole("button", { name: "Send to Mum" });
    select("Mum");
    select("Dad");
    fireEvent.click(screen.getByRole("button", { name: "Choose files…" }));
    expect((await screen.findByRole("alert")).textContent).toContain("Could not send. no");
    expect(screen.getByText("2 Devices selected")).toBeTruthy();
  });

  it("offers files that are waiting to be sent to the selection, each as a Batch", async () => {
    const device = await start(fakeApi({}, contacts));
    await screen.findByRole("button", { name: "Send to Mum" });
    await device.shell({ type: "send_files", paths: ["/tmp/a.txt", "/tmp/b.txt"] });
    select("Mum");
    select("Dad");

    fireEvent.click(screen.getByRole("button", { name: "Send a.txt, b.txt" }));

    await waitFor(() => expect(device.api.sendBatch).toHaveBeenCalledTimes(2));
    expect(device.api.sendBatch).toHaveBeenNthCalledWith(1, [MUM, DAD], ["/tmp/a.txt"]);
    expect(device.api.sendBatch).toHaveBeenNthCalledWith(2, [MUM, DAD], ["/tmp/b.txt"]);
    await waitFor(() => expect(screen.queryByText(/Choose a Device to send/)).toBeNull());
  });

  it("shows a Batch as one row with its overall status, and each Device when it is opened", async () => {
    const device = await start(fakeApi({}, contacts));
    await screen.findByRole("button", { name: "Send to Mum" });
    await member(device, 1, MUM, { kind: "completed", saved_to: null });
    await member(device, 2, DAD, { kind: "declined" });
    await member(device, 3, SIS, { kind: "waiting" });

    expect(screen.getByText("photo.jpg to 3 Devices")).toBeTruthy();
    expect(screen.getByText("1 of 3 delivered, 1 declined, 1 in progress")).toBeTruthy();
    expect(screen.queryByText("Dad declined.")).toBeNull();

    const toggle = screen.getByRole("button", { name: /Show each Device/ });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByText("photo.jpg to Mum")).toBeTruthy();
    expect(screen.getByText("Dad declined.")).toBeTruthy();
    expect(screen.getByText("Sis accepted. Waiting for a turn to send…")).toBeTruthy();

    fireEvent.click(toggle);
    expect(screen.queryByText("Dad declined.")).toBeNull();
  });

  it("cancels one Device of a Batch, or all of them", async () => {
    const device = await start(fakeApi({}, contacts));
    await screen.findByRole("button", { name: "Send to Mum" });
    await member(device, 1, MUM, { kind: "transferring" });
    await member(device, 2, DAD, { kind: "waiting" });
    fireEvent.click(screen.getByRole("button", { name: /Show each Device/ }));

    fireEvent.click(screen.getByRole("button", { name: "Cancel all of photo.jpg" }));
    expect(device.api.cancelBatch).toHaveBeenCalledWith(BATCH);

    const waiting = screen.getByText("Dad accepted. Waiting for a turn to send…").closest("li")!;
    fireEvent.click(within(waiting).getByRole("button", { name: "Cancel photo.jpg" }));
    expect(device.api.cancelTransfer).toHaveBeenCalledWith("22".repeat(16));
  });

  it("offers Cancel all only while something in the Batch can still be cancelled", async () => {
    const device = await start(fakeApi({}, contacts));
    await screen.findByRole("button", { name: "Send to Mum" });
    await member(device, 1, MUM, { kind: "completed", saved_to: null });
    await member(device, 2, DAD, { kind: "declined" });
    expect(screen.queryByRole("button", { name: /Cancel all/ })).toBeNull();
  });

  it("retries a Failed Device with a new Offer, and not a Declined one", async () => {
    const device = await start(fakeApi({}, contacts));
    await screen.findByRole("button", { name: "Send to Mum" });
    await member(device, 1, MUM, { kind: "failed", reason: "The other Device went away." });
    await member(device, 2, DAD, { kind: "declined" });
    fireEvent.click(screen.getByRole("button", { name: /Show each Device/ }));

    expect(screen.getAllByRole("button", { name: /^Retry / })).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "Retry photo.jpg to Mum" }));
    expect(device.api.retryTransfer).toHaveBeenCalledWith("11".repeat(16));

    // The new Offer replaces the Failed row; the Failed Transfer is no longer shown.
    await member(device, 3, MUM, { kind: "offered" });
    expect(screen.queryByText(/The other Device went away/)).toBeNull();
    expect(screen.getByText("Waiting for Mum…")).toBeTruthy();
    expect(screen.getByText("0 of 2 delivered, 1 declined, 1 in progress")).toBeTruthy();
  });

  it("says why a retry did not go out", async () => {
    const retryTransfer = vi.fn(() => Promise.reject(new Error("it has been retried already")));
    const device = await start(fakeApi({ retryTransfer }, contacts));
    await screen.findByRole("button", { name: "Send to Mum" });
    await member(device, 1, MUM, { kind: "failed", reason: "x" });
    fireEvent.click(screen.getByRole("button", { name: /Show each Device/ }));
    fireEvent.click(screen.getByRole("button", { name: "Retry photo.jpg to Mum" }));
    expect((await screen.findByRole("alert")).textContent).toContain("it has been retried already");
  });
});

describe("Text", () => {
  /** Text that would do harm if it were ever taken for markup. */
  const HOSTILE = '<img src=x onerror="window.pwned = 1"><script>window.pwned = 2</script> <b>bold</b> &amp;';
  const MUM = PEER_ID;
  const DAD = "Q2WERTYU" + "C".repeat(44);
  const contacts = [
    contact({ id: MUM, nickname: "Mum", added_at: 1 }),
    contact({ id: DAD, nickname: "Dad", added_at: 2 }),
  ];
  const write = (text: string) =>
    fireEvent.change(screen.getByLabelText("Text"), { target: { value: text } });

  it("offers a text composer next to the file picker and sends exactly what was typed", async () => {
    const device = await start();
    fireEvent.click(screen.getByRole("button", { name: "Send to ID…" }));
    fireEvent.change(screen.getByLabelText("Device ID"), { target: { value: ` ${PEER_ID} ` } });
    expect(screen.getByRole("button", { name: "Choose files…" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Write text…" }));

    // Nothing to send until something is written.
    const send = screen.getByRole("button", { name: "Send text" }) as HTMLButtonElement;
    expect(send.disabled).toBe(true);
    write("  \n ");
    expect(send.disabled).toBe(true);
    write("  hello\nworld  ");
    fireEvent.click(send);

    await waitFor(() => expect(device.api.sendText).toHaveBeenCalled());
    // The ID is trimmed, the text is not.
    expect(device.api.sendText).toHaveBeenCalledWith(PEER_ID, "  hello\nworld  ");
    expect(device.api.sendFiles).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("needs a Device ID before text can be written, and can go back to the pickers", async () => {
    await start();
    fireEvent.click(screen.getByRole("button", { name: "Send to ID…" }));
    expect((screen.getByRole("button", { name: "Write text…" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(screen.getByLabelText("Device ID"), { target: { value: PEER_ID } });
    fireEvent.click(screen.getByRole("button", { name: "Write text…" }));
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    expect(screen.getByRole("button", { name: "Choose files…" })).toBeTruthy();
  });

  it("says why the text could not be sent and keeps it", async () => {
    const sendText = vi.fn(() => Promise.reject(new Error("There is no text to send.")));
    await start(fakeApi({ sendText }));
    fireEvent.click(screen.getByRole("button", { name: "Send to ID…" }));
    fireEvent.change(screen.getByLabelText("Device ID"), { target: { value: PEER_ID } });
    fireEvent.click(screen.getByRole("button", { name: "Write text…" }));
    write("keep me");
    fireEvent.click(screen.getByRole("button", { name: "Send text" }));
    expect((await screen.findByRole("alert")).textContent).toContain("Could not send. There is no text to send.");
    expect((screen.getByLabelText("Text") as HTMLTextAreaElement).value).toBe("keep me");
  });

  it("sends text to one selected Device as an ordinary Transfer, and to several as a Batch", async () => {
    const device = await start(fakeApi({}, contacts));
    await screen.findByRole("button", { name: "Send to Mum" });
    const select = (name: string) =>
      fireEvent.click(screen.getByRole("checkbox", { name: `Select ${name} to send to several Devices at once` }));

    select("Mum");
    fireEvent.click(screen.getByRole("button", { name: "Write text…" }));
    write("just Mum");
    fireEvent.click(screen.getByRole("button", { name: "Send text" }));
    await waitFor(() => expect(device.api.sendText).toHaveBeenCalledWith(MUM, "just Mum"));
    expect(device.api.sendTextBatch).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.queryByText("1 Device selected")).toBeNull());

    select("Mum");
    select("Dad");
    fireEvent.click(screen.getByRole("button", { name: "Write text…" }));
    write("everyone");
    fireEvent.click(screen.getByRole("button", { name: "Send text" }));
    await waitFor(() => expect(device.api.sendTextBatch).toHaveBeenCalledWith([MUM, DAD], "everyone"));
    expect(device.api.sendText).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(screen.queryByText("2 Devices selected")).toBeNull());
  });

  it("shows an incoming text in the Offer sheet as plain text, with nothing to save and nothing to check", async () => {
    const device = await start();
    await device.transfer("receiver", { kind: "offered" }, "Laptop", {
      kind: "text",
      text: HOSTILE,
      name: "",
      items: [],
      file_count: 0,
    });

    const sheet = await screen.findByRole("dialog", { name: "Incoming text" });
    // Every character of it is shown as text: none of it became an element.
    expect(within(sheet).getByRole("region", { name: "Text" }).textContent).toBe(HOSTILE);
    expect(sheet.querySelector("img, script, b")).toBeNull();
    expect((window as unknown as { pwned?: number }).pwned).toBeUndefined();
    // It is no file: no folder, no space check, no items.
    expect(sheet.textContent).not.toContain("Will be saved to");
    expect(sheet.textContent).not.toContain("Items");
    expect(device.api.checkOffer).not.toHaveBeenCalled();
    expect((within(sheet).getByRole("button", { name: "Accept" }) as HTMLButtonElement).disabled).toBe(false);

    fireEvent.click(within(sheet).getByRole("button", { name: "Accept" }));
    expect(device.api.acceptOffer).toHaveBeenCalledWith(TRANSFER, null);
  });

  it("can decline an incoming text", async () => {
    const device = await start();
    await device.transfer("receiver", { kind: "offered" }, null, { kind: "text", text: "hi", name: "", items: [] });
    fireEvent.click(await screen.findByRole("button", { name: "Decline" }));
    expect(device.api.declineOffer).toHaveBeenCalledWith(TRANSFER);
  });

  it("shows the received text in the app as plain text, with a Copy button that copies it exactly", async () => {
    const device = await start();
    const incoming = { kind: "text" as const, text: HOSTILE, name: "", items: [], file_count: 0, size: 99 };
    await device.transfer("receiver", { kind: "offered" }, "Laptop", incoming);
    await device.transfer("receiver", { kind: "accepted" }, "Laptop", incoming);
    await device.transfer("receiver", { kind: "completed", saved_to: null }, "Laptop", incoming);

    expect(screen.queryByRole("dialog")).toBeNull();
    const row = screen.getByText("Text from Laptop · K3QF-7XNA").closest("li")!;
    expect(within(row).getByRole("region", { name: "Text" }).textContent).toBe(HOSTILE);
    expect(row.querySelector("img, script, b")).toBeNull();
    // A text has no folder to show.
    expect(within(row).queryByRole("button", { name: /Show/ })).toBeNull();

    fireEvent.click(within(row).getByRole("button", { name: "Copy the text from Laptop · K3QF-7XNA" }));
    await waitFor(() => expect(device.api.copyText).toHaveBeenCalledWith(HOSTILE));
    expect(await within(row).findByText("Copied")).toBeTruthy();
  });

  it("says when the text could not be copied", async () => {
    const copyText = vi.fn(() => Promise.reject(new Error("no clipboard")));
    const device = await start(fakeApi({ copyText }));
    await device.transfer("receiver", { kind: "completed", saved_to: null }, "Laptop", {
      kind: "text",
      text: "hi",
      name: "",
      items: [],
    });
    fireEvent.click(screen.getByRole("button", { name: /^Copy the text/ }));
    expect(await screen.findByText(/Could not copy/)).toBeTruthy();
  });

  it("names a text Transfer 'Text' on the Sender's row, without showing what it says", async () => {
    const device = await start();
    await device.transfer("sender", { kind: "completed", saved_to: null }, "Laptop", {
      kind: "text",
      text: "a secret",
      name: "",
      items: [],
    });
    expect(screen.getByText("Text to Laptop · K3QF-7XNA")).toBeTruthy();
    expect(screen.getByText("Sent.")).toBeTruthy();
    expect(screen.queryByText("a secret")).toBeNull();
    expect(screen.queryByRole("button", { name: /^Copy the text/ })).toBeNull();
  });

  it("does not show a file's name for a long text sent as text.txt: it is an ordinary file", async () => {
    const device = await start();
    await device.transfer("receiver", { kind: "offered" }, null, { name: "text.txt", items: ["text.txt"] });
    const sheet = await screen.findByRole("dialog", { name: "Incoming files" });
    expect(sheet.textContent).toContain("text.txt");
    expect(device.api.checkOffer).toHaveBeenCalled();
  });
});
