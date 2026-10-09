// TEMPORARY (issue #72): measures where the time goes on the Windows runner. Removed before the end.
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { afterEach, beforeAll, expect, it, vi } from "vitest";
import { App } from "./App";
import { PEER_ID, contact, fakeApi } from "./testApi";
vi.mock("./QrScanner", () => ({ QrScanner: () => <p>Camera</p> }));
afterEach(cleanup);
beforeAll(() => {
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
  Object.defineProperty(window, "CSS", { configurable: true, value: { escape: (s: string) => s } });
});

it("profile", async () => {
  const log: string[] = [];
  let timers = 0;
  const orig = globalThis.setTimeout;
  // @ts-ignore counting only
  globalThis.setTimeout = (...a: any[]) => { timers++; return (orig as any)(...a); };
  const lap = (l: string, t0: number) => { log.push(`PROF ${l}: ${(performance.now() - t0).toFixed(0)}ms timers=${timers}`); timers = 0; };
  let t0 = performance.now();
  for (let i = 0; i < 50; i++) await new Promise((r) => orig(r, 0));
  lap("50 x setTimeout(0), one after another", t0);
  t0 = performance.now();
  const device = fakeApi({}, [contact({ id: PEER_ID, nickname: "Mum" })]);
  render(<App api={device.api} />);
  await screen.findByRole("heading", { name: /^(My ID|Welcome to BhayanakShare)$/ });
  lap("render + findBy My ID", t0);
  for (const [name, el] of [["body", document.body], ["main", document.querySelector("main")!]] as const) {
    t0 = performance.now();
    await axe.run(el as HTMLElement, { rules: { "color-contrast": { enabled: false } } });
    lap(`axe.run ${name}`, t0);
  }
  const user = userEvent.setup();
  t0 = performance.now();
  for (let i = 0; i < 24; i++) await user.tab();
  lap("24 x user.tab(), delay 0", t0);
  const user2 = userEvent.setup({ delay: null });
  t0 = performance.now();
  for (let i = 0; i < 24; i++) await user2.tab();
  lap("24 x user.tab(), delay null", t0);
  globalThis.setTimeout = orig;
  console.error(log.join("\n"));
  expect(log.length).toBeGreaterThan(0);
});
