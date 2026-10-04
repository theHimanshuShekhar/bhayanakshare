import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { App } from "./App";

afterEach(cleanup);

const stubId = () =>
  Promise.resolve({ id: "A".repeat(52), fingerprint: "AAAA-AAAA" });

describe("App", () => {
  it("shows this Device's ID and Fingerprint on Home", async () => {
    render(<App loadMyId={stubId} />);
    expect(await screen.findByText("AAAA-AAAA")).toBeTruthy();
    expect(screen.getByText("A".repeat(52))).toBeTruthy();
  });

  it("reports a Device that failed to start", async () => {
    render(<App loadMyId={() => Promise.reject(new Error("no device"))} />);
    expect((await screen.findByRole("alert")).textContent).toContain("could not start");
  });

  it("switches between the four tabs and marks the current one", async () => {
    render(<App loadMyId={stubId} />);
    const nav = screen.getByRole("navigation", { name: "Main" });
    expect(nav.querySelectorAll("button")).toHaveLength(4);

    fireEvent.click(screen.getByRole("button", { name: "History" }));
    expect(screen.getByRole("button", { name: "History" }).getAttribute("aria-current")).toBe("page");
    expect(screen.getByRole("button", { name: "Home" }).getAttribute("aria-current")).toBeNull();
    expect(screen.getByText("Your Transfer History will appear here.")).toBeTruthy();
  });
});
