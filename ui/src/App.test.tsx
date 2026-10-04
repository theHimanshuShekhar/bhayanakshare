import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import type { Api, DeviceEvent } from "./api";
import type { TransferState } from "./bindings";

afterEach(cleanup);

const MY_ID = "A".repeat(52);
const PEER_ID = "K3QF7XNA" + "B".repeat(44);
const TRANSFER = "ab".repeat(16);

/** A Device event without the stream position and time the stand-in fills in. */
type Unstamped = DeviceEvent extends infer E
  ? E extends unknown
    ? Omit<E, "seq" | "at"> & { at?: number }
    : never
  : never;

/** A stand-in for the Rust shell: records commands, and lets a test push Device events. */
function fakeApi(overrides: Partial<Api> = {}) {
  let handler: (event: DeviceEvent) => void = () => {};
  let seq = 0;
  const api = {
    myId: () => Promise.resolve({ id: MY_ID, fingerprint: "AAAA-AAAA" }),
    saveFolder: () => Promise.resolve("/home/me/Downloads/BhayanakShare"),
    sendFile: vi.fn(() => Promise.resolve(TRANSFER)),
    acceptOffer: vi.fn(() => Promise.resolve(null)),
    declineOffer: vi.fn(() => Promise.resolve(null)),
    pickFile: vi.fn(() => Promise.resolve<string | null>("/tmp/photo.jpg")),
    showInFolder: vi.fn(() => Promise.resolve()),
    copyText: vi.fn(() => Promise.resolve()),
    onDeviceEvent: (h: (event: DeviceEvent) => void) => {
      handler = h;
      return Promise.resolve(() => {});
    },
    ...overrides,
  } satisfies Api;
  const push = (event: Unstamped) =>
    act(() => handler({ seq: seq++, at: 1_000 * seq, ...event } as DeviceEvent));
  const transfer = (role: "sender" | "receiver", state: TransferState) =>
    push({ type: "transfer", transfer_id: TRANSFER, role, peer: PEER_ID, name: "photo.jpg", size: 2048, state });
  return { api, push, transfer };
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
  fireEvent.click(screen.getByRole("button", { name: "Choose file…" }));
  await waitFor(() => expect(device.api.sendFile).toHaveBeenCalled());
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

    expect(device.api.pickFile).toHaveBeenCalled();
    expect(device.api.sendFile).toHaveBeenCalledWith(PEER_ID, "/tmp/photo.jpg");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    await device.transfer("sender", { kind: "offered" });
    expect(screen.getByText("photo.jpg to K3QF-7XNA")).toBeTruthy();
    expect(screen.getByText("Waiting for K3QF-7XNA…")).toBeTruthy();
  });

  it("sends nothing when the file picker is cancelled", async () => {
    const device = await start(fakeApi({ pickFile: vi.fn(() => Promise.resolve(null)) }));
    fireEvent.click(screen.getByRole("button", { name: "Send to ID…" }));
    fireEvent.change(screen.getByLabelText("Device ID"), { target: { value: PEER_ID } });
    fireEvent.click(screen.getByRole("button", { name: "Choose file…" }));
    await waitFor(() => expect(device.api.pickFile).toHaveBeenCalled());
    expect(device.api.sendFile).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog")).toBeTruthy();
  });

  it("keeps the dialog open and says why when the Device refuses to send", async () => {
    const device = await start(
      fakeApi({ sendFile: vi.fn(() => Promise.reject("a Device ID is 52 characters of base32")) }),
    );
    await sendPhoto(device);
    expect((await screen.findByRole("alert")).textContent).toContain("Could not send the file");
    expect(screen.getByRole("alert").textContent).toContain("52 characters");
    expect(screen.getByRole("dialog")).toBeTruthy();
  });

  it("needs an ID before it will pick a file", async () => {
    await start();
    fireEvent.click(screen.getByRole("button", { name: "Send to ID…" }));
    expect((screen.getByRole("button", { name: "Choose file…" }) as HTMLButtonElement).disabled).toBe(true);
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
      name: "b.bin",
      size: 1,
      state: { kind: "failed", reason: "The other Device went away." },
    });
    expect(screen.getByText("Could not send. The other Device went away.")).toBeTruthy();
  });
});

describe("receiving", () => {
  it("shows the Offer with the Sender's Fingerprint, name, size and save folder", async () => {
    const device = await start();
    await device.transfer("receiver", { kind: "offered" });

    const sheet = await screen.findByRole("dialog", { name: "Incoming file" });
    expect(sheet.textContent).toContain("K3QF-7XNA");
    expect(sheet.textContent).toContain("photo.jpg");
    expect(sheet.textContent).toContain("2 KiB");
    await waitFor(() => expect(sheet.textContent).toContain("/home/me/Downloads/BhayanakShare"));
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
    expect(device.api.acceptOffer).toHaveBeenCalledWith(TRANSFER);

    await device.transfer("receiver", { kind: "accepted" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByText("Accepted. Starting…")).toBeTruthy();
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
    expect((await screen.findByRole("alert")).textContent).toContain("no longer waiting");
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
