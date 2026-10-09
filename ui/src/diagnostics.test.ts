// Nothing here needs a document, so no jsdom: building one costs seconds of CPU on the Windows runner.
// @vitest-environment node
import { describe, expect, it } from "vitest";
import { diagnosticsFileName } from "./diagnostics";

describe("diagnosticsFileName", () => {
  it("names the zip for the day, with the month and day in two digits", () => {
    expect(diagnosticsFileName(new Date(2026, 9, 7))).toBe("bhayanakshare-diagnostics-2026-10-07.zip");
    expect(diagnosticsFileName(new Date(2027, 0, 31))).toBe("bhayanakshare-diagnostics-2027-01-31.zip");
  });
});
