import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ANNOUNCEMENT_MS } from "./Announcer";
import { App } from "./App";
import { BATCH, PEER_ID, TRANSFER, contact, fakeApi } from "./testApi";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

const DAD = "Q2WERTYU" + "C".repeat(44);

/** The one place the app speaks to a screen reader from: everything said, one line each. */
const announcer = () => document.querySelector<HTMLElement>("[data-announcer]")!;
const said = () => [...announcer().querySelectorAll("p")].map((p) => p.textContent);

async function start(device = fakeApi({}, [contact({ id: PEER_ID, nickname: "Mum" })])) {
  render(<App api={device.api} />);
  await screen.findByRole("heading", { name: "My ID" });
  return device;
}

describe("the announcer", () => {
  it("is the only live region the app puts on Transfers and their rows", async () => {
    const device = await start();
    await device.transfer("sender", { kind: "offered" });
    await device.push({
      type: "transfer",
      transfer_id: "22".repeat(16),
      role: "sender",
      peer: PEER_ID,
      peer_name: null,
      kind: "files",
      text: null,
      name: "a",
      items: ["a"],
      file_count: 1,
      skipped_links: 0,
      adjusted_names: 0,
      batch_id: BATCH,
      size: 1,
      expires_at: 1,
      state: { kind: "offered" },
    });
    // One region, polite and announced as a whole; rows and Batch rows carry none of their own.
    expect(document.querySelectorAll("[aria-live]")).toHaveLength(1);
    expect(announcer().getAttribute("aria-live")).toBe("polite");
    expect(announcer().getAttribute("role")).toBe("log");
    expect(announcer().getAttribute("aria-atomic")).not.toBe("true");
  });

  it("is outside everything the app makes inert, where a screen reader would skip it", async () => {
    const device = await start();
    await device.transfer("receiver", { kind: "offered" });
    expect(announcer().closest("[inert]")).toBeNull();
  });

  it("leaves the Offer that the Offer sheet opens for to the sheet, which takes focus and is read itself", async () => {
    const device = await start();
    await device.transfer("receiver", { kind: "offered" });
    expect(document.activeElement).toBe(screen.getByRole("heading", { name: "Incoming files" }));
    expect(said()).toEqual([]);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("announces an Offer that arrives while another is being answered, as no sheet takes focus for it", async () => {
    const device = await start();
    await device.transfer("receiver", { kind: "offered" });
    await device.push({
      type: "transfer",
      transfer_id: "22".repeat(16),
      role: "receiver",
      peer: PEER_ID,
      peer_name: null,
      kind: "files",
      text: null,
      name: "b.txt",
      items: ["b.txt"],
      file_count: 1,
      skipped_links: 0,
      adjusted_names: 0,
      batch_id: null,
      size: 1,
      expires_at: 1,
      state: { kind: "offered" },
    });
    expect(said()).toEqual(["b.txt from Mum: Waiting for your answer."]);
    expect(document.activeElement).toBe(screen.getByRole("heading", { name: "Incoming files" }));
  });

  it("announces an Offer that arrives while another sheet is up, since the Offer sheet is not what has focus then", async () => {
    // Send to ID is open when the Offer comes: the Offer sheet goes over it and takes focus, so
    // it reads itself. What is said is only about what is not on screen with focus.
    const device = await start();
    fireEvent.click(screen.getByRole("button", { name: "Send to ID…" }));
    await device.transfer("receiver", { kind: "offered" });
    expect(screen.getByRole("dialog", { name: "Incoming files" })).toBeTruthy();
    expect(said()).toEqual([]);
  });

  it("announces accepted, receiving and received as they happen", async () => {
    const device = await start();
    await device.transfer("receiver", { kind: "offered" });
    await device.transfer("receiver", { kind: "accepted" });
    await device.transfer("receiver", { kind: "transferring" });
    await device.transfer("receiver", { kind: "completed", saved_to: "/home/me/photo.jpg" });
    expect(said()).toEqual([
      "photo.jpg from Mum: Accepted. Starting…",
      "photo.jpg from Mum: Receiving…",
      "photo.jpg from Mum: Received.",
    ]);
  });

  it("announces why a Transfer failed, but not as an alert", async () => {
    const device = await start();
    await device.transfer("sender", { kind: "offered" });
    await device.transfer("sender", { kind: "failed", reason: "The connection dropped." });
    expect(said().at(-1)).toBe("photo.jpg to Mum: Could not send. The connection dropped.");
    expect(within(announcer()).queryByRole("alert")).toBeNull();
  });

  it("announces progress at coarse steps, not on every report", async () => {
    const device = await start();
    await device.transfer("receiver", { kind: "transferring" });
    for (let bytes = 0; bytes <= 2048; bytes += 64) {
      await device.push({ type: "progress", transfer_id: TRANSFER, bytes, total: 2048, at: 1_000 + bytes * 10 });
    }
    const steps = said().filter((m) => m?.includes("% of"));
    // 33 reports, over 20 seconds: a few quarters, never one per report.
    expect(steps.length).toBeGreaterThanOrEqual(1);
    expect(steps.length).toBeLessThanOrEqual(3);
    expect(steps[0]).toMatch(/^photo\.jpg from Mum: (25|26|27|28)% of 2 KiB$/);
  });

  it("announces a Batch as it runs: it starting, a Receiver failing at once, and how it ended", async () => {
    const device = await start(
      fakeApi({}, [contact({ id: PEER_ID, nickname: "Mum" }), contact({ id: DAD, nickname: "Dad", added_at: 2 })]),
    );
    const member = (n: number, peer: string, state: Parameters<typeof device.transfer>[1]) =>
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
        size: 1,
        expires_at: 1,
        state,
      });
    await member(1, PEER_ID, { kind: "offered" });
    expect(said()).toEqual(["photo.jpg to 1 Devices: 0 of 1 delivered, 1 in progress"]);
    // The second Receiver follows at once: not said again until ten seconds have passed.
    await member(2, DAD, { kind: "offered" });
    await member(1, PEER_ID, { kind: "transferring" });
    expect(said()).toHaveLength(1);
    await member(2, DAD, { kind: "failed", reason: "Gone." });
    expect(said().at(-1)).toBe("photo.jpg to Dad: Could not send. Gone.");
    await member(1, PEER_ID, { kind: "completed", saved_to: null });
    expect(said().at(-1)).toBe("photo.jpg to 2 Devices: 1 of 2 delivered, 1 failed");
  });

  it("goes by a Contact's name", async () => {
    const device = await start(fakeApi({}, [contact({ id: PEER_ID, nickname: "Mum" })]));
    await device.transfer("sender", { kind: "offered" });
    expect(said()).toEqual(["photo.jpg to Mum: Waiting for Mum…"]);
  });

  it("takes its words back after a while, so that it does not pile up", async () => {
    const device = await start();
    vi.useFakeTimers({ toFake: ["setTimeout"] });
    await device.transfer("sender", { kind: "offered" });
    expect(said()).toHaveLength(1);
    act(() => vi.advanceTimersByTime(ANNOUNCEMENT_MS));
    expect(said()).toEqual([]);
  });

  it("says the same words twice when the same thing happens twice", async () => {
    const device = await start();
    const sent = (n: number) =>
      device.push({
        type: "transfer",
        transfer_id: String(n).repeat(32),
        role: "sender",
        peer: PEER_ID,
        peer_name: null,
        kind: "files",
        text: null,
        name: "a.txt",
        items: ["a.txt"],
        file_count: 1,
        skipped_links: 0,
        adjusted_names: 0,
        batch_id: null,
        size: 1,
        expires_at: 1,
        state: { kind: "offered" },
      });
    await sent(2);
    await sent(3);
    expect(said()).toEqual(["a.txt to Mum: Waiting for Mum…", "a.txt to Mum: Waiting for Mum…"]);
  });
});

describe("the announcer: copying", () => {
  it("announces a copy of My ID or a share link through the same region", async () => {
    const device = await start();
    fireEvent.click(screen.getByRole("button", { name: "Copy ID" }));
    await waitFor(() => expect(said()).toEqual(["Copied to the clipboard."]));
    expect(device.api.copyText).toHaveBeenCalled();
    // And the words next to the button are still there to read.
    expect(screen.getByText("Copied")).toBeTruthy();
    expect(screen.getByText("Copied").getAttribute("role")).toBeNull();
  });

  it("shows a copy that failed as an alert next to the button, not in the log: the user has to act", async () => {
    await start(fakeApi({ copyText: () => Promise.reject(new Error("no")) }));
    fireEvent.click(screen.getByRole("button", { name: "Copy ID" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toBe("Could not copy. Select the text and copy it by hand.");
    expect(said()).toEqual([]);
    expect(within(alert.closest("p")!).getByRole("button", { name: "Copy ID" })).toBeTruthy();
  });

  it("does the same for the copy button of a received text", async () => {
    const device = await start(fakeApi({ copyText: () => Promise.reject(new Error("no")) }));
    await device.transfer("receiver", { kind: "completed", saved_to: null }, null, {
      kind: "text",
      text: "hello",
      name: "",
      items: [],
    });
    fireEvent.click(await screen.findByRole("button", { name: /Copy the text from/ }));
    expect((await screen.findByRole("alert")).textContent).toContain("Could not copy");
    // Only the text's arrival is in the log, nothing about the copy.
    expect(said()).toEqual(["Text from K3QF-7XNA: Received."]);
  });
});
