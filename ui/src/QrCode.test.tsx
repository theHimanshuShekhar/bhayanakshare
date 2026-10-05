import { cleanup, render, screen } from "@testing-library/react";
import jsQR from "jsqr";
import { afterEach, describe, expect, it } from "vitest";
import { QrCode } from "./QrCode";
import { parseShareLink, shareLink } from "./shareLink";

afterEach(cleanup);

const ID = "K3QF7XNA" + "B".repeat(44);

/** What a camera would see: the drawn code as pixels, `scale` per module, black on white. */
function pixels(svg: Element, scale: number) {
  const side = Number(svg.getAttribute("viewBox")!.split(" ")[2]);
  const width = side * scale;
  const data = new Uint8ClampedArray(width * width * 4).fill(255);
  const path = svg.querySelector("path")!.getAttribute("d")!;
  for (const [, x, y] of path.matchAll(/M(\d+) (\d+)h1v1h-1z/g)) {
    for (let dy = 0; dy < scale; dy++) {
      for (let dx = 0; dx < scale; dx++) {
        const at = ((Number(y) * scale + dy) * width + Number(x) * scale + dx) * 4;
        data.fill(0, at, at + 3);
      }
    }
  }
  return { data, width };
}

describe("QrCode", () => {
  it("draws a code that reads back as the text", () => {
    const texts = [
      shareLink(ID, "Mum's laptop"),
      shareLink(ID, "é🙂".repeat(30)),
      shareLink(ID, "x".repeat(64)),
      shareLink(ID, null),
    ];
    for (const text of texts) {
      render(<QrCode text={text} label="code" />);
      const { data, width } = pixels(screen.getByRole("img", { name: "code" }), 4);
      expect(jsQR(data, width, width)?.data).toBe(text);
      cleanup();
    }
  });

  it("holds a share link that reads back as the Device", () => {
    render(<QrCode text={shareLink(ID, "Alice")} label="code" />);
    const { data, width } = pixels(screen.getByRole("img", { name: "code" }), 4);
    expect(parseShareLink(jsQR(data, width, width)!.data)).toEqual({ id: ID, name: "Alice" });
  });
});
