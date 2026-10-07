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

  it("announces an Offer when it arrives, and says it is not an alert", async () => {
    const device = await start();
    await device.transfer("receiver", { kind: "offered" });
    expect(said()).toEqual(["photo.jpg from Mum: Waiting for your answer."]);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("announces accepted, receiving and received as they happen", async () => {
    const device = await start();
    await device.transfer("receiver", { kind: "offered" });
    await device.transfer("receiver", { kind: "accepted" });
    await device.transfer("receiver", { kind: "transferring" });
    await device.transfer("receiver", { kind: "completed", saved_to: "/home/me/photo.jpg" });
    expect(said()).toEqual([
      "photo.jpg from Mum: Waiting for your answer.",
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

  it("does not read a Batch out Receiver by Receiver, and says how it ended", async () => {
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
    await member(2, DAD, { kind: "offered" });
    await member(1, PEER_ID, { kind: "completed", saved_to: null });
    expect(said()).toEqual([]);
    await member(2, DAD, { kind: "failed", reason: "Gone." });
    expect(said()).toEqual(["photo.jpg to 2 Devices: 1 of 2 delivered, 1 failed"]);
  });

  it("goes by a Contact's name", async () => {
    const device = await start(fakeApi({}, [contact({ id: PEER_ID, nickname: "Mum" })]));
    await device.transfer("sender", { kind: "offered" });
    expect(said()).toEqual(["photo.jpg to Mum: Waiting for Mum…"]);
  });

  it("takes its words back after a while, so that it does not pile up", async () => {
    const device = await start();
    vi.useFakeTimers({ toFake: ["setTimeout"] });
    await device.transfer("receiver", { kind: "offered" });
    expect(said()).toHaveLength(1);
    act(() => vi.advanceTimersByTime(ANNOUNCEMENT_MS));
    expect(said()).toEqual([]);
  });

  it("says the same words twice when the same thing happens twice", async () => {
    const device = await start();
    await device.transfer("receiver", { kind: "offered" }, null, { name: "a.txt", items: ["a.txt"] });
    await device.push({
      type: "transfer",
      transfer_id: "22".repeat(16),
      role: "receiver",
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
    expect(said()).toEqual(["a.txt from Mum: Waiting for your answer.", "a.txt from Mum: Waiting for your answer."]);
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

  it("announces a copy that failed", async () => {
    await start(fakeApi({ copyText: () => Promise.reject(new Error("no")) }));
    fireEvent.click(screen.getByRole("button", { name: "Copy ID" }));
    await waitFor(() => expect(said()).toEqual(["Copy failed. Select the text and copy it by hand."]));
  });
});
