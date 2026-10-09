// Nothing here needs a document, so no jsdom: building one costs seconds of CPU on the Windows runner.
// @vitest-environment node
import { describe, expect, it } from "vitest";
import { parseIdOrLink, parseShareLink, shareLink } from "./shareLink";

const ID = "K3QF7XNA" + "B".repeat(44);

describe("shareLink", () => {
  it("puts the Device ID in the path and the Device Name in the query", () => {
    expect(shareLink(ID, "Mum's laptop")).toBe(`bhayanakshare://add/${ID}?name=Mum's%20laptop`);
  });

  it("percent-encodes whatever would end the query or the link", () => {
    expect(shareLink(ID, "a&b=c#d?e é 🙂")).toBe(
      `bhayanakshare://add/${ID}?name=a%26b%3Dc%23d%3Fe%20%C3%A9%20%F0%9F%99%82`,
    );
  });

  it("leaves the name out when there is none", () => {
    expect(shareLink(ID, null)).toBe(`bhayanakshare://add/${ID}`);
    expect(shareLink(ID, "")).toBe(`bhayanakshare://add/${ID}`);
  });

  it("parses back to what it was made from", () => {
    for (const name of ["Mum's laptop", "a&b=c#d?e é 🙂", "100% +plus+", "x".repeat(64)]) {
      expect(parseShareLink(shareLink(ID, name))).toEqual({ id: ID, name });
    }
  });
});

describe("parseShareLink", () => {
  const link = (rest: string) => `bhayanakshare://add/${rest}`;

  it("reads the Device ID and the suggested name", () => {
    expect(parseShareLink(link(`${ID}?name=Alice`))).toEqual({ id: ID, name: "Alice" });
    expect(parseShareLink(link(ID))).toEqual({ id: ID, name: null });
  });

  it("gives the Device ID in capitals, however it was written", () => {
    expect(parseShareLink(link(ID.toLowerCase()))?.id).toBe(ID);
  });

  it("ignores case in the scheme and the host, which a system may change", () => {
    expect(parseShareLink(`BhayanakShare://ADD/${ID}`)?.id).toBe(ID);
  });

  it("ignores whitespace around a pasted link", () => {
    expect(parseShareLink(`  \n${link(ID)}\r\n`)?.id).toBe(ID);
  });

  it("accepts a trailing slash and a fragment, and ignores other parameters", () => {
    expect(parseShareLink(link(`${ID}/`))?.id).toBe(ID);
    expect(parseShareLink(link(`${ID}/?name=A`))?.name).toBe("A");
    expect(parseShareLink(link(`${ID}?name=A#top`))).toEqual({ id: ID, name: "A" });
    expect(parseShareLink(link(`${ID}?utm=x&name=A&other=y`))).toEqual({ id: ID, name: "A" });
  });

  it("takes the first name when there are several", () => {
    expect(parseShareLink(link(`${ID}?name=A&name=B`))?.name).toBe("A");
  });

  it("decodes percent-escapes and plus signs in the name", () => {
    expect(parseShareLink(link(`${ID}?name=Mum%27s%20laptop`))?.name).toBe("Mum's laptop");
    expect(parseShareLink(link(`${ID}?name=Mum+and+Dad`))?.name).toBe("Mum and Dad");
    expect(parseShareLink(link(`${ID}?name=%C3%A9%F0%9F%99%82`))?.name).toBe("é🙂");
  });

  it("makes no name of a missing, empty or blank one", () => {
    expect(parseShareLink(link(`${ID}?`))).toEqual({ id: ID, name: null });
    expect(parseShareLink(link(`${ID}?name`))).toEqual({ id: ID, name: null });
    expect(parseShareLink(link(`${ID}?name=`))).toEqual({ id: ID, name: null });
    expect(parseShareLink(link(`${ID}?name=%20%20`))).toEqual({ id: ID, name: null });
    expect(parseShareLink(link(`${ID}?other=1`))).toEqual({ id: ID, name: null });
  });

  it("cleans the name: trimmed, no control characters, cut to 64 characters", () => {
    expect(parseShareLink(link(`${ID}?name=%20%20Alice%20`))?.name).toBe("Alice");
    expect(parseShareLink(link(`${ID}?name=Al%00i%0Ace%1B%7F`))?.name).toBe("Alice");
    expect(parseShareLink(link(`${ID}?name=${"x".repeat(500)}`))?.name).toBe("x".repeat(64));
    expect([...(parseShareLink(link(`${ID}?name=${"%F0%9F%99%82".repeat(100)}`))?.name ?? "")]).toHaveLength(64);
  });

  it("keeps the Device ID when only the name is bad", () => {
    // A percent-escape that is not valid UTF-8, or not an escape at all.
    expect(parseShareLink(link(`${ID}?name=%FF%FE`))).toEqual({ id: ID, name: null });
    expect(parseShareLink(link(`${ID}?name=100%`))).toEqual({ id: ID, name: null });
    expect(parseShareLink(link(`${ID}?name=%zz`))).toEqual({ id: ID, name: null });
  });

  it("refuses anything that is not a share link", () => {
    const bad = [
      "",
      "   ",
      ID, // a bare Device ID is not a link
      "K3QF-7XNA",
      "https://example.com/add/" + ID,
      `bhayanakshare:add/${ID}`, // no //
      `bhayanakshare:/add/${ID}`,
      `bhayanakshare:///add/${ID}`,
      `bhayanakshare://${ID}`, // no host
      `bhayanakshare://send/${ID}`, // wrong host
      `bhayanakshare://add.example/${ID}`,
      `bhayanakshare://add:80/${ID}`,
      `bhayanakshare://user@add/${ID}`,
      `bhayanakshare://add//${ID}`,
      `bhayanakshare://add/x/${ID}`, // extra path
      `bhayanakshare://add/${ID}/x`,
      "bhayanakshare://add/",
      "bhayanakshare://add",
      "bhayanakshare://",
      "bhayanakshare://add/?name=Alice",
      `bhayanakshare://add/${ID}${ID}`,
      `xbhayanakshare://add/${ID}`,
      `${link(ID)} ${link(ID)}`, // two links
      `see ${link(ID)}`, // a link inside a sentence
      `bhayanakshare://add/ ${ID}`, // whitespace inside
      `bhayanakshare://add/${ID.slice(0, 10)} ${ID.slice(10)}`,
    ];
    for (const text of bad) expect(parseShareLink(text), JSON.stringify(text)).toBeNull();
  });

  it("refuses a Device ID of the wrong length or alphabet", () => {
    const bad = [
      ID.slice(1), // 51
      ID + "B", // 53
      "1" + ID.slice(1), // 1 is not base32
      "8" + ID.slice(1),
      "0" + ID.slice(1),
      ID.slice(1) + "=", // padding
      ID.slice(1) + "-",
      "K3QF-7XNA" + "B".repeat(43),
      ID.slice(1) + "é",
      ID.replace("K", "%4B"), // escapes do not apply in the path
    ];
    for (const id of bad) expect(parseShareLink(`bhayanakshare://add/${id}`), id).toBeNull();
  });
});

describe("parseIdOrLink", () => {
  it("takes a bare Device ID, in either case, with no name", () => {
    expect(parseIdOrLink(ID)).toEqual({ id: ID, name: null });
    expect(parseIdOrLink(` ${ID.toLowerCase()}\n`)).toEqual({ id: ID, name: null });
  });

  it("takes a share link", () => {
    expect(parseIdOrLink(`bhayanakshare://add/${ID}?name=Alice`)).toEqual({ id: ID, name: "Alice" });
  });

  it("takes nothing else", () => {
    expect(parseIdOrLink("")).toBeNull();
    expect(parseIdOrLink("K3QF-7XNA")).toBeNull();
    expect(parseIdOrLink(ID.slice(1))).toBeNull();
    expect(parseIdOrLink(`https://example.com/${ID}`)).toBeNull();
    expect(parseIdOrLink(`bhayanakshare://send/${ID}`)).toBeNull();
  });
});
