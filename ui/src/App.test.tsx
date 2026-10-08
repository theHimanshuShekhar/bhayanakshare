import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import type { Api, DiscoveryStatus, UpdateAction } from "./api";
import type { HistoryEntry, TransferRecord, TransferState } from "./bindings";
import { NEARBY_WAIT_MS } from "./nearby";
import { BATCH, EXPIRES_AT, MY_ID, PEER_ID, TRANSFER, contact, fakeApi } from "./testApi";

// The camera cannot be tested here (QrScanner.test.tsx covers how it is read): this stand-in
// "reads" whichever text a test chooses.
const camera = vi.hoisted(() => ({ sees: "" }));
vi.mock("./QrScanner", () => ({
  QrScanner: ({ onDecoded }: { onDecoded: (text: string) => void }) => (
    <button type="button" onClick={() => onDecoded(camera.sees)}>
      camera sees a code
    </button>
  ),
}));

afterEach(cleanup);

/** Renders the app and waits until it is listening for events. */
async function start(device = fakeApi()) {
  render(<App api={device.api} />);
  await screen.findByText("AAAA-AAAA");
  return device;
}

/** The live regions that say something now: some (the one of My ID, until something is copied)
 * are there empty, to be heard when they change. */
const saying = () => screen.queryAllByRole("status").filter((el) => el.textContent !== "");
const findStatus = () =>
  waitFor(() => {
    const [first] = saying();
    expect(first).toBeTruthy();
    return first;
  });

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
    fireEvent.click(screen.getByRole("button", { name: "Copy ID" }));
    expect(await screen.findByText("Copied")).toBeTruthy();
    expect(device.api.copyText).toHaveBeenCalledWith(MY_ID);
  });

  it("shows the share link with this Device's name, and its QR code", async () => {
    await start();
    const link = `bhayanakshare://add/${MY_ID}?name=Alice's%20desktop`;
    expect(screen.getByText(link)).toBeTruthy();
    expect(screen.getByRole("img", { name: "QR code of the share link" })).toBeTruthy();
  });

  it("copies the share link and says so", async () => {
    const device = await start();
    fireEvent.click(screen.getByRole("button", { name: "Copy link" }));
    expect(await screen.findByText("Copied")).toBeTruthy();
    expect(device.api.copyText).toHaveBeenCalledWith(`bhayanakshare://add/${MY_ID}?name=Alice's%20desktop`);
  });

  it("shares a link without a name when the name cannot be read", async () => {
    await start(fakeApi({ deviceName: () => Promise.reject(new Error("no name")) }));
    expect(screen.getByText(`bhayanakshare://add/${MY_ID}`)).toBeTruthy();
  });

  it("says when the ID could not be copied", async () => {
    await start(fakeApi({ copyText: () => Promise.reject(new Error("no clipboard")) }));
    fireEvent.click(screen.getByRole("button", { name: "Copy ID" }));
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
    expect(await screen.findByText("No Transfers in your History yet.")).toBeTruthy();
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

  it("shows the folder the check says the Offer would be saved to, not one read earlier", async () => {
    const device = await start(
      fakeApi({
        checkOffer: vi.fn(() =>
          Promise.resolve({ folder: "/mnt/drive/received", needed: 2048, free: 1_000_000, paths_too_long: false }),
        ),
      }),
    );
    await device.transfer("receiver", { kind: "offered" });
    const sheet = await screen.findByRole("dialog", { name: "Incoming files" });
    await waitFor(() => expect(sheet.textContent).toContain("/mnt/drive/received"));
    expect(sheet.textContent).not.toContain("/home/me/Downloads/BhayanakShare");
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
      fakeApi({ checkOffer: vi.fn(() => Promise.resolve({ folder: "/home/me/Downloads/BhayanakShare", needed: 2048, free: 512, paths_too_long: false })) }),
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
      Promise.resolve({ folder: folder ?? "/home/me/Downloads/BhayanakShare", needed: 2048, free: 1_000_000, paths_too_long: folder === null }),
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
      fakeApi({ checkOffer: vi.fn(() => Promise.resolve({ folder: "/home/me/Downloads/BhayanakShare", needed: 2048, free: null, paths_too_long: false })) }),
    );
    await device.transfer("receiver", { kind: "offered" });
    await waitFor(() => expect(device.api.checkOffer).toHaveBeenCalled());
    expect(screen.queryByRole("alert")).toBeNull();
    expect((screen.getByRole("button", { name: "Accept" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("checks again in the folder chosen for this Offer, and accepts into it", async () => {
    const checkOffer = vi.fn((_id: string, folder: string | null) =>
      Promise.resolve({ folder: folder ?? "/home/me/Downloads/BhayanakShare", needed: 2048, free: folder === null ? 512 : 1_000_000, paths_too_long: false }),
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

    fireEvent.click(screen.getByRole("button", { name: "Show in folder: photo.jpg" }));
    expect(device.api.showInFolder).toHaveBeenCalledWith("/home/me/Downloads/BhayanakShare/photo.jpg");
  });

  it("says so when no file manager can show the folder, and clears it on the next try", async () => {
    const showInFolder = vi
      .fn<Api["showInFolder"]>()
      .mockRejectedValueOnce("ServiceUnknown")
      .mockResolvedValueOnce();
    const device = await start(fakeApi({ showInFolder }));
    await device.transfer("receiver", { kind: "completed", saved_to: "/home/me/Downloads/photo.jpg" });

    const button = screen.getByRole("button", { name: "Show in folder: photo.jpg" });
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

    fireEvent.click(screen.getByRole("button", { name: "Send again: photo.jpg" }));
    expect(device.api.resendTransfer).toHaveBeenCalledWith(TRANSFER);
    // Once sent again, the expired row no longer offers it.
    await waitFor(() => expect(screen.queryByRole("button", { name: "Send again: photo.jpg" })).toBeNull());
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
    fireEvent.click(screen.getByRole("button", { name: "Send again: photo.jpg" }));
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
    const list = screen.getByRole("region", { name: "Transfers" });
    expect(within(list).getByText(/Could not send\. The other Device already has too many Offers/)).toBeTruthy();
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
    expect(screen.getAllByRole("button", { name: /^Save as Contact: / })).toHaveLength(1);
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
    fireEvent.click(await screen.findByRole("button", { name: /^Save as Contact: Dad's PC · K3QF-7XNA$/ }));

    // Straight to the check: the ID was not typed, but the Fingerprint is still compared.
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText("K3QF-7XNA")).toBeTruthy();
    expect(device.api.addContact).not.toHaveBeenCalled();

    fireEvent.click(within(dialog).getByRole("button", { name: "It matches, add Contact" }));
    await waitFor(() => expect(device.api.addContact).toHaveBeenCalledWith(PEER_ID, "Dad's PC"));
    // Now a Contact: its tile is badged and the Save button is gone.
    expect(await screen.findByRole("button", { name: "Send to Dad's PC" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /^Save as Contact: / })).toBeNull();
  });

  it("saves nothing when the Fingerprint check is cancelled", async () => {
    const device = await start();
    await device.nearby({ id: PEER_ID, name: null });
    fireEvent.click(await screen.findByRole("button", { name: /^Save as Contact: K3QF-7XNA$/ }));
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(device.api.addContact).not.toHaveBeenCalled();
  });
});

describe("share links, QR codes and deep links", () => {
  const link = (id = PEER_ID, name = "Dad's PC") => `bhayanakshare://add/${id}?name=${encodeURIComponent(name)}`;
  const nameField = () => screen.getByLabelText("Name (optional)") as HTMLInputElement;
  const idField = () => screen.getByLabelText("Device ID") as HTMLInputElement;

  it("fills in the Device ID and the name from a pasted link, to be checked as usual", async () => {
    const device = await start();
    fireEvent.click(screen.getByRole("button", { name: "Contacts" }));
    fireEvent.click(screen.getByRole("button", { name: "Add Contact…" }));
    fireEvent.change(idField(), { target: { value: ` ${link()}\n` } });
    expect(idField().value).toBe(PEER_ID);
    expect(nameField().value).toBe("Dad's PC");

    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(within(screen.getByRole("dialog")).getByText("K3QF-7XNA")).toBeTruthy();
    expect(device.api.addContact).not.toHaveBeenCalled();
  });

  it("keeps a name the user typed when a link is pasted after it", async () => {
    const device = await start();
    fireEvent.click(screen.getByRole("button", { name: "Contacts" }));
    fireEvent.click(screen.getByRole("button", { name: "Add Contact…" }));
    fireEvent.change(nameField(), { target: { value: "Dad" } });
    fireEvent.change(idField(), { target: { value: link() } });
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    fireEvent.click(screen.getByRole("button", { name: "It matches, add Contact" }));
    await waitFor(() => expect(device.api.addContact).toHaveBeenCalledWith(PEER_ID, "Dad"));
  });

  it("opens Add Contact with the fields filled in when a link is opened, and the name can be changed", async () => {
    const device = await start();
    await device.openLink(link());

    const dialog = screen.getByRole("dialog", { name: "Add Contact" });
    expect(idField().value).toBe(PEER_ID);
    expect(nameField().value).toBe("Dad's PC");

    // The suggested name is only a suggestion: change it, and the Fingerprint still has to be checked.
    fireEvent.change(nameField(), { target: { value: "Dad" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Next" }));
    expect(device.api.addContact).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "It matches, add Contact" }));
    await waitFor(() => expect(device.api.addContact).toHaveBeenCalledWith(PEER_ID, "Dad"));
  });

  it("opens the dialog in place of an Add Contact dialog that is already open", async () => {
    const device = await start();
    fireEvent.click(screen.getByRole("button", { name: "Contacts" }));
    fireEvent.click(screen.getByRole("button", { name: "Add Contact…" }));
    fireEvent.change(idField(), { target: { value: "K3QF" } });
    await device.openLink(link());
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
    expect(idField().value).toBe(PEER_ID);
  });

  it("says so, with an empty ID field, when the link that was opened is not a share link", async () => {
    const device = await start();
    await device.openLink(`bhayanakshare://add/${PEER_ID.slice(1)}?name=Eve`);
    expect(screen.getByRole("alert").textContent).toContain("not a BhayanakShare share link");
    expect(idField().value).toBe("");
    expect(nameField().value).toBe("");
    expect((screen.getByRole("button", { name: "Next" }) as HTMLButtonElement).disabled).toBe(true);

    // The message goes once the user starts on the ID themselves.
    fireEvent.change(idField(), { target: { value: "K3QF" } });
    expect(screen.getByRole("alert").textContent).not.toContain("not a BhayanakShare share link");
    fireEvent.change(idField(), { target: { value: PEER_ID } });
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("keeps a link that arrives over an Offer until the Offer is answered", async () => {
    const device = await start();
    await device.transfer("receiver", { kind: "offered" });
    await screen.findByRole("dialog", { name: "Incoming files" });
    await device.openLink(link());
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
    expect(screen.queryByRole("dialog", { name: "Add Contact" })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Decline" }));
    await device.transfer("receiver", { kind: "declined" });
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
    expect(screen.getByRole("dialog", { name: "Add Contact" })).toBeTruthy();
    expect(idField().value).toBe(PEER_ID);
    expect(nameField().value).toBe("Dad's PC");
  });

  it("keeps a link that arrives over Send to ID until that is closed, and does not stack on it", async () => {
    const device = await start();
    fireEvent.click(screen.getByRole("button", { name: "Send to ID…" }));
    await device.openLink(link());
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
    expect(screen.getByRole("dialog", { name: "Send to ID" })).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
    expect(idField().value).toBe(PEER_ID);
  });

  it("shows only the newest of several links that arrive while another dialog is open", async () => {
    const device = await start();
    fireEvent.click(screen.getByRole("button", { name: "Send to ID…" }));
    await device.openLink(link(PEER_ID, "Old"));
    await device.openLink(link("C".repeat(52), "New"));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(idField().value).toBe("C".repeat(52));
    expect(nameField().value).toBe("New");
  });

  it("replaces the suggested name when a link for another Device comes after it", async () => {
    await start();
    fireEvent.click(screen.getByRole("button", { name: "Contacts" }));
    fireEvent.click(screen.getByRole("button", { name: "Add Contact…" }));
    fireEvent.change(idField(), { target: { value: link(PEER_ID, "Dad's PC") } });
    expect(nameField().value).toBe("Dad's PC");

    fireEvent.change(idField(), { target: { value: link("C".repeat(52), "Mum's laptop") } });
    expect(idField().value).toBe("C".repeat(52));
    expect(nameField().value).toBe("Mum's laptop");

    // A link with no name must not leave the other Device's name on this one.
    fireEvent.change(idField(), { target: { value: `bhayanakshare://add/${PEER_ID}` } });
    expect(idField().value).toBe(PEER_ID);
    expect(nameField().value).toBe("");
  });

  it("replaces the suggested name from an opened link when a scan or paste names another Device", async () => {
    const device = await start();
    await device.openLink(link(PEER_ID, "Dad's PC"));
    fireEvent.change(idField(), { target: { value: link("C".repeat(52), "Mum's laptop") } });
    expect(nameField().value).toBe("Mum's laptop");
  });

  it("never replaces a name the user changed, whichever link comes next", async () => {
    const device = await start();
    await device.openLink(link(PEER_ID, "Dad's PC"));
    fireEvent.change(nameField(), { target: { value: "Dad" } });
    fireEvent.change(idField(), { target: { value: link("C".repeat(52), "Mum's laptop") } });
    expect(idField().value).toBe("C".repeat(52));
    expect(nameField().value).toBe("Dad");
  });

  it("does not add anyone just because a link was opened", async () => {
    const device = await start();
    await device.openLink(link());
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(device.api.addContact).not.toHaveBeenCalled();
  });

  it("takes a pasted link anywhere a Device ID is accepted", async () => {
    const device = await start();
    fireEvent.click(screen.getByRole("button", { name: "Send to ID…" }));
    fireEvent.change(idField(), { target: { value: link() } });
    expect(idField().value).toBe(PEER_ID);
    fireEvent.click(screen.getByRole("button", { name: "Choose files…" }));
    await waitFor(() => expect(device.api.sendFiles).toHaveBeenCalledWith(PEER_ID, ["/tmp/photo.jpg"]));
  });

  describe("scanning a QR code", () => {
    const scan = async (sees: string) => {
      camera.sees = sees;
      fireEvent.click(screen.getByRole("button", { name: "Contacts" }));
      fireEvent.click(screen.getByRole("button", { name: "Add Contact…" }));
      fireEvent.click(screen.getByRole("button", { name: "Scan QR code…" }));
      fireEvent.click(screen.getByRole("button", { name: "camera sees a code" }));
    };

    it("fills in the fields from a share link, and stops looking", async () => {
      await start();
      await scan(link());
      expect(idField().value).toBe(PEER_ID);
      expect(nameField().value).toBe("Dad's PC");
      expect(screen.queryByRole("button", { name: "camera sees a code" })).toBeNull();
    });

    it("takes a bare Device ID too", async () => {
      await start();
      await scan(PEER_ID);
      expect(idField().value).toBe(PEER_ID);
      expect(nameField().value).toBe("");
    });

    it("says so, and keeps looking, when the code is something else", async () => {
      await start();
      await scan("https://example.com/");
      expect(screen.getByRole("alert").textContent).toContain("not a BhayanakShare share link");
      expect(idField().value).toBe("");
      expect(screen.getByRole("button", { name: "camera sees a code" })).toBeTruthy();
    });

    it("can be stopped, and goes off when the user moves on", async () => {
      await start();
      await scan("nonsense");
      fireEvent.click(screen.getByRole("button", { name: "Stop scanning" }));
      expect(screen.queryByRole("button", { name: "camera sees a code" })).toBeNull();

      fireEvent.click(screen.getByRole("button", { name: "Scan QR code…" }));
      fireEvent.change(idField(), { target: { value: PEER_ID } });
      fireEvent.click(screen.getByRole("button", { name: "Next" }));
      expect(screen.queryByRole("button", { name: "camera sees a code" })).toBeNull();
      fireEvent.click(screen.getByRole("button", { name: "Back" }));
      expect(screen.queryByRole("button", { name: "camera sees a code" })).toBeNull();
    });
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

describe("the hint that local discovery could not start", () => {
  const FIREWALL = /No Devices found on this network yet/;
  const HIDDEN = "You're Hidden, so Nearby Devices aren't shown.";
  const DOCS = "https://github.com/theHimanshuShekhar/bhayanakshare/blob/main/docs/firewall.md";
  const DOWN = /Local discovery could not start, so Nearby Devices can't be found\./;
  const DOWN_HIDDEN = /Local discovery could not start, so people who have your ID can't find this Device/;
  const PORT = /UDP 5353/;
  const PORT_IN_USE: DiscoveryStatus = { state: "unavailable", reason: "port_in_use" };
  const WORKING: DiscoveryStatus = { state: "working" };

  /** Lets the 30-second wait run out at once, as the firewall hint's tests do. */
  function skipTheWait() {
    const real = globalThis.setTimeout;
    vi.spyOn(globalThis, "setTimeout").mockImplementation(((
      fn: () => void,
      ms?: number,
      ...args: unknown[]
    ) => real(fn, ms === NEARBY_WAIT_MS ? 0 : ms, ...args)) as unknown as typeof setTimeout);
  }

  afterEach(() => vi.restoreAllMocks());

  it("names the problem and the reason at once, and links to the firewall docs", async () => {
    const device = await start();
    expect(screen.queryByText(DOWN)).toBeNull();
    await device.discoveryChanges(PORT_IN_USE);
    // No 30-second wait: it is known, not guessed.
    expect(screen.getByText(DOWN, { exact: false })).toBeTruthy();
    expect(screen.getByText(PORT, { exact: false })).toBeTruthy();

    const link = screen.getByRole("link", { name: "How to allow local discovery" });
    expect(link.getAttribute("href")).toBe(DOCS);
    fireEvent.click(link);
    expect(device.api.openUrl).toHaveBeenCalledWith(DOCS);
  });

  it("gives each reason its own words", async () => {
    const device = await start();
    await device.discoveryChanges({ state: "unavailable", reason: "no_interface" });
    expect(screen.getByText(/No network connection that can use it was found/)).toBeTruthy();
    await device.discoveryChanges({ state: "unavailable", reason: "other" });
    expect(screen.getByText(/The system reported an error/)).toBeTruthy();
    expect(screen.queryByText(/No network connection that can use it was found/)).toBeNull();
  });

  it("goes away when discovery recovers", async () => {
    const device = await start();
    await device.discoveryChanges(PORT_IN_USE);
    expect(screen.getByText(DOWN, { exact: false })).toBeTruthy();
    await device.discoveryChanges(WORKING);
    expect(screen.queryByText(DOWN, { exact: false })).toBeNull();
    expect(screen.queryByText(PORT, { exact: false })).toBeNull();
  });

  it("is there after a reload, from what the Device says, without an event", async () => {
    const device = fakeApi();
    device.setDiscovery(PORT_IN_USE);
    await start(device);
    expect(await screen.findByText(DOWN, { exact: false })).toBeTruthy();
  });

  it("is not undone by a status read that comes after a newer event", async () => {
    let answer: (status: DiscoveryStatus) => void = () => {};
    const device = fakeApi({ discoveryStatus: () => new Promise((resolve) => (answer = resolve)) });
    await start(device);
    await device.discoveryChanges(PORT_IN_USE);
    await act(async () => answer(WORKING));
    expect(screen.getByText(DOWN, { exact: false })).toBeTruthy();
  });

  it("replaces the firewall hint, which says nothing of why", async () => {
    skipTheWait();
    const device = await start();
    expect(await screen.findByText(FIREWALL)).toBeTruthy();
    await device.discoveryChanges(PORT_IN_USE);
    expect(screen.queryByText(FIREWALL)).toBeNull();
    expect(screen.getAllByRole("link", { name: "How to allow local discovery" })).toHaveLength(1);
    // And gives it back, once the wait is over, if the status recovers with nobody Nearby.
    await device.discoveryChanges(WORKING);
    expect(await screen.findByText(FIREWALL)).toBeTruthy();
  });

  it("is announced the way the firewall hint is: as one status, not a second live region", async () => {
    skipTheWait();
    const device = await start();
    await screen.findByText(FIREWALL);
    const before = screen.getAllByRole("status").filter((el) => el.textContent !== "").length;
    await device.discoveryChanges(PORT_IN_USE);
    const hint = screen.getByText(DOWN, { exact: false });
    expect(hint.closest("[role=status]")).toBe(hint);
    expect(screen.getAllByRole("status").filter((el) => el.textContent !== "").length).toBe(before);
  });

  it("is not shown while a Device is Nearby, as the other hints are not", async () => {
    const device = await start();
    await device.nearby({ id: PEER_ID, name: "Dad's PC" });
    await device.discoveryChanges(PORT_IN_USE);
    expect(screen.queryByText(DOWN, { exact: false })).toBeNull();
  });

  it("shows the Hidden hint for a Hidden Device whose discovery is fine, and this one when its responder is down", async () => {
    const device = fakeApi();
    await device.api.setVisibility("hidden");
    await start(device);
    expect(await screen.findByText(HIDDEN)).toBeTruthy();

    // Hidden lists nobody, but ID holders cannot reach a Device that cannot answer them.
    await device.discoveryChanges(PORT_IN_USE);
    expect(screen.getByText(DOWN_HIDDEN, { exact: false })).toBeTruthy();
    expect(screen.queryByText(DOWN)).toBeNull();
    expect(screen.queryByText(HIDDEN)).toBeNull();
    expect(screen.getByRole("link", { name: "How to allow local discovery" })).toBeTruthy();

    await device.discoveryChanges(WORKING);
    expect(screen.getByText(HIDDEN)).toBeTruthy();
    expect(screen.queryByText(DOWN_HIDDEN, { exact: false })).toBeNull();
  });
});

describe("the Hidden hint", () => {
  const FIREWALL = /No Devices found on this network yet/;
  const HIDDEN = "You're Hidden, so Nearby Devices aren't shown.";

  /** Lets the 30-second wait run out at once, as the firewall hint's tests do. */
  function skipTheWait() {
    const real = globalThis.setTimeout;
    vi.spyOn(globalThis, "setTimeout").mockImplementation(((
      fn: () => void,
      ms?: number,
      ...args: unknown[]
    ) => real(fn, ms === NEARBY_WAIT_MS ? 0 : ms, ...args)) as unknown as typeof setTimeout);
  }

  afterEach(() => vi.restoreAllMocks());

  it("replaces the firewall hint on a Hidden Device, which never lists anyone", async () => {
    skipTheWait();
    const device = fakeApi();
    await device.api.setVisibility("hidden");
    await start(device);
    expect(await screen.findByText(HIDDEN)).toBeTruthy();
    await new Promise((r) => setTimeout(r, 20));
    expect(screen.queryByText(FIREWALL)).toBeNull();
  });

  it("is not shown at the other settings, which keep the firewall hint", async () => {
    skipTheWait();
    await start();
    expect(await screen.findByText(FIREWALL)).toBeTruthy();
    expect(screen.queryByText(HIDDEN)).toBeNull();
  });

  it("follows the setting when it is changed, and leads to it", async () => {
    skipTheWait();
    const device = await start();
    expect(await screen.findByText(FIREWALL)).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    fireEvent.click(await screen.findByRole("radio", { name: "Hidden" }));
    await waitFor(() => expect(device.api.setVisibility).toHaveBeenCalledWith("hidden"));
    fireEvent.click(screen.getByRole("button", { name: "Home" }));
    expect(await screen.findByText(HIDDEN)).toBeTruthy();
    expect(screen.queryByText(FIREWALL)).toBeNull();

    // The button goes to the setting.
    fireEvent.click(screen.getByRole("button", { name: "Change Visibility" }));
    fireEvent.click(await screen.findByRole("radio", { name: "Everyone" }));
    await waitFor(() => expect(device.api.setVisibility).toHaveBeenCalledWith("everyone"));
    fireEvent.click(screen.getByRole("button", { name: "Home" }));
    expect(await screen.findByText(FIREWALL)).toBeTruthy();
    expect(screen.queryByText(HIDDEN)).toBeNull();
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
      "You won't see or be seen Nearby. People with your ID can still send to you.",
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

describe("first run", () => {
  const needed = (overrides: Partial<Api> = {}) =>
    fakeApi({ needsFirstRun: vi.fn(() => Promise.resolve(true)), ...overrides });
  const begin = async (device = needed()) => {
    render(<App api={device.api} />);
    await screen.findByRole("heading", { name: "Welcome to BhayanakShare" });
    return device;
  };
  const nameField = () => screen.getByRole("textbox", { name: "Device Name" }) as HTMLInputElement;
  const getStarted = () => screen.getByRole("button", { name: "Get started" });
  const radio = (name: string) => screen.getByRole("radio", { name }) as HTMLInputElement;
  const loginBox = () => screen.getByRole("checkbox", { name: "Start at login" }) as HTMLInputElement;

  it("is one screen instead of the tabs, with a heading, labelled controls and a note on privacy", async () => {
    await begin();
    expect(screen.queryByRole("navigation")).toBeNull();
    expect(screen.queryByRole("button", { name: "Settings" })).toBeNull();
    expect(screen.queryByText("AAAA-AAAA")).toBeNull();
    expect(screen.getByRole("heading", { name: "Welcome to BhayanakShare", level: 2 })).toBeTruthy();
    const region = screen.getByRole("region", { name: "Welcome to BhayanakShare" });
    expect(within(region).getByRole("group", { name: "Who can see this Device nearby" })).toBeTruthy();
    expect(within(region).getByRole("button", { name: "Change the folder received files are saved to" })).toBeTruthy();
    expect(region.textContent).toContain(
      "Your Device Name is shown only to the people Visibility allows, and is never published online.",
    );
  });

  it("starts with the Device's name, Visibility, start at login and save folder, and the name has focus", async () => {
    await begin();
    expect(nameField().value).toBe("Alice's desktop");
    expect(document.activeElement).toBe(nameField());
    expect(radio("People who have my ID").checked).toBe(true);
    expect(radio("Everyone").checked).toBe(false);
    expect(radio("Hidden").checked).toBe(false);
    expect(loginBox().checked).toBe(true);
    expect(screen.getByText("/home/me/Downloads/BhayanakShare")).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("keeps all of that at Get started, applies it, marks first run done and shows Home", async () => {
    const device = await begin();
    fireEvent.click(getStarted());

    await screen.findByText("AAAA-AAAA");
    expect(device.api.setDeviceName).toHaveBeenCalledExactlyOnceWith("Alice's desktop");
    expect(device.api.setVisibility).toHaveBeenCalledExactlyOnceWith("id_holders");
    // Start at login is on, and said so to the shell; the folder is the default, so it is not kept.
    expect(device.api.setAutostart).toHaveBeenCalledExactlyOnceWith(true);
    expect(device.api.setSaveFolder).not.toHaveBeenCalled();
    expect(device.api.finishFirstRun).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("navigation")).toBeTruthy();
    expect(screen.queryByRole("heading", { name: "Welcome to BhayanakShare" })).toBeNull();
    expect(screen.getByRole("heading", { name: "Devices" })).toBeTruthy();
  });

  it("keeps what the user changed: the name, Visibility, start at login and the folder", async () => {
    const device = await begin();
    fireEvent.change(nameField(), { target: { value: "  Bob's PC " } });
    fireEvent.click(radio("Hidden"));
    fireEvent.click(loginBox());
    fireEvent.click(screen.getByRole("button", { name: "Change the folder received files are saved to" }));
    expect(await screen.findByText("/mnt/big")).toBeTruthy();
    // Nothing is kept before Get started.
    expect(device.api.setDeviceName).not.toHaveBeenCalled();
    expect(device.api.setSaveFolder).not.toHaveBeenCalled();
    expect(device.api.finishFirstRun).not.toHaveBeenCalled();

    fireEvent.click(getStarted());
    await screen.findByText("AAAA-AAAA");
    expect(device.api.setDeviceName).toHaveBeenCalledWith("  Bob's PC ");
    expect(device.api.setVisibility).toHaveBeenCalledWith("hidden");
    expect(device.api.setAutostart).toHaveBeenCalledExactlyOnceWith(false);
    expect(device.api.setSaveFolder).toHaveBeenCalledExactlyOnceWith("/mnt/big");
    expect(device.api.finishFirstRun).toHaveBeenCalledTimes(1);
    // The Device has them now: Settings shows them as they were kept.
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    await waitFor(() => expect(radio("Hidden").checked).toBe(true));
    expect((await screen.findByRole("textbox", { name: "Device Name" }) as HTMLInputElement).value).toBe("Bob's PC");
    expect(await screen.findByText("/mnt/big")).toBeTruthy();
  });

  it("starts with start at login checked, whatever the Device says now, and switches it on at Get started", async () => {
    // The shell's own default may not have run yet, or may have failed: what the screen shows is
    // what is applied.
    const device = await begin(needed({ autostartEnabled: vi.fn(() => Promise.resolve(false)) }));
    expect(loginBox().checked).toBe(true);
    fireEvent.click(getStarted());
    await screen.findByText("AAAA-AAAA");
    expect(device.api.setAutostart).toHaveBeenCalledExactlyOnceWith(true);
  });

  it("finishes first run when start at login cannot be switched on, saying why once before going on", async () => {
    const device = await begin(
      needed({ setAutostart: vi.fn(() => Promise.reject("This system has no login items.")) }),
    );
    fireEvent.click(getStarted());

    const note = await screen.findByRole("status");
    expect(note.textContent).toBe(
      "Setup is done, but start at login could not be switched on: This system has no login items. You can try again in Settings.",
    );
    // Everything else was kept and first run is done; nothing is in the way of going on.
    expect(device.api.setDeviceName).toHaveBeenCalled();
    expect(device.api.setVisibility).toHaveBeenCalled();
    expect(device.api.finishFirstRun).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.queryByRole("button", { name: "Get started" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    expect(await screen.findByText("AAAA-AAAA")).toBeTruthy();
    expect(screen.getByRole("navigation")).toBeTruthy();
  });

  it("shows Hidden's hint on Home, not the firewall hint, when Hidden was chosen at first run", async () => {
    await begin();
    fireEvent.click(radio("Hidden"));
    fireEvent.click(getStarted());
    await screen.findByText("AAAA-AAAA");
    expect(await screen.findByText(/You're Hidden, so Nearby Devices aren't shown/)).toBeTruthy();
    expect(screen.queryByText(/a firewall may be blocking local discovery/)).toBeNull();
  });

  it("keeps the folder the Offer sheet shows from then on", async () => {
    const device = await begin();
    fireEvent.click(screen.getByRole("button", { name: "Change the folder received files are saved to" }));
    await screen.findByText("/mnt/big");
    fireEvent.click(getStarted());
    await screen.findByText("AAAA-AAAA");
    await device.transfer("receiver", { kind: "offered" });
    const sheet = await screen.findByRole("dialog", { name: "Incoming files" });
    await waitFor(() => expect(sheet.textContent).toContain("/mnt/big"));
  });

  it("leaves the folder as it was when the folder dialog is cancelled", async () => {
    const device = await begin(needed({ pickFolder: vi.fn(() => Promise.resolve<string | null>(null)) }));
    fireEvent.click(screen.getByRole("button", { name: "Change the folder received files are saved to" }));
    await waitFor(() => expect(device.api.pickFolder).toHaveBeenCalled());
    expect(screen.getByText("/home/me/Downloads/BhayanakShare")).toBeTruthy();
    fireEvent.click(getStarted());
    await screen.findByText("AAAA-AAAA");
    expect(device.api.setSaveFolder).not.toHaveBeenCalled();
  });

  it("says inline that a name is needed, and goes no further", async () => {
    const device = await begin();
    fireEvent.change(nameField(), { target: { value: "   " } });
    fireEvent.click(getStarted());

    expect((await screen.findByRole("alert")).textContent).toBe("Enter a name for this Device.");
    expect(nameField().getAttribute("aria-invalid")).toBe("true");
    expect(document.activeElement).toBe(nameField());
    expect(device.api.setDeviceName).not.toHaveBeenCalled();
    expect(device.api.finishFirstRun).not.toHaveBeenCalled();
    expect(screen.getByRole("heading", { name: "Welcome to BhayanakShare" })).toBeTruthy();

    // Fixed, it goes on.
    fireEvent.change(nameField(), { target: { value: "Bob's PC" } });
    fireEvent.click(getStarted());
    await screen.findByText("AAAA-AAAA");
    expect(device.api.setDeviceName).toHaveBeenCalledWith("Bob's PC");
  });

  it("limits the name to 64 characters, which is what the Device keeps", async () => {
    await begin();
    expect(nameField().maxLength).toBe(64);
  });

  it("shows the Device's reason when it refuses the name, and the other choices are not kept", async () => {
    const device = await begin(
      needed({ setDeviceName: vi.fn(() => Promise.reject(new Error("A Device Name cannot be empty."))) }),
    );
    fireEvent.change(nameField(), { target: { value: "\u0007" } });
    fireEvent.click(getStarted());
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Could not save the name. A Device Name cannot be empty.",
    );
    expect(device.api.setVisibility).not.toHaveBeenCalled();
    expect(device.api.finishFirstRun).not.toHaveBeenCalled();
    expect((getStarted() as HTMLButtonElement).disabled).toBe(false);
  });

  it("shows the reason inline when the folder cannot be used, and first run is not done", async () => {
    const device = await begin(
      needed({
        setSaveFolder: vi.fn(() => Promise.reject({ kind: "not_writable", message: "the folder is not writable" })),
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Change the folder received files are saved to" }));
    await screen.findByText("/mnt/big");
    fireEvent.click(getStarted());
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Could not use that folder. BhayanakShare cannot write to it.",
    );
    expect(device.api.finishFirstRun).not.toHaveBeenCalled();
    expect(screen.getByRole("heading", { name: "Welcome to BhayanakShare" })).toBeTruthy();
  });

  it("says why it could not finish, and can be tried again", async () => {
    const finishFirstRun = vi
      .fn<() => Promise<unknown>>()
      .mockRejectedValueOnce(new Error("disk full"))
      .mockResolvedValue(null);
    await begin(needed({ finishFirstRun }));
    fireEvent.click(getStarted());
    expect((await screen.findByRole("alert")).textContent).toBe("Could not finish setting up. disk full");
    fireEvent.click(getStarted());
    await screen.findByText("AAAA-AAAA");
    expect(finishFirstRun).toHaveBeenCalledTimes(2);
  });

  it("does not keep a Visibility it could only guess: unread and not chosen, it is left as it is", async () => {
    const device = await begin(needed({ visibility: () => Promise.reject(new Error("no")) }));
    expect((await screen.findByRole("alert")).textContent).toContain("Could not read some of this Device's settings");
    // Offered as the default, and not a reason to overwrite whatever the Device has.
    expect(radio("People who have my ID").checked).toBe(true);
    fireEvent.click(getStarted());
    await screen.findByText("AAAA-AAAA");
    expect(device.api.setVisibility).not.toHaveBeenCalled();
    expect(device.api.finishFirstRun).toHaveBeenCalledTimes(1);
  });

  it("keeps a Visibility the user chose even when the Device's could not be read", async () => {
    const device = await begin(needed({ visibility: () => Promise.reject(new Error("no")) }));
    fireEvent.click(radio("Everyone"));
    fireEvent.click(getStarted());
    await screen.findByText("AAAA-AAAA");
    expect(device.api.setVisibility).toHaveBeenCalledExactlyOnceWith("everyone");
  });

  it("offers the Visibility the Device has, such as Hidden from the tray", async () => {
    // The Device was set to Hidden before first run was shown.
    const device = needed();
    await device.api.setVisibility("hidden");
    vi.mocked(device.api.setVisibility).mockClear();
    await begin(device);
    expect(radio("Hidden").checked).toBe(true);
    expect(device.api.setVisibility).not.toHaveBeenCalled();
  });

  it("is not shown when first run is done", async () => {
    const device = await start();
    expect(device.api.needsFirstRun).toHaveBeenCalled();
    expect(screen.queryByRole("heading", { name: "Welcome to BhayanakShare" })).toBeNull();
    expect(screen.getByRole("navigation")).toBeTruthy();
  });

  it("does not hold the user up when the Device cannot say whether it is needed", async () => {
    await start(fakeApi({ needsFirstRun: () => Promise.reject(new Error("no")) }));
    expect(screen.queryByRole("heading", { name: "Welcome to BhayanakShare" })).toBeNull();
    expect(screen.getByRole("navigation")).toBeTruthy();
  });

  it("is not shown again after it was finished", async () => {
    // The Device keeps whether it was.
    let done = false;
    const device = await begin(
      needed({
        needsFirstRun: vi.fn(() => Promise.resolve(!done)),
        finishFirstRun: vi.fn(() => {
          done = true;
          return Promise.resolve(null);
        }),
      }),
    );
    fireEvent.click(getStarted());
    await screen.findByText("AAAA-AAAA");
    cleanup();
    await start(device);
    expect(screen.queryByRole("heading", { name: "Welcome to BhayanakShare" })).toBeNull();
  });
});

describe("Settings sections", () => {
  const open = async (device = fakeApi()) => {
    await start(device);
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    await screen.findByRole("heading", { name: "Diagnostics" });
    return device;
  };

  it("has a heading for each section, in order", async () => {
    await open();
    expect(screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent)).toEqual([
      "This Device",
      "Privacy",
      "Receiving",
      "App",
      "Identity",
      "Diagnostics",
    ]);
    // What each holds.
    const inSection = (name: string) => within(screen.getByRole("region", { name }));
    expect(inSection("This Device").getByRole("textbox", { name: "Device Name" })).toBeTruthy();
    expect(inSection("This Device").getByRole("heading", { name: "My ID", level: 4 })).toBeTruthy();
    expect(inSection("Privacy").getByRole("radio", { name: "Hidden" })).toBeTruthy();
    expect(inSection("Privacy").getByRole("checkbox", { name: "Public DHT" })).toBeTruthy();
    expect(inSection("Receiving").getByRole("button", { name: "Change the folder received files are saved to" })).toBeTruthy();
    expect(inSection("App").getByRole("checkbox", { name: "Start at login" })).toBeTruthy();
    expect(inSection("App").getByRole("button", { name: "Check for updates" })).toBeTruthy();
    expect(inSection("Identity").getByRole("button", { name: "Export identity…" })).toBeTruthy();
    expect(inSection("Diagnostics").getByRole("button", { name: "Export diagnostics…" })).toBeTruthy();
  });

  it("shows My ID: the Fingerprint, the Device ID and the share link with its QR code", async () => {
    await open();
    const region = screen.getByRole("region", { name: "My ID" });
    expect(await within(region).findByText(MY_ID)).toBeTruthy();
    expect(within(region).getByText(`bhayanakshare://add/${MY_ID}?name=Alice's%20desktop`)).toBeTruthy();
    expect(within(region).getByRole("img", { name: "QR code of the share link" })).toBeTruthy();
  });

  describe("Device Name", () => {
    const field = async () => {
      const input = (await screen.findByRole("textbox", { name: "Device Name" })) as HTMLInputElement;
      await waitFor(() => expect(input.value).toBe("Alice's desktop"));
      return input;
    };
    const save = () => screen.getByRole("button", { name: "Save" }) as HTMLButtonElement;

    it("shows the current name, and Save waits for a change", async () => {
      await open();
      const input = await field();
      expect(save().disabled).toBe(true);
      fireEvent.change(input, { target: { value: "Alice's laptop" } });
      expect(save().disabled).toBe(false);
      fireEvent.change(input, { target: { value: "  Alice's desktop " } });
      expect(save().disabled).toBe(true);
    });

    it("renames the Device at once, shows the name as kept, and the share link follows", async () => {
      const device = await open();
      const input = await field();
      fireEvent.change(input, { target: { value: "  Bob's PC " } });
      fireEvent.click(save());

      expect((await findStatus()).textContent).toBe("Saved as Bob's PC.");
      expect(device.api.setDeviceName).toHaveBeenCalledExactlyOnceWith("  Bob's PC ");
      expect(input.value).toBe("Bob's PC");
      expect(save().disabled).toBe(true);
      await screen.findByText(`bhayanakshare://add/${MY_ID}?name=Bob's%20PC`);
      expect(screen.queryByRole("alert")).toBeNull();
    });

    it("says inline that a name is needed, without asking the Device", async () => {
      const device = await open();
      const input = await field();
      fireEvent.change(input, { target: { value: "  " } });
      fireEvent.click(save());
      expect((await screen.findByRole("alert")).textContent).toBe("Enter a name for this Device.");
      expect(input.getAttribute("aria-invalid")).toBe("true");
      expect(device.api.setDeviceName).not.toHaveBeenCalled();
      expect(input.maxLength).toBe(64);
    });

    it("shows the Device's reason when it refuses the name, and keeps what was typed", async () => {
      await open(fakeApi({ setDeviceName: vi.fn(() => Promise.reject(new Error("the Device is shutting down"))) }));
      const input = await field();
      fireEvent.change(input, { target: { value: "Bob's PC" } });
      fireEvent.click(save());
      expect((await screen.findByRole("alert")).textContent).toBe(
        "Could not save the name. the Device is shutting down",
      );
      expect(input.value).toBe("Bob's PC");
      expect(saying().filter((el) => el.textContent?.startsWith("Saved"))).toHaveLength(0);
    });

    it("says so, and offers no editing, when the name cannot be read", async () => {
      await open(fakeApi({ deviceName: () => Promise.reject(new Error("no")) }));
      expect((await screen.findByRole("alert")).textContent).toContain("Could not read this Device's name");
      expect((screen.getByRole("textbox", { name: "Device Name" }) as HTMLInputElement).disabled).toBe(true);
      expect(save().disabled).toBe(true);
    });
  });

  describe("Public DHT", () => {
    const box = async () => {
      const found = (await screen.findByRole("checkbox", { name: "Public DHT" })) as HTMLInputElement;
      await waitFor(() => expect(found.disabled).toBe(false));
      return found;
    };

    it("is on as the Device has it, with a hint, and switches off and on", async () => {
      const device = await open();
      const dht = await box();
      expect(dht.checked).toBe(true);
      const hint = document.getElementById(dht.getAttribute("aria-describedby") ?? "");
      expect(hint?.textContent).toContain("public BitTorrent DHT");
      expect(hint?.textContent).toContain("Your name is never published");

      fireEvent.click(dht);
      await waitFor(() => expect(device.api.setPublicDht).toHaveBeenCalledWith(false));
      await waitFor(() => expect(dht.checked).toBe(false));
      fireEvent.click(dht);
      await waitFor(() => expect(device.api.setPublicDht).toHaveBeenLastCalledWith(true));
      await waitFor(() => expect(dht.checked).toBe(true));
    });

    it("is read again when Settings is opened again", async () => {
      await open();
      fireEvent.click(await box());
      await waitFor(async () => expect((await box()).checked).toBe(false));
      fireEvent.click(screen.getByRole("button", { name: "Home" }));
      fireEvent.click(screen.getByRole("button", { name: "Settings" }));
      await waitFor(async () => expect((await box()).checked).toBe(false));
    });

    it("keeps the old setting and says why when the change is refused", async () => {
      await open(fakeApi({ setPublicDht: vi.fn(() => Promise.reject(new Error("the Device is shutting down"))) }));
      const dht = await box();
      fireEvent.click(dht);
      expect((await screen.findByRole("alert")).textContent).toBe(
        "Could not change the public DHT. the Device is shutting down",
      );
      expect(dht.checked).toBe(true);
    });

    it("says so, and offers no choice, when the setting cannot be read", async () => {
      await open(fakeApi({ publicDht: () => Promise.reject(new Error("no")) }));
      expect((await screen.findByRole("alert")).textContent).toContain("Could not read whether the public DHT is on");
      expect((screen.getByRole("checkbox", { name: "Public DHT" }) as HTMLInputElement).disabled).toBe(true);
    });
  });

  describe("Receiving", () => {
    const change = () => screen.getByRole("button", { name: "Change the folder received files are saved to" });
    const shown = async () => {
      const region = screen.getByRole("region", { name: "Receiving" });
      await waitFor(() => expect(region.querySelector("code")?.textContent).not.toBe(""));
      return region.querySelector("code")!.textContent;
    };

    it("shows the save folder, and changes it from the folder dialog", async () => {
      const device = await open();
      expect(await shown()).toBe("/home/me/Downloads/BhayanakShare");

      fireEvent.click(change());
      expect(await screen.findByText("/mnt/big")).toBeTruthy();
      expect(device.api.setSaveFolder).toHaveBeenCalledExactlyOnceWith("/mnt/big");
      expect((await findStatus()).textContent).toContain("Files you receive from now on go there");
      expect(await shown()).toBe("/mnt/big");
    });

    it("shows the folder as the Device kept it", async () => {
      await open(fakeApi({ setSaveFolder: vi.fn(() => Promise.resolve("/mnt/big/received")) }));
      fireEvent.click(change());
      expect(await screen.findByText("/mnt/big/received")).toBeTruthy();
    });

    it("changes nothing when the folder dialog is cancelled", async () => {
      const device = await open(fakeApi({ pickFolder: vi.fn(() => Promise.resolve<string | null>(null)) }));
      await shown();
      fireEvent.click(change());
      await waitFor(() => expect(device.api.pickFolder).toHaveBeenCalled());
      expect(device.api.setSaveFolder).not.toHaveBeenCalled();
      expect(await shown()).toBe("/home/me/Downloads/BhayanakShare");
      expect(screen.queryByRole("alert")).toBeNull();
    });

    it("keeps the old folder and says why when the folder cannot be used", async () => {
      await open(
        fakeApi({ setSaveFolder: vi.fn(() => Promise.reject({ kind: "not_a_folder", message: "not a folder" })) }),
      );
      await shown();
      fireEvent.click(change());
      expect((await screen.findByRole("alert")).textContent).toBe("Could not use that folder. That is not a folder.");
      expect(await shown()).toBe("/home/me/Downloads/BhayanakShare");
    });

    it("is what the next Offer's sheet shows", async () => {
      const device = await open();
      await shown();
      fireEvent.click(change());
      await screen.findByText("/mnt/big");
      fireEvent.click(screen.getByRole("button", { name: "Home" }));
      await device.transfer("receiver", { kind: "offered" });
      const sheet = await screen.findByRole("dialog", { name: "Incoming files" });
      await waitFor(() => expect(sheet.textContent).toContain("/mnt/big"));
    });

    it("words each reason a folder is refused, and an unknown one with the Device's message", async () => {
      const cases: [string, string][] = [
        ["not_absolute", "Could not use that folder. Its path must be a full path, not a relative one."],
        ["not_a_folder", "Could not use that folder. That is not a folder."],
        ["cannot_create", "Could not use that folder. It does not exist and could not be created."],
        ["not_writable", "Could not use that folder. BhayanakShare cannot write to it."],
        ["not_text", "Could not use that folder. Its path cannot be used."],
        ["other", "Could not use that folder. the Device is shutting down"],
      ];
      for (const [kind, words] of cases) {
        await open(
          fakeApi({
            setSaveFolder: vi.fn(() => Promise.reject({ kind, message: "the Device is shutting down" })),
          }),
        );
        await shown();
        fireEvent.click(change());
        expect((await screen.findByRole("alert")).textContent).toBe(words);
        cleanup();
      }
    });

    it("says so when the folder cannot be read", async () => {
      await open(fakeApi({ saveFolder: () => Promise.reject(new Error("no")) }));
      expect((await screen.findByRole("alert")).textContent).toContain("Could not read the folder");
      expect((change() as HTMLButtonElement).disabled).toBe(true);
    });
  });

  describe("Updates", () => {
    const check = () => screen.getByRole("button", { name: "Check for updates" }) as HTMLButtonElement;
    const updates = () => within(screen.getByRole("region", { name: "Updates" }));

    it("shows this version", async () => {
      await open();
      expect(await updates().findByText("Version 0.1.0")).toBeTruthy();
    });

    it("says when the version cannot be read", async () => {
      await open(fakeApi({ appVersion: () => Promise.reject(new Error("no")) }));
      expect(await updates().findByText("The version of BhayanakShare could not be read.")).toBeTruthy();
    });

    it("looks now, and says when nothing newer was found", async () => {
      const device = await open();
      fireEvent.click(check());
      expect((await findStatus()).textContent).toBe("No newer version was found.");
      expect(device.api.checkForUpdate).toHaveBeenCalledTimes(1);
      expect(updates().queryByRole("button", { name: "Install and restart" })).toBeNull();
      expect(check().disabled).toBe(false);

      // An answer to this press: it is not there when Settings is opened again, nor on Home.
      fireEvent.click(screen.getByRole("button", { name: "Home" }));
      expect(screen.queryByText("No newer version was found.")).toBeNull();
      fireEvent.click(screen.getByRole("button", { name: "Settings" }));
      await screen.findByRole("heading", { name: "Diagnostics" });
      expect(screen.queryByText("No newer version was found.")).toBeNull();
    });

    it("says Checking while it looks, with the button off", async () => {
      let answer: (a: UpdateAction) => void = () => {};
      await open(fakeApi({ checkForUpdate: vi.fn(() => new Promise<UpdateAction>((r) => (answer = r))) }));
      fireEvent.click(check());
      expect((await findStatus()).textContent).toBe("Checking for updates…");
      expect(check().disabled).toBe(true);
      await act(async () => answer({ type: "none" }));
      expect((await findStatus()).textContent).toBe("No newer version was found.");
      expect(check().disabled).toBe(false);
    });

    it("offers an AppImage the update it found, installing only when asked", async () => {
      const device = await open(
        fakeApi({ checkForUpdate: vi.fn(() => Promise.resolve<UpdateAction>({ type: "install", version: "0.2.0" })) }),
      );
      expect(updates().queryByText(/Update available/)).toBeNull();
      fireEvent.click(check());
      expect(await updates().findByText(/Update available \(version 0\.2\.0\)/)).toBeTruthy();
      expect(device.api.installUpdate).not.toHaveBeenCalled();
      // It is an answer, not a banner: nothing to dismiss, and not shown twice.
      expect(updates().queryByRole("button", { name: "Dismiss" })).toBeNull();
      expect(screen.getAllByText(/Update available/)).toHaveLength(1);
      expect(screen.getAllByRole("button", { name: "Install and restart" })).toHaveLength(1);

      fireEvent.click(updates().getByRole("button", { name: "Install and restart" }));
      expect(await screen.findByText("Installing version 0.2.0…")).toBeTruthy();
      expect(device.api.installUpdate).toHaveBeenCalledExactlyOnceWith("0.2.0");
      expect(screen.getAllByText("Installing version 0.2.0…")).toHaveLength(1);
      expect(check().disabled).toBe(true);
    });

    it("links a package to the release page", async () => {
      const device = await open(
        fakeApi({ checkForUpdate: vi.fn(() => Promise.resolve<UpdateAction>({ type: "open_page", version: "0.2.0" })) }),
      );
      fireEvent.click(check());
      await updates().findByText(/Update available \(version 0\.2\.0\)/);
      expect(updates().queryByRole("button", { name: "Install and restart" })).toBeNull();
      fireEvent.click(updates().getByRole("link", { name: "Open the release page" }));
      expect(device.api.openUrl).toHaveBeenCalledWith(
        "https://github.com/theHimanshuShekhar/bhayanakshare/releases/latest",
      );
    });

    it("shows what the shell found already, and the banner shows what a check here found", async () => {
      const device = await open(
        fakeApi({
          pendingUpdate: () => Promise.resolve<UpdateAction>({ type: "install", version: "0.2.0" }),
        }),
      );
      expect(await updates().findByText(/Update available \(version 0\.2\.0\)/)).toBeTruthy();
      fireEvent.click(screen.getByRole("button", { name: "Home" }));
      expect(screen.getByText(/Update available \(version 0\.2\.0\)/)).toBeTruthy();
      expect(device.api.installUpdate).not.toHaveBeenCalled();
    });

    it("notes what a check in Settings found, for the banner on the other tabs", async () => {
      await open(
        fakeApi({ checkForUpdate: vi.fn(() => Promise.resolve<UpdateAction>({ type: "install", version: "0.3.0" })) }),
      );
      fireEvent.click(check());
      await updates().findByText(/Update available \(version 0\.3\.0\)/);
      fireEvent.click(screen.getByRole("button", { name: "Home" }));
      expect(screen.getByText(/Update available \(version 0\.3\.0\)/)).toBeTruthy();
      expect(screen.getByRole("button", { name: "Dismiss" })).toBeTruthy();
    });

    it("says when the check could not be made", async () => {
      await open(fakeApi({ checkForUpdate: vi.fn(() => Promise.reject(new Error("offline"))) }));
      fireEvent.click(check());
      expect((await screen.findByRole("alert")).textContent).toBe("Could not check for updates. Are you online?");
      expect(check().disabled).toBe(false);
    });

    it("is one update at a time: the check waits while an install runs, whoever started it", async () => {
      const device = await open(
        fakeApi({
          pendingUpdate: () => Promise.resolve<UpdateAction>({ type: "install", version: "0.2.0" }),
          installUpdate: vi.fn(() => new Promise(() => {})),
        }),
      );
      fireEvent.click(await updates().findByRole("button", { name: "Install and restart" }));
      expect(await screen.findByText("Installing version 0.2.0…")).toBeTruthy();
      expect(check().disabled).toBe(true);
      fireEvent.click(check());
      expect(device.api.checkForUpdate).not.toHaveBeenCalled();
    });

    it("shows what Update now is doing, once, while Settings is open", async () => {
      const device = await open(
        fakeApi({ checkForUpdate: vi.fn(() => Promise.resolve<UpdateAction>({ type: "none" })) }),
      );
      await device.push({
        type: "version_mismatch",
        peer: PEER_ID,
        peer_name: "Alice's Laptop",
        peer_app_version: "9.9.9",
        outdated: "this_device",
      });
      fireEvent.click(await screen.findByRole("button", { name: "Update now" }));
      expect((await screen.findByRole("alert")).textContent).toBe("No newer version was found.");
      expect(screen.getAllByText("No newer version was found.")).toHaveLength(1);
    });
  });
});

describe("Diagnostics", () => {
  const open = async (device = fakeApi()) => {
    await start(device);
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    await screen.findByRole("heading", { name: "Diagnostics" });
    return device;
  };
  const debugBox = async () => {
    const box = (await screen.findByRole("checkbox", { name: "Debug logging" })) as HTMLInputElement;
    await waitFor(() => expect(box.disabled).toBe(false));
    return box;
  };
  const exportButton = () => screen.getByRole("button", { name: "Export diagnostics…" });

  it("is a section of Settings, and says what debug logs hold and do not hold", async () => {
    await open();
    const section = screen.getByRole("region", { name: "Diagnostics" });
    const box = await debugBox();
    expect(within(section).getByRole("button", { name: "Export diagnostics…" })).toBeTruthy();
    const hint = document.getElementById(box.getAttribute("aria-describedby") ?? "");
    expect(hint?.textContent).toBe(
      "Debug logs are bigger, but still contain no file names, text or full Device IDs.",
    );
  });

  it("shows debug logging as the Device has it, and sets it", async () => {
    const device = await open();
    const box = await debugBox();
    expect(box.checked).toBe(false);

    fireEvent.click(box);
    await waitFor(() => expect(device.api.setDebugLogging).toHaveBeenCalledWith(true));
    await waitFor(() => expect(box.checked).toBe(true));

    // Read again when Settings is opened again.
    fireEvent.click(screen.getByRole("button", { name: "Home" }));
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    await waitFor(async () => expect(((await debugBox()) as HTMLInputElement).checked).toBe(true));
    fireEvent.click(await debugBox());
    await waitFor(() => expect(device.api.setDebugLogging).toHaveBeenLastCalledWith(false));
  });

  it("keeps the old setting and says why when the change is refused", async () => {
    await open(
      fakeApi({ setDebugLogging: () => Promise.reject(new Error("the Device is shutting down")) }),
    );
    const box = await debugBox();
    fireEvent.click(box);
    expect((await screen.findByRole("alert")).textContent).toContain("the Device is shutting down");
    expect(box.checked).toBe(false);
  });

  it("says so, and offers no switch, when the setting cannot be read", async () => {
    await open(fakeApi({ debugLogging: () => Promise.reject(new Error("no")) }));
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Could not read whether debug logging is on.",
    );
    expect((screen.getByRole("checkbox", { name: "Debug logging" }) as HTMLInputElement).disabled).toBe(true);
  });

  it("exports to the file the user chose, suggesting a name with today's date", async () => {
    const device = await open();
    fireEvent.click(exportButton());

    expect((await findStatus()).textContent).toBe("Saved to /home/me/diagnostics.zip.");
    expect(device.api.exportDiagnostics).toHaveBeenCalledWith("/home/me/diagnostics.zip");
    expect(device.api.pickDiagnosticsSavePath).toHaveBeenCalledTimes(1);
    expect(vi.mocked(device.api.pickDiagnosticsSavePath).mock.calls[0][0]).toMatch(
      /^bhayanakshare-diagnostics-\d{4}-\d{2}-\d{2}\.zip$/,
    );
  });

  it("exports nothing when the save dialog is cancelled", async () => {
    const device = await open(fakeApi({ pickDiagnosticsSavePath: () => Promise.resolve(null) }));
    fireEvent.click(exportButton());
    await waitFor(() => expect((exportButton() as HTMLButtonElement).disabled).toBe(false));
    expect(device.api.exportDiagnostics).not.toHaveBeenCalled();
    expect(saying()).toHaveLength(0);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("shows an error where the button is, and nothing is called saved", async () => {
    await open(
      fakeApi({ exportDiagnostics: () => Promise.reject("writing the diagnostics zip: No space left on device") }),
    );
    fireEvent.click(exportButton());
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Could not export diagnostics. writing the diagnostics zip: No space left on device",
    );
    expect(saying()).toHaveLength(0);
    // It can be tried again.
    expect((exportButton() as HTMLButtonElement).disabled).toBe(false);
  });

  it("clears an earlier result when exporting again", async () => {
    const exportDiagnostics = vi
      .fn<(path: string) => Promise<unknown>>()
      .mockResolvedValueOnce(null)
      .mockRejectedValueOnce(new Error("disk full"));
    await open(fakeApi({ exportDiagnostics }));
    fireEvent.click(exportButton());
    await findStatus();
    fireEvent.click(exportButton());
    expect((await screen.findByRole("alert")).textContent).toContain("disk full");
    expect(saying()).toHaveLength(0);
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
    const list = screen.getByRole("region", { name: "Transfers" });
    expect(within(list).queryByText(/The other Device went away/)).toBeNull();
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

describe("History", () => {
  const SAVED = "/home/me/Downloads/BhayanakShare/photo.jpg";
  const MORE_ID = "Q7ZZ4RTB" + "C".repeat(44);

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

  const single = (over: Partial<TransferRecord> = {}, saved_present: boolean | null = null): HistoryEntry => ({
    kind: "transfer",
    transfer: { record: record(over), saved_present },
  });

  /** The ID of the nth Transfer of a test. */
  const id = (n: number) => n.toString(16).padStart(2, "0").repeat(16);

  /** A Batch sent to Receivers named `Device 0`…, each ending as `states` say. */
  function batch(states: TransferState[]): HistoryEntry {
    return {
      kind: "batch",
      batch_id: BATCH,
      transfers: states.map((state, i) => ({
        saved_present: null,
        record: record({
          id: id(i + 1),
          role: "sender",
          peer: i === 0 ? PEER_ID : `${i}`.repeat(52),
          peer_name: `Device ${i}`,
          name: "report.txt",
          items: ["report.txt"],
          batch_id: BATCH,
          state,
        }),
      })),
    };
  }

  async function openHistory(device: ReturnType<typeof fakeApi>, entries: HistoryEntry[]) {
    device.setHistory(entries);
    await start(device);
    fireEvent.click(screen.getByRole("button", { name: "History" }));
  }

  const lastQuery = (device: ReturnType<typeof fakeApi>) => device.historyReads.at(-1);

  it("says when there is nothing yet, and when nothing matches the filters", async () => {
    const device = fakeApi();
    await openHistory(device, []);
    expect(await screen.findByText("No Transfers in your History yet.")).toBeTruthy();

    fireEvent.change(screen.getByLabelText("Search by item name"), { target: { value: "zzz" } });
    expect(await screen.findByText("No Transfers match.")).toBeTruthy();
  });

  it("lists what was received: whom from, what, how big, when, how it ended and where it was saved", async () => {
    const device = fakeApi();
    await openHistory(device, [
      single({ adjusted_names: 2, name: "photos", items: ["photos", "a.txt"], file_count: 12, size: 5 << 20 }, true),
    ]);
    const row = (await screen.findByText("photos and 1 more from Laptop · K3QF-7XNA")).closest("li")!;
    expect(within(row).getByText("Received.")).toBeTruthy();
    expect(within(row).getByText("5 MiB · 12 files · 2 names adjusted")).toBeTruthy();
    // The times are the user's own format; each is there.
    const times = within(row).getByText(/^Offered .* · Accepted .* · Finished /);
    expect(times.textContent).toContain(new Date(1_700_000_005_000).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" }));
    expect(within(row).getByText(`Saved to ${SAVED}`, { exact: false })).toBeTruthy();

    fireEvent.click(within(row).getByRole("button", { name: "Show in folder: photos and 1 more" }));
    expect(device.api.showInFolder).toHaveBeenCalledWith(SAVED);
  });

  it("says the file is no longer at its saved location instead of offering to show it", async () => {
    const device = fakeApi();
    await openHistory(device, [single({}, false)]);
    const row = (await screen.findByText("photo.jpg from Laptop · K3QF-7XNA")).closest("li")!;
    expect(within(row).getByText("File no longer at saved location")).toBeTruthy();
    expect(within(row).queryByRole("button", { name: /in folder/ })).toBeNull();
  });

  it("shows a text in full as plain text, with Copy, whichever way it went", async () => {
    const device = fakeApi();
    await openHistory(device, [
      single({ id: id(2), role: "sender", kind: "text", text: "my <b>reply</b>", name: "", items: [], file_count: 0, state: { kind: "completed", saved_to: null } }),
      single({ kind: "text", text: "<img src=x onerror=alert(1)>", name: "", items: [], file_count: 0, state: { kind: "completed", saved_to: null } }),
      // A text that was declined is not kept.
      single({ id: id(3), kind: "text", text: null, name: "", items: [], file_count: 0, state: { kind: "declined" } }),
    ]);
    const received = (await screen.findAllByText("Text from Laptop · K3QF-7XNA"))[0].closest("li")!;
    expect(within(received).getByRole("region", { name: "Text" }).textContent).toBe("<img src=x onerror=alert(1)>");
    expect(received.querySelector("img")).toBeNull();
    expect(within(received).queryByRole("button", { name: /in folder/ })).toBeNull();
    fireEvent.click(within(received).getByRole("button", { name: "Copy the text from Laptop · K3QF-7XNA" }));
    await waitFor(() => expect(device.api.copyText).toHaveBeenCalledWith("<img src=x onerror=alert(1)>"));

    const sent = screen.getByText("Text to Laptop · K3QF-7XNA").closest("li")!;
    expect(within(sent).getByRole("region", { name: "Text" }).textContent).toBe("my <b>reply</b>");
    fireEvent.click(within(sent).getByRole("button", { name: "Copy the text sent to Laptop · K3QF-7XNA" }));
    await waitFor(() => expect(device.api.copyText).toHaveBeenCalledWith("my <b>reply</b>"));

    const declined = screen.getAllByText("Text from Laptop · K3QF-7XNA")[1].closest("li")!;
    expect(within(declined).getByText("You declined.")).toBeTruthy();
    expect(within(declined).queryByRole("region", { name: "Text" })).toBeNull();
  });

  it("shows a Batch as one entry that opens to each Receiver, and counts them by how they ended", async () => {
    const device = fakeApi();
    await openHistory(device, [
      batch([{ kind: "completed", saved_to: null }, { kind: "declined" }, { kind: "failed", reason: "Could not reach the receiving Device." }]),
    ]);
    const entry = (await screen.findByText("report.txt to 3 Devices")).closest("li")!;
    expect(within(entry).getByText("1 of 3 delivered, 1 declined, 1 failed")).toBeTruthy();
    expect(within(entry).queryByText("report.txt to Device 1 · 1111-1111")).toBeNull();

    fireEvent.click(within(entry).getByRole("button", { name: /Show each Device/ }));
    expect(within(entry).getByText("report.txt to Device 0 · K3QF-7XNA")).toBeTruthy();
    expect(within(entry).getByText("Could not send. Could not reach the receiving Device.")).toBeTruthy();
    expect(within(entry).getByText("Device 1 · 1111-1111 declined.")).toBeTruthy();
  });

  it("offers Retry for a Failed Transfer of a Batch, once, and never for a Declined one", async () => {
    const device = fakeApi();
    const failed: TransferState = { kind: "failed", reason: "gone" };
    await openHistory(device, [batch([failed, { kind: "declined" }, failed])]);
    const entry = (await screen.findByText("report.txt to 3 Devices")).closest("li")!;
    fireEvent.click(within(entry).getByRole("button", { name: /Show each Device/ }));
    const retries = within(entry).getAllByRole("button", { name: /^Retry/ });
    expect(retries).toHaveLength(2);

    fireEvent.click(within(entry).getByRole("button", { name: "Retry report.txt to Device 2 · 2222-2222" }));
    expect(device.api.retryTransfer).toHaveBeenCalledWith(id(3));
    expect(within(entry).queryByRole("button", { name: "Retry report.txt to Device 1 · 1111-1111" })).toBeNull();
  });

  it("does not offer Retry for a Failed Transfer that was sent again, nor one that is not in a Batch", async () => {
    const device = fakeApi();
    const sent = batch([{ kind: "failed", reason: "gone" }, { kind: "completed", saved_to: null }]);
    // A later Transfer of the same Batch to the same Receiver, which went through.
    if (sent.kind !== "batch") throw new Error("a Batch");
    const again = { ...sent.transfers[0], record: { ...sent.transfers[0].record, id: id(9), state: { kind: "completed", saved_to: null } as TransferState } };
    sent.transfers.push(again);
    const alone = single({ id: id(5), role: "sender", name: "alone.txt", items: ["alone.txt"], state: { kind: "failed", reason: "gone" } });
    await openHistory(device, [alone, sent]);

    const entry = (await screen.findByText("report.txt to 2 Devices")).closest("li")!;
    fireEvent.click(within(entry).getByRole("button", { name: /Show each Device/ }));
    expect(screen.queryByRole("button", { name: /^Retry/ })).toBeNull();
  });

  it("deletes one Transfer and reads History again; a Transfer that is still going has no Delete", async () => {
    const running = single({ id: id(4), role: "sender", name: "big.iso", items: ["big.iso"], state: { kind: "transferring" } });
    const ended = single({ id: id(2) });
    const device = fakeApi({
      deleteHistoryTransfer: vi.fn((_id: string) => {
        device.setHistory([running]);
        return Promise.resolve(null);
      }),
    });
    await openHistory(device, [running, ended]);
    const row = (await screen.findByText("photo.jpg from Laptop · K3QF-7XNA")).closest("li")!;
    const going = screen.getByText("big.iso to Laptop · K3QF-7XNA").closest("li")!;
    expect(within(going).queryByRole("button", { name: /^Delete/ })).toBeNull();

    fireEvent.click(within(row).getByRole("button", { name: "Delete photo.jpg from Laptop · K3QF-7XNA from History" }));
    await waitFor(() => expect(device.api.deleteHistoryTransfer).toHaveBeenCalledWith(id(2)));
    await waitFor(() => expect(screen.queryByText("photo.jpg from Laptop · K3QF-7XNA")).toBeNull());
    expect(screen.getByText("big.iso to Laptop · K3QF-7XNA")).toBeTruthy();
  });

  it("says when a Transfer could not be deleted", async () => {
    const device = fakeApi({ deleteHistoryTransfer: () => Promise.reject(new Error("it has not ended")) });
    await openHistory(device, [single()]);
    fireEvent.click(await screen.findByRole("button", { name: /^Delete/ }));
    expect((await screen.findByRole("alert")).textContent).toContain("Could not delete it. it has not ended");
  });

  it("deletes a Batch once all of it has ended, and not before", async () => {
    const device = fakeApi();
    await openHistory(device, [batch([{ kind: "completed", saved_to: null }, { kind: "transferring" }])]);
    const entry = (await screen.findByText("report.txt to 2 Devices")).closest("li")!;
    expect(within(entry).queryByRole("button", { name: /^Delete all/ })).toBeNull();
    cleanup();

    const done = fakeApi();
    await openHistory(done, [batch([{ kind: "completed", saved_to: null }, { kind: "declined" }])]);
    fireEvent.click(await screen.findByRole("button", { name: "Delete all of report.txt from History" }));
    await waitFor(() => expect(done.api.deleteHistoryBatch).toHaveBeenCalledWith(BATCH));
  });

  it("narrows History by Device, direction and item name through the Device", async () => {
    const device = fakeApi({}, [contact({ nickname: "Mum" })]);
    await openHistory(device, [single(), single({ id: id(2), peer: MORE_ID, peer_name: "Phone" })]);
    await screen.findByText("photo.jpg from Mum");
    expect(lastQuery(device)).toEqual([null, null, null]);

    // Contacts, and Devices seen in History, can be chosen.
    const choices = within(screen.getByLabelText("Device")).getAllByRole("option").map((o) => o.textContent);
    expect(choices).toEqual(["Any Device", "Mum", "Phone · Q7ZZ-4RTB"]);
    fireEvent.change(screen.getByLabelText("Device"), { target: { value: MORE_ID } });
    await waitFor(() => expect(lastQuery(device)).toEqual([MORE_ID, null, null]));

    fireEvent.change(screen.getByLabelText("Direction"), { target: { value: "sender" } });
    await waitFor(() => expect(lastQuery(device)).toEqual([MORE_ID, "sender", null]));
    fireEvent.change(screen.getByLabelText("Search by item name"), { target: { value: " report " } });
    await waitFor(() => expect(lastQuery(device)).toEqual([MORE_ID, "sender", "report"]));
    // Narrowed to one Device, the others can still be chosen.
    expect(within(screen.getByLabelText("Device")).getAllByRole("option")).toHaveLength(3);

    fireEvent.change(screen.getByLabelText("Device"), { target: { value: "" } });
    await waitFor(() => expect(lastQuery(device)).toEqual([null, "sender", "report"]));
  });

  it("opens a Contact's History from the Contacts tab, narrowed to that Contact", async () => {
    const device = fakeApi({}, [contact({ nickname: "Mum" })]);
    device.setHistory([single({ peer_name: "Mum's phone" })]);
    await start(device);
    fireEvent.click(screen.getByRole("button", { name: "Contacts" }));
    fireEvent.click(await screen.findByRole("button", { name: "Show the Transfer History with Mum" }));

    expect(screen.getByRole("button", { name: "History" }).getAttribute("aria-current")).toBe("page");
    await waitFor(() => expect(lastQuery(device)).toEqual([PEER_ID, null, null]));
    expect((screen.getByLabelText("Device") as HTMLSelectElement).value).toBe(PEER_ID);
    // The Contact is called what the user calls it.
    expect(await screen.findByText("photo.jpg from Mum")).toBeTruthy();
  });

  it("clears History only after asking, and reads it again", async () => {
    const device = fakeApi();
    await openHistory(device, [single()]);
    await screen.findByText("photo.jpg from Laptop · K3QF-7XNA");

    fireEvent.click(screen.getByRole("button", { name: "Clear History…" }));
    const dialog = screen.getByRole("alertdialog", { name: "Clear History?" });
    expect(dialog.textContent).toContain("Transfers still going are not touched");
    // The safe answer has focus, so Enter cannot clear it by accident.
    expect(document.activeElement).toBe(within(dialog).getByRole("button", { name: "Keep" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Keep" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(device.api.clearHistory).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Clear History…" }));
    device.setHistory([]);
    const reads = device.historyReads.length;
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Clear History" }));
    await waitFor(() => expect(device.api.clearHistory).toHaveBeenCalledTimes(1));
    expect(await screen.findByText("No Transfers in your History yet.")).toBeTruthy();
    expect(device.historyReads.length).toBeGreaterThan(reads);
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("says when History could not be cleared", async () => {
    const device = fakeApi({ clearHistory: () => Promise.reject(new Error("disk full")) });
    await openHistory(device, [single()]);
    fireEvent.click(await screen.findByRole("button", { name: "Clear History…" }));
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Clear History" }));
    expect((await screen.findByRole("alert")).textContent).toContain("Could not clear History. disk full");
    expect(screen.getByRole("alertdialog")).toBeTruthy();
  });

  it("reads History again when a Transfer of this session changes state", async () => {
    const device = fakeApi();
    await openHistory(device, []);
    await screen.findByText("No Transfers in your History yet.");
    const reads = device.historyReads.length;

    await device.transfer("receiver", { kind: "offered" }, "Laptop");
    await waitFor(() => expect(device.historyReads.length).toBeGreaterThan(reads));
    // Progress is not a change of state.
    const after = device.historyReads.length;
    await device.push({ type: "progress", transfer_id: TRANSFER, bytes: 5, total: 2048 });
    expect(device.historyReads.length).toBe(after);
  });

  it("says when History could not be read", async () => {
    const device = fakeApi({ history: () => Promise.reject(new Error("db")) });
    await openHistory(device, []);
    expect((await screen.findByRole("alert")).textContent).toBe("Could not load your History.");
  });
});

describe("version mismatches", () => {
  const RELEASES = "https://github.com/theHimanshuShekhar/bhayanakshare/releases/latest";
  const refused = (device: ReturnType<typeof fakeApi>, outdated: "this_device" | "peer") =>
    device.push({
      type: "version_mismatch",
      peer: PEER_ID,
      peer_name: "Alice's Laptop",
      peer_app_version: "9.9.9",
      outdated,
    });

  it("tells the user to ask the other Device to update when that one is older", async () => {
    const device = await start();
    await refused(device, "peer");
    expect(
      screen.getByText(/Alice's Laptop · K3QF-7XNA is running an older BhayanakShare. Ask them to update./),
    ).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Update now" })).toBeNull();
  });

  it("offers Update now when this Device is the older one, and opens the release page for a package", async () => {
    const device = await start(fakeApi({ checkForUpdate: () => Promise.resolve({ type: "open_page", version: "9.9.9" }) }));
    await refused(device, "this_device");
    expect(screen.getByText(/This Device is running an older BhayanakShare than Alice's Laptop/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Update now" }));
    await waitFor(() => expect(device.api.openUrl).toHaveBeenCalledWith(RELEASES));
    expect(device.api.installUpdate).not.toHaveBeenCalled();
  });

  it("Update now looks for the update and installs it on an AppImage, saying which version", async () => {
    // The install does not finish: the app is restarting.
    const device = await start(
      fakeApi({
        checkForUpdate: vi.fn(() => Promise.resolve<UpdateAction>({ type: "install", version: "9.9.9" })),
        installUpdate: vi.fn(() => new Promise(() => {})),
      }),
    );
    await refused(device, "this_device");
    fireEvent.click(screen.getByRole("button", { name: "Update now" }));
    expect((await screen.findByText("Installing version 9.9.9…")).getAttribute("role")).toBe("status");
    expect(device.api.checkForUpdate).toHaveBeenCalledTimes(1);
    // The version it was shown is the version it installs.
    expect(device.api.installUpdate).toHaveBeenCalledWith("9.9.9");
    expect(device.api.openUrl).not.toHaveBeenCalled();
    expect((screen.getByRole("button", { name: "Update now" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("says when no update was found", async () => {
    const device = await start();
    await refused(device, "this_device");
    fireEvent.click(screen.getByRole("button", { name: "Update now" }));
    expect((await screen.findByRole("alert")).textContent).toBe("No newer version was found.");
    expect(device.api.installUpdate).not.toHaveBeenCalled();
    expect(device.api.openUrl).not.toHaveBeenCalled();
  });

  it("says when the check could not be made, as when offline", async () => {
    const device = await start(fakeApi({ checkForUpdate: () => Promise.reject("error sending request") }));
    await refused(device, "this_device");
    fireEvent.click(screen.getByRole("button", { name: "Update now" }));
    expect((await screen.findByRole("alert")).textContent).toBe("Could not check for updates. Are you online?");
    // The button is there to try again.
    expect((screen.getByRole("button", { name: "Update now" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("says when the update could not be installed", async () => {
    const device = await start(
      fakeApi({
        checkForUpdate: () => Promise.resolve({ type: "install", version: "9.9.9" }),
        installUpdate: () => Promise.reject("signature_invalid"),
      }),
    );
    await refused(device, "this_device");
    fireEvent.click(screen.getByRole("button", { name: "Update now" }));
    expect((await screen.findByRole("alert")).textContent).toBe(
      "The update's signature did not check out, so it was not installed.",
    );
  });

  it("asks first when Transfers are in progress, and installs once the user says so", async () => {
    const device = await start(
      fakeApi({
        checkForUpdate: () => Promise.resolve({ type: "install", version: "9.9.9" }),
        transfersInProgress: () => Promise.resolve(2),
      }),
    );
    await refused(device, "this_device");
    fireEvent.click(screen.getByRole("button", { name: "Update now" }));
    expect((await screen.findByRole("alert")).textContent).toContain(
      "2 Transfers are in progress. They stop for the restart and resume when BhayanakShare is back.",
    );
    expect(device.api.installUpdate).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Install version 9.9.9 and restart" }));
    await waitFor(() => expect(device.api.installUpdate).toHaveBeenCalledWith("9.9.9"));
  });

  it("does not install when the user says not now", async () => {
    const device = await start(
      fakeApi({
        checkForUpdate: () => Promise.resolve({ type: "install", version: "9.9.9" }),
        transfersInProgress: () => Promise.resolve(1),
      }),
    );
    await refused(device, "this_device");
    fireEvent.click(screen.getByRole("button", { name: "Update now" }));
    expect((await screen.findByRole("alert")).textContent).toContain("A Transfer is in progress. It stops");
    fireEvent.click(screen.getByRole("button", { name: "Not now" }));
    expect(screen.queryByRole("alert")).toBeNull();
    expect(device.api.installUpdate).not.toHaveBeenCalled();
    expect((screen.getByRole("button", { name: "Update now" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("goes away when dismissed", async () => {
    const device = await start();
    await refused(device, "this_device");
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByRole("button", { name: "Update now" })).toBeNull();
  });
});

describe("updates", () => {
  const RELEASES = "https://github.com/theHimanshuShekhar/bhayanakshare/releases/latest";
  const found = (action: UpdateAction, overrides: Partial<Api> = {}) =>
    fakeApi({ pendingUpdate: () => Promise.resolve(action), ...overrides });
  const installing = (): Partial<Api> => ({ installUpdate: vi.fn(() => new Promise(() => {})) });

  it("shows nothing when there is no newer release", async () => {
    await start();
    expect(screen.queryByText(/Update available/)).toBeNull();
  });

  it("offers an AppImage to install the new version and restart, and installs only when asked", async () => {
    const device = await start(found({ type: "install", version: "0.2.0" }));
    expect((await screen.findByText(/Update available \(version 0\.2\.0\)/)).getAttribute("role")).toBe("status");
    expect(screen.queryByRole("link", { name: "Open the release page" })).toBeNull();
    expect(device.api.installUpdate).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Install and restart" }));
    expect(await screen.findByText("Installing version 0.2.0…")).toBeTruthy();
    // No Transfers in progress: nothing more to ask.
    expect(device.api.installUpdate).toHaveBeenCalledWith("0.2.0");
    expect(device.api.openUrl).not.toHaveBeenCalled();
  });

  it("installs the version the banner showed, not whatever is newest now", async () => {
    const device = await start(found({ type: "install", version: "0.2.0" }));
    fireEvent.click(await screen.findByRole("button", { name: "Install and restart" }));
    await waitFor(() => expect(device.api.installUpdate).toHaveBeenCalledWith("0.2.0"));
  });

  it("links a package to the release page instead of updating it", async () => {
    const device = await start(found({ type: "open_page", version: "0.2.0" }));
    expect(await screen.findByText(/Update available \(version 0\.2\.0\)/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Install and restart" })).toBeNull();

    fireEvent.click(screen.getByRole("link", { name: "Open the release page" }));
    expect(device.api.openUrl).toHaveBeenCalledWith(RELEASES);
    expect(device.api.installUpdate).not.toHaveBeenCalled();
  });

  it("shows an update the shell finds while the app is open", async () => {
    const device = await start();
    await device.shell({ type: "update_available", action: { type: "install", version: "0.3.0" } });
    expect(screen.getByText(/Update available \(version 0\.3\.0\)/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Install and restart" })).toBeTruthy();
  });

  it("says in words why the update could not be installed, and lets the user try again", async () => {
    const cases: [string, string][] = [
      ["version_changed", "A newer version came out in the meantime. Check again to install it."],
      ["none_pending", "There is no update to install. Check again."],
      ["already_installing", "An update is already being installed."],
      ["quitting", "BhayanakShare is closing, so nothing was installed."],
      ["not_app_image", "This install is updated from the release page, not by BhayanakShare itself."],
      ["download_failed", "Could not download the update. Are you online?"],
      ["signature_invalid", "The update's signature did not check out, so it was not installed."],
      ["install_failed", "Could not install the update."],
      // Not a kind: never shown as it is, as it could be English or name a path.
      ["No such file /home/me/x.AppImage", "Could not install the update."],
    ];
    for (const [kind, words] of cases) {
      const device = await start(
        found(
          { type: "install", version: "0.2.0" },
          { installUpdate: vi.fn(() => Promise.reject(kind)) },
        ),
      );
      fireEvent.click(await screen.findByRole("button", { name: "Install and restart" }));
      expect((await screen.findByRole("alert")).textContent).toBe(words);
      expect((screen.getByRole("button", { name: "Install and restart" }) as HTMLButtonElement).disabled).toBe(false);
      expect(device.api.installUpdate).toHaveBeenCalledTimes(1);
      cleanup();
    }
  });

  it("asks first when Transfers are in progress, and installs only once the user says so", async () => {
    const device = await start(found({ type: "install", version: "0.2.0" }, { transfersInProgress: () => Promise.resolve(3) }));
    fireEvent.click(await screen.findByRole("button", { name: "Install and restart" }));
    expect((await screen.findByRole("alert")).textContent).toContain(
      "3 Transfers are in progress. They stop for the restart and resume when BhayanakShare is back.",
    );
    expect(device.api.installUpdate).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Install version 0.2.0 and restart" }));
    expect(await screen.findByText("Installing version 0.2.0…")).toBeTruthy();
    expect(device.api.installUpdate).toHaveBeenCalledWith("0.2.0");
  });

  it("says Transfer, not Transfers, for one in progress, and does nothing on not now", async () => {
    const device = await start(found({ type: "install", version: "0.2.0" }, { transfersInProgress: () => Promise.resolve(1) }));
    fireEvent.click(await screen.findByRole("button", { name: "Install and restart" }));
    expect((await screen.findByRole("alert")).textContent).toContain("A Transfer is in progress. It stops for the restart");

    fireEvent.click(screen.getByRole("button", { name: "Not now" }));
    expect(screen.queryByRole("alert")).toBeNull();
    expect(device.api.installUpdate).not.toHaveBeenCalled();
    expect((screen.getByRole("button", { name: "Install and restart" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("installs without asking when how many Transfers are in progress cannot be read", async () => {
    const device = await start(
      found({ type: "install", version: "0.2.0" }, { transfersInProgress: () => Promise.reject(new Error("db")) }),
    );
    fireEvent.click(await screen.findByRole("button", { name: "Install and restart" }));
    await waitFor(() => expect(device.api.installUpdate).toHaveBeenCalledWith("0.2.0"));
  });

  it("is one update at a time: Update now waits while the banner's install runs", async () => {
    const device = await start(found({ type: "install", version: "0.2.0" }, installing()));
    await device.push({
      type: "version_mismatch",
      peer: PEER_ID,
      peer_name: "Alice's Laptop",
      peer_app_version: "9.9.9",
      outdated: "this_device",
    });
    fireEvent.click(await screen.findByRole("button", { name: "Install and restart" }));
    expect(await screen.findByText("Installing version 0.2.0…")).toBeTruthy();

    // Both buttons are off, so there is no second install to fail, and one status line.
    expect((screen.getByRole("button", { name: "Update now" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Install and restart" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Update now" }));
    expect(device.api.checkForUpdate).not.toHaveBeenCalled();
    expect(screen.getAllByText("Installing version 0.2.0…")).toHaveLength(1);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("is one update at a time: the banner waits while Update now's install runs", async () => {
    const device = await start(
      found({ type: "install", version: "0.2.0" }, {
        ...installing(),
        checkForUpdate: vi.fn(() => Promise.resolve<UpdateAction>({ type: "install", version: "0.2.0" })),
      }),
    );
    await device.push({
      type: "version_mismatch",
      peer: PEER_ID,
      peer_name: "Alice's Laptop",
      peer_app_version: "9.9.9",
      outdated: "this_device",
    });
    await screen.findByRole("button", { name: "Install and restart" });
    fireEvent.click(screen.getByRole("button", { name: "Update now" }));
    expect(await screen.findByText("Installing version 0.2.0…")).toBeTruthy();
    expect((screen.getByRole("button", { name: "Install and restart" }) as HTMLButtonElement).disabled).toBe(true);
    expect(device.api.installUpdate).toHaveBeenCalledTimes(1);
  });

  it("goes away when dismissed, until there is a newer version still", async () => {
    const device = await start(found({ type: "open_page", version: "0.2.0" }));
    fireEvent.click(await screen.findByRole("button", { name: "Dismiss" }));
    expect(screen.queryByText(/Update available/)).toBeNull();

    await device.shell({ type: "update_available", action: { type: "open_page", version: "0.2.0" } });
    expect(screen.queryByText(/Update available/)).toBeNull();
    await device.shell({ type: "update_available", action: { type: "open_page", version: "0.3.0" } });
    expect(screen.getByText(/Update available \(version 0\.3\.0\)/)).toBeTruthy();
  });
});

describe("Identity", () => {
  const PASSWORD = "correct horse";
  const fail = (kind: string, message = "details") => Promise.reject({ kind, message });

  const open = async (device = fakeApi()) => {
    await start(device);
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    await screen.findByRole("heading", { name: "Identity" });
    return device;
  };
  const typeInto = (label: string, value: string) =>
    fireEvent.change(screen.getByLabelText(label), { target: { value } });
  /** Clicks as a person does, which leaves the button focused (a bare click event does not). */
  const press = (name: string) => {
    const button = screen.getByRole("button", { name });
    button.focus();
    fireEvent.click(button);
  };
  const startExport = async (device = fakeApi()) => {
    await open(device);
    await waitFor(() =>
      expect((screen.getByRole("button", { name: "Export identity…" }) as HTMLButtonElement).disabled).toBe(false),
    );
    press("Export identity…");
    await screen.findByRole("dialog", { name: "Export identity" });
    return device;
  };
  const fillExport = (password: string, again = password) => {
    typeInto("Password", password);
    typeInto("Type the password again", again);
    fireEvent.click(screen.getByRole("button", { name: "Save as…" }));
  };
  const startImport = async (device = fakeApi()) => {
    await open(device);
    await waitFor(() =>
      expect((screen.getByRole("button", { name: "Import identity…" }) as HTMLButtonElement).disabled).toBe(false),
    );
    press("Import identity…");
    await screen.findByLabelText("Password of this file");
    return device;
  };
  /** Through the password step, to the confirmation. */
  const toConfirmation = async (device: ReturnType<typeof fakeApi>) => {
    typeInto("Password of this file", PASSWORD);
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    await screen.findByRole("alertdialog", { name: "Replace this Device's identity?" });
    return device;
  };

  it("is a section of Settings with this Device's Fingerprint and the two actions", async () => {
    await open();
    const section = screen.getByRole("region", { name: "Identity" });
    expect(await within(section).findByText("AAAA-AAAA")).toBeTruthy();
    expect(within(section).getByRole("button", { name: "Export identity…" })).toBeTruthy();
    expect(within(section).getByRole("button", { name: "Import identity…" })).toBeTruthy();
  });

  describe("export", () => {
    it("asks for the password twice, and refuses a short one before any dialog or file", async () => {
      const device = await startExport();
      fillExport("short");
      expect((await screen.findByRole("alert")).textContent).toBe("Use at least 8 characters.");
      expect(device.api.pickIdentitySavePath).not.toHaveBeenCalled();
      expect(device.api.exportIdentity).not.toHaveBeenCalled();
    });

    it("refuses two passwords that differ", async () => {
      const device = await startExport();
      fillExport(PASSWORD, PASSWORD + "!");
      expect((await screen.findByRole("alert")).textContent).toBe("The two passwords are not the same.");
      expect(device.api.pickIdentitySavePath).not.toHaveBeenCalled();
      expect(device.api.exportIdentity).not.toHaveBeenCalled();
    });

    it("labels both fields, and keeps them out of sight", async () => {
      await startExport();
      for (const label of ["Password", "Type the password again"]) {
        expect(screen.getByLabelText(label).getAttribute("type")).toBe("password");
      }
      // The first field has focus, so typing can start at once. An effect moves it, which can
      // run after the field is found.
      await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText("Password")));
    });

    it("suggests a file name with the Fingerprint, writes the file there and says where", async () => {
      const device = await startExport();
      fillExport(PASSWORD);
      await waitFor(() =>
        expect(device.api.pickIdentitySavePath).toHaveBeenCalledWith("bhayanakshare-identity-AAAA-AAAA.bhid"),
      );
      await waitFor(() => expect(device.api.exportIdentity).toHaveBeenCalledWith("/home/me/id.bhid", PASSWORD));
      expect((await findStatus()).textContent).toContain("Saved to /home/me/id.bhid");
      const close = screen.getByRole("button", { name: "Close" });
      await waitFor(() => expect(document.activeElement).toBe(close));
      // The password does not outlive its use.
      expect(screen.queryByLabelText("Password")).toBeNull();
      fireEvent.click(close);
      expect(screen.queryByRole("dialog")).toBeNull();
      expect(document.activeElement).toBe(screen.getByRole("button", { name: "Export identity…" }));
    });

    it("writes nothing when the save dialog is cancelled, and stays open", async () => {
      const device = await startExport(
        fakeApi({ pickIdentitySavePath: vi.fn(() => Promise.resolve<string | null>(null)) }),
      );
      fillExport(PASSWORD);
      await waitFor(() => expect(device.api.pickIdentitySavePath).toHaveBeenCalled());
      expect(device.api.exportIdentity).not.toHaveBeenCalled();
      expect(screen.getByRole("dialog", { name: "Export identity" })).toBeTruthy();
      expect(screen.queryByRole("alert")).toBeNull();
    });

    it("shows a failure to write in the dialog", async () => {
      await startExport(fakeApi({ exportIdentity: vi.fn(() => fail("other", "writing /ro/id.bhid: read-only")) }));
      fillExport(PASSWORD);
      expect((await screen.findByRole("alert")).textContent).toBe(
        "Something went wrong. writing /ro/id.bhid: read-only",
      );
    });

    it("is closed by Escape and Cancel, which hand focus back", async () => {
      await startExport();
      fireEvent.keyDown(screen.getByLabelText("Password"), { key: "Escape" });
      expect(screen.queryByRole("dialog")).toBeNull();
      expect(document.activeElement).toBe(screen.getByRole("button", { name: "Export identity…" }));
      press("Export identity…");
      fireEvent.click(await screen.findByRole("button", { name: "Cancel" }));
      expect(screen.queryByRole("dialog")).toBeNull();
    });
  });

  describe("import", () => {
    it("does nothing when no file is chosen", async () => {
      const device = await open(fakeApi({ pickIdentityFile: vi.fn(() => Promise.resolve<string | null>(null)) }));
      await waitFor(() =>
        expect((screen.getByRole("button", { name: "Import identity…" }) as HTMLButtonElement).disabled).toBe(false),
      );
      fireEvent.click(screen.getByRole("button", { name: "Import identity…" }));
      await waitFor(() => expect(device.api.pickIdentityFile).toHaveBeenCalled());
      expect(screen.queryByRole("dialog")).toBeNull();
    });

    it("names the file and asks for its password, with focus on the field", async () => {
      await startImport();
      expect(screen.getByText("File: old-laptop.bhid")).toBeTruthy();
      expect(screen.getByLabelText("Password of this file").getAttribute("type")).toBe("password");
      await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText("Password of this file")));
    });

    it("warns before replacing, showing the Fingerprint being replaced, and changes nothing yet", async () => {
      const device = await startImport();
      await toConfirmation(device);
      expect(device.api.checkIdentityImport).toHaveBeenCalledWith("/home/me/old-laptop.bhid", PASSWORD);
      const warning = screen.getByRole("alertdialog", { name: "Replace this Device's identity?" });
      // The two Fingerprints, so the user can see what is replaced by what.
      expect(warning.textContent).toContain("AAAA-AAAA becomes BBBB-BBBB");
      expect(warning.textContent).toContain("must not keep running with this identity");
      expect(warning.textContent).toContain("BhayanakShare will restart");
      expect(warning.textContent).toContain("Contacts and Transfer History stay");
      // The warning is what the dialog is described by, and the safe answer has focus.
      const body = document.getElementById(warning.getAttribute("aria-describedby") ?? "");
      expect(body?.textContent).toContain("must not keep running");
      // Focus is moved by an effect, which can run after the dialog is found on a slow machine.
      await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("button", { name: "Cancel" })));
      expect(device.api.importIdentity).not.toHaveBeenCalled();
    });

    it("says how many Transfers in progress will stop, and won't resume, when there are some", async () => {
      const device = await startImport(fakeApi({ transfersInProgress: vi.fn(() => Promise.resolve(2)) }));
      await toConfirmation(device);
      const warning = screen.getByRole("alertdialog", { name: "Replace this Device's identity?" });
      expect(warning.textContent).toContain("2 Transfers are in progress. They will stop and won't resume.");
      // Part of what the dialog is described by, like the rest of the warning.
      const body = document.getElementById(warning.getAttribute("aria-describedby") ?? "");
      expect(body?.textContent).toContain("2 Transfers are in progress");
    });

    it("has the singular form for one Transfer", async () => {
      const device = await startImport(fakeApi({ transfersInProgress: vi.fn(() => Promise.resolve(1)) }));
      await toConfirmation(device);
      expect(screen.getByRole("alertdialog").textContent).toContain(
        "A Transfer is in progress. It will stop and won't resume.",
      );
    });

    it("says nothing about Transfers when none are in progress", async () => {
      const device = await startImport();
      await toConfirmation(device);
      expect(device.api.transfersInProgress).toHaveBeenCalled();
      expect(screen.getByRole("alertdialog").textContent).not.toMatch(/in progress/);
    });

    it("replaces the identity only when confirmed, and says it is restarting", async () => {
      const device = await startImport();
      await toConfirmation(device);
      fireEvent.click(screen.getByRole("button", { name: "Replace and restart" }));
      await waitFor(() =>
        expect(device.api.importIdentity).toHaveBeenCalledWith("/home/me/old-laptop.bhid", PASSWORD),
      );
      expect((await findStatus()).textContent).toBe("Restarting…");
      expect((screen.getByRole("button", { name: "Replace and restart" }) as HTMLButtonElement).disabled).toBe(true);
    });

    it("leaves the identity alone when cancelled at the warning", async () => {
      const device = await startImport();
      await toConfirmation(device);
      fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
      expect(screen.queryByRole("alertdialog")).toBeNull();
      expect(device.api.importIdentity).not.toHaveBeenCalled();
      expect(document.activeElement).toBe(screen.getByRole("button", { name: "Import identity…" }));
    });

    it("shows a wrong password in the dialog and goes no further", async () => {
      const device = await startImport(
        fakeApi({ checkIdentityImport: vi.fn(() => fail("wrong_password")) }),
      );
      typeInto("Password of this file", "nope");
      fireEvent.click(screen.getByRole("button", { name: "Continue" }));
      expect((await screen.findByRole("alert")).textContent).toBe("Wrong password, or the file is damaged.");
      expect(screen.queryByRole("alertdialog")).toBeNull();
      expect(device.api.importIdentity).not.toHaveBeenCalled();
      // Back in the field, to try again.
      await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText("Password of this file")));
    });

    it("says when the file is not an identity file", async () => {
      await startImport(fakeApi({ checkIdentityImport: vi.fn(() => fail("not_an_identity_file")) }));
      typeInto("Password of this file", PASSWORD);
      fireEvent.click(screen.getByRole("button", { name: "Continue" }));
      expect((await screen.findByRole("alert")).textContent).toBe("That is not a BhayanakShare identity file.");
    });

    it("says when the secret store cannot take the key, at the warning", async () => {
      const device = await startImport(
        fakeApi({ importIdentity: vi.fn(() => fail("store_unavailable", "the keychain is locked")) }),
      );
      await toConfirmation(device);
      fireEvent.click(screen.getByRole("button", { name: "Replace and restart" }));
      expect((await screen.findByRole("alert")).textContent).toBe(
        "The secret key could not be stored. the keychain is locked",
      );
      expect(screen.queryByText("Restarting…")).toBeNull();
      // Nothing was replaced, so the user may try again or cancel.
      expect((screen.getByRole("button", { name: "Replace and restart" }) as HTMLButtonElement).disabled).toBe(false);
    });

    it("says so when replacing the key failed half-way and the identity may have changed", async () => {
      const device = await startImport(
        fakeApi({ importIdentity: vi.fn(() => fail("replace_uncertain")) }),
      );
      await toConfirmation(device);
      fireEvent.click(screen.getByRole("button", { name: "Replace and restart" }));
      expect((await screen.findByRole("alert")).textContent).toContain("identity may have changed");
      expect(screen.queryByText("Restarting…")).toBeNull();
    });

    it("does not warn about replacing when the file holds this Device's own identity", async () => {
      const device = await startImport(
        fakeApi({ checkIdentityImport: vi.fn(() => Promise.resolve({ id: MY_ID, fingerprint: "AAAA-AAAA" })) }),
      );
      typeInto("Password of this file", PASSWORD);
      fireEvent.click(screen.getByRole("button", { name: "Continue" }));
      expect((await findStatus()).textContent).toContain("Nothing needs to change");
      expect(screen.queryByRole("alertdialog")).toBeNull();
      expect(device.api.importIdentity).not.toHaveBeenCalled();
    });
  });
});
