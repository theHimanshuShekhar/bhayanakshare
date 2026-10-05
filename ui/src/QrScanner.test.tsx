import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { QrScanner } from "./QrScanner";

const read = vi.hoisted(() => vi.fn());
vi.mock("jsqr", () => ({ default: read }));

const stop = vi.fn();
const getUserMedia = vi.fn();

/** A camera whose frames are 1280 by 720, as the video element reports them. */
beforeEach(() => {
  vi.useFakeTimers();
  stop.mockReset();
  read.mockReset().mockReturnValue(null);
  getUserMedia.mockReset().mockResolvedValue({ getTracks: () => [{ stop }] });
  Object.defineProperty(navigator, "mediaDevices", { value: { getUserMedia }, configurable: true });
  Object.defineProperty(HTMLVideoElement.prototype, "videoWidth", { value: 1280, configurable: true });
  Object.defineProperty(HTMLVideoElement.prototype, "videoHeight", { value: 720, configurable: true });
  HTMLMediaElement.prototype.play = vi.fn(() => Promise.resolve());
  HTMLCanvasElement.prototype.getContext = vi.fn(() => ({
    drawImage: vi.fn(),
    getImageData: vi.fn((_x: number, _y: number, w: number, h: number) => ({
      data: new Uint8ClampedArray(w * h * 4),
      width: w,
      height: h,
    })),
  })) as never;
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

/** Lets the camera open, then lets time pass. */
async function opened(ms = 0) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

describe("QrScanner", () => {
  it("asks for the camera and hands over what a code says, from a frame no wider than it needs", async () => {
    const seen = vi.fn();
    render(<QrScanner onDecoded={seen} />);
    expect(getUserMedia).toHaveBeenCalledWith({ video: { facingMode: "environment" } });

    await opened(200);
    expect(seen).not.toHaveBeenCalled(); // a frame with no code in it says nothing
    expect(read.mock.calls[0].slice(1, 3)).toEqual([640, 360]);

    read.mockReturnValue({ data: "bhayanakshare://add/XYZ" });
    await opened(200);
    expect(seen).toHaveBeenCalledWith("bhayanakshare://add/XYZ");
  });

  it("gives the text of a code every time it reads one, and ignores an empty one", async () => {
    const seen = vi.fn();
    render(<QrScanner onDecoded={seen} />);
    await opened();
    read.mockReturnValue({ data: "" });
    await opened(400);
    expect(seen).not.toHaveBeenCalled();
    read.mockReturnValue({ data: "a" });
    await opened(400);
    expect(seen).toHaveBeenCalledTimes(2);
  });

  it("lets go of the camera when it goes away", async () => {
    const { unmount } = render(<QrScanner onDecoded={() => {}} />);
    await opened();
    expect(stop).not.toHaveBeenCalled();
    unmount();
    expect(stop).toHaveBeenCalledTimes(1);

    read.mockReturnValue({ data: "late" });
    await opened(1000); // no longer looking
    expect(read).not.toHaveBeenCalled();
  });

  it("lets go of a camera that opened after the scanner was gone", async () => {
    let open: (stream: unknown) => void = () => {};
    getUserMedia.mockReturnValue(new Promise((resolve) => (open = resolve)));
    const { unmount } = render(<QrScanner onDecoded={() => {}} />);
    unmount();
    await act(async () => open({ getTracks: () => [{ stop }] }));
    expect(stop).toHaveBeenCalledTimes(1);
  });

  it("says so when the user does not allow the camera", async () => {
    getUserMedia.mockRejectedValue(new DOMException("denied", "NotAllowedError"));
    render(<QrScanner onDecoded={() => {}} />);
    await opened();
    expect(screen.getByRole("alert").textContent).toContain("not allowed to use the camera");
  });

  it("says so when there is no camera", async () => {
    getUserMedia.mockRejectedValue(new DOMException("none", "NotFoundError"));
    render(<QrScanner onDecoded={() => {}} />);
    await opened();
    expect(screen.getByRole("alert").textContent).toContain("No camera could be opened");
  });

  it("says so when the webview has no camera support at all", async () => {
    Object.defineProperty(navigator, "mediaDevices", { value: undefined, configurable: true });
    render(<QrScanner onDecoded={() => {}} />);
    expect(screen.getByRole("alert").textContent).toContain("No camera could be opened");
  });
});
