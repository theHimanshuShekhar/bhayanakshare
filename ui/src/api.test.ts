import { beforeEach, describe, expect, it, vi } from "vitest";

const plugin = vi.hoisted(() => ({
  getCurrent: vi.fn(),
  onOpenUrl: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-deep-link", () => plugin);

const LINK = `bhayanakshare://add/${"A".repeat(52)}`;

/** A fresh copy of the module, as a freshly loaded page has. */
async function load() {
  vi.resetModules();
  return (await import("./api")).tauriApi;
}

/** Lets the startup link, which arrives on a promise, be delivered. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

beforeEach(() => {
  plugin.getCurrent.mockReset().mockResolvedValue([LINK]);
  plugin.onOpenUrl.mockReset().mockResolvedValue(() => {});
});

describe("onOpenLink", () => {
  it("hands over the link that started the app, and every link opened after", async () => {
    const api = await load();
    const seen = vi.fn();
    await api.onOpenLink(seen);
    await settle();
    expect(seen).toHaveBeenCalledExactlyOnceWith(LINK);

    plugin.onOpenUrl.mock.calls[0][0]([`${LINK}?name=A`, `${LINK}?name=B`]);
    expect(seen.mock.calls.map(([url]) => url)).toEqual([LINK, `${LINK}?name=A`, `${LINK}?name=B`]);
  });

  it("hands over the link that started the app once per page, however often it is listened for", async () => {
    const api = await load();
    const first = vi.fn();
    const stop = await api.onOpenLink(first);
    await settle();
    stop();

    // The webview reloads, or the UI mounts again: the dismissed link is not opened again.
    const second = vi.fn();
    await api.onOpenLink(second);
    await settle();
    expect(first).toHaveBeenCalledTimes(1);
    expect(second).not.toHaveBeenCalled();
    expect(plugin.getCurrent).toHaveBeenCalledTimes(1);
  });

  it("still hands over the startup link to a second listener when the first stopped before it arrived", async () => {
    const api = await load();
    let arrive: (urls: string[]) => void = () => {};
    plugin.getCurrent.mockReturnValueOnce(new Promise((resolve) => (arrive = resolve)));
    const first = vi.fn();
    const stop = await api.onOpenLink(first);
    stop(); // as a development build's second mount does, before the link has come
    const second = vi.fn();
    await api.onOpenLink(second);
    arrive([LINK]); // the first answer comes late, to a listener that has gone
    await settle();
    expect(first).not.toHaveBeenCalled();
    expect(second).toHaveBeenCalledExactlyOnceWith(LINK);
  });

  it("stops handing over links once unsubscribed", async () => {
    const unlisten = vi.fn();
    plugin.onOpenUrl.mockResolvedValue(unlisten);
    const api = await load();
    const seen = vi.fn();
    const stop = await api.onOpenLink(seen);
    await settle();
    seen.mockClear();

    stop();
    expect(unlisten).toHaveBeenCalledTimes(1);
    plugin.onOpenUrl.mock.calls[0][0]([LINK]);
    expect(seen).not.toHaveBeenCalled();
  });

  it("copes with no startup link, and with the plugin failing to say", async () => {
    const api = await load();
    plugin.getCurrent.mockResolvedValueOnce(null);
    await api.onOpenLink(vi.fn());
    await settle();

    const other = await load();
    plugin.getCurrent.mockRejectedValueOnce(new Error("no plugin"));
    await expect(other.onOpenLink(vi.fn())).resolves.toBeTypeOf("function");
    await settle();
  });
});
