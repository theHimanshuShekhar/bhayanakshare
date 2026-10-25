// The error-message catalogue is complete: every Transfer state, failure reason and warning the UI
// can show maps to a message of the translation layer, and none of them is worded by the catch-all
// (C12) where it has a sentence of its own. The maps in the source are `Record<Kind, MessageKey>`,
// so a kind added in the core or the shell fails the typecheck first; these tests check what the
// typecheck cannot: that the keys exist, that the sentences are whole, and that the reasons the core
// gives are the ones the UI words.

import { describe, expect, it } from "vitest";
import type {
  IdentityErrorKind,
  Role,
  SaveFolderErrorKind,
  TransferState,
  UnavailableReason,
  UpdateError,
} from "./bindings";
import {
  EXACT_FAILURES,
  PREFIXED_FAILURES,
  SUFFIXED_FAILURES,
  UNEXPECTED_FAILURE,
  failureMessage,
  failureMessageKey,
} from "./failures";
import { FAILURE_REASONS, FAILURE_REASON_PREFIXES, FAILURE_REASON_SUFFIXES } from "./failureReasons";
import { hasMessage, t, type MessageKey } from "./i18n";
import { identityFailure, IDENTITY_ERRORS } from "./identity";
import { DISCOVERY_MESSAGES, NEARBY_HINTS, discoveryHint } from "./nearby";
import { SAVE_FOLDER_ERRORS, saveFolderFailure } from "./saveFolder";
import {
  CANCELLED_MESSAGES,
  OFFER_WARNINGS,
  STATE_MESSAGES,
  adjustedNamesText,
  skippedLinksText,
  statusText,
} from "./transfers";
import { UPDATE_ERRORS, updateErrorMessage } from "./updates";

const ROLES: Role[] = ["sender", "receiver"];

/** A sentence is whole when every `{name}` in it was filled in. */
function expectWhole(sentence: string) {
  expect(sentence).not.toMatch(/\{\w+\}/);
  expect(sentence.trim()).not.toBe("");
}

/** Every kind of Transfer state. A kind added in the core fails the typecheck of this table. */
const STATE_KINDS: Record<TransferState["kind"], true> = {
  offered: true,
  accepted: true,
  declined: true,
  waiting: true,
  transferring: true,
  reconnecting: true,
  saving: true,
  completed: true,
  failed: true,
  expired: true,
  cancelled: true,
};

function stateOf(kind: TransferState["kind"], role: Role): TransferState {
  switch (kind) {
    case "completed":
      return { kind, saved_to: null };
    case "failed":
      return { kind, reason: FAILURE_REASONS.lost };
    case "cancelled":
      return { kind, by: role };
    default:
      return { kind };
  }
}

describe("Transfer states", () => {
  it("words every state for each role, preparing included, with a message that exists", () => {
    for (const role of ROLES) {
      for (const [state, key] of Object.entries(STATE_MESSAGES[role])) {
        expect(hasMessage(key), `${role} ${state}: ${key}`).toBe(true);
      }
      const wordedKinds = Object.keys(STATE_MESSAGES[role]).filter((s) => s !== "preparing");
      const shown = Object.keys(STATE_KINDS).filter((k) => k !== "failed" && k !== "cancelled");
      expect(wordedKinds.sort()).toEqual(shown.sort());
    }
  });

  it("says every state, as a whole sentence, and none of them with the catch-all", () => {
    for (const role of ROLES) {
      for (const kind of Object.keys(STATE_KINDS) as TransferState["kind"][]) {
        const sentence = statusText({ role, state: stateOf(kind, role) }, "Mum");
        expectWhole(sentence);
        expect(sentence, `${role} ${kind}`).not.toBe(failureMessage("?", role, "Mum"));
      }
      expectWhole(statusText({ role, state: { kind: "accepted" } }, "Mum", true));
    }
  });

  it("says who cancelled, in each direction, in words of their own", () => {
    for (const key of Object.values(CANCELLED_MESSAGES)) expect(hasMessage(key)).toBe(true);
    for (const role of ROLES) {
      const other: Role = role === "sender" ? "receiver" : "sender";
      const you = statusText({ role, state: { kind: "cancelled", by: role } }, "Mum");
      const them = statusText({ role, state: { kind: "cancelled", by: other } }, "Mum");
      expect(you).toBe("You cancelled.");
      expect(them).toBe("Mum cancelled.");
    }
  });

  it("names the other Device in the states that are about it, and only there", () => {
    const sentence = (role: Role, kind: TransferState["kind"]) =>
      statusText({ role, state: stateOf(kind, role) }, "Mum");
    expect(sentence("sender", "offered")).toContain("Mum");
    expect(sentence("sender", "declined")).toContain("Mum");
    expect(sentence("receiver", "declined")).toBe("You declined.");
    expect(sentence("receiver", "offered")).toBe("Waiting for your answer.");
  });
});

describe("failure reasons", () => {
  /** What the core gives as a reason, one of each, in the shape it is stored in. */
  const reasons: { id: string; reason: string }[] = [
    ...Object.entries(FAILURE_REASONS).map(([id, reason]) => ({ id, reason: reason as string })),
    ...Object.entries(FAILURE_REASON_PREFIXES).map(([id, text]) => ({ id, reason: `${text}photo.jpg` })),
    ...Object.entries(FAILURE_REASON_SUFFIXES).map(([id, text]) => ({ id, reason: `Alice's Laptop${text}` })),
  ];

  /**
   * The reasons that are the catch-all's (C12) on purpose: the other Device broke a rule of the
   * protocol, or something failed inside that the user cannot act on. Anything else needs a
   * sentence of its own, so a reason added to the core cannot quietly end up here.
   */
  const CATCH_ALL = [
    "unexpected",
    "bad_offer",
    "wrong_file",
    "too_much",
    "cant_record",
    "cant_prepare_space",
    "cant_look_up",
    "cant_check_files",
    "cant_read_files",
    "cant_prepare_files",
  ];

  it("maps every reason of the core to a message that exists", () => {
    const messages = [
      ...Object.values(EXACT_FAILURES),
      ...Object.values(PREFIXED_FAILURES),
      ...Object.values(SUFFIXED_FAILURES),
    ].flatMap((m) => (typeof m === "string" ? [m] : Object.values(m)));
    expect(messages.length).toBeGreaterThanOrEqual(reasons.length);
    for (const key of messages) expect(hasMessage(key), key).toBe(true);
    // The core's list is all there is: the maps have an entry for each, and nothing more.
    expect(Object.keys(EXACT_FAILURES).sort()).toEqual(Object.keys(FAILURE_REASONS).sort());
    expect(Object.keys(PREFIXED_FAILURES).sort()).toEqual(Object.keys(FAILURE_REASON_PREFIXES).sort());
    expect(Object.keys(SUFFIXED_FAILURES).sort()).toEqual(Object.keys(FAILURE_REASON_SUFFIXES).sort());
  });

  it("gives every reason a sentence of its own, except the few that are the catch-all", () => {
    for (const role of ROLES) {
      for (const { id, reason } of reasons) {
        const { key } = failureMessageKey(reason, role);
        expect(hasMessage(key), id).toBe(true);
        expect(key === UNEXPECTED_FAILURE, `${role} ${id} -> ${key}`).toBe(CATCH_ALL.includes(id));
        expectWhole(failureMessage(reason, role, "Mum"));
      }
    }
  });

  it("puts the file's name in the sentence of a file that changed", () => {
    const reason = `${FAILURE_REASON_PREFIXES.file_changed}docs/plan.txt`;
    expect(failureMessage(reason, "sender", "Mum")).toBe(
      "A file on the sending Device changed or went missing: docs/plan.txt; send the Transfer again.",
    );
  });

  it("words a stall by the side that shows it, and the others the same for both", () => {
    const stalled = FAILURE_REASONS.stalled;
    expect(failureMessage(stalled, "sender", "Mum")).toContain("send it again");
    expect(failureMessage(stalled, "receiver", "Mum")).toContain("partly received files were deleted");
    expect(failureMessage(stalled, "receiver", "Mum")).toContain("ask Mum to send it again");
    for (const { id, reason } of reasons.filter(({ id }) => id !== "stalled")) {
      expect(failureMessage(reason, "sender", "Mum"), id).toBe(failureMessage(reason, "receiver", "Mum"));
    }
  });

  it("words a refusal for the version as the notice on Home does, naming the Device as its tile does", () => {
    const older = `Alice is running an older BhayanakShare. Ask them to update.`;
    expect(failureMessage(older, "sender", "Alice · K3QF-7XNA")).toBe(
      "Alice · K3QF-7XNA is running an older BhayanakShare; ask them to update.",
    );
    const newer = `Alice is running a newer BhayanakShare. Update this Device, then try again.`;
    expect(failureMessage(newer, "sender", "Alice · K3QF-7XNA")).toBe(
      "This Device is running an older BhayanakShare than Alice · K3QF-7XNA; update it to send and receive with them.",
    );
  });

  it("shows any other reason in brackets of the catch-all, without a doubled full stop", () => {
    for (const reason of ["Disk on fire.", "", "Disk on fire"]) {
      expect(failureMessageKey(reason, "sender").key).toBe(UNEXPECTED_FAILURE);
    }
    expect(failureMessage("Disk on fire.", "receiver", "Mum")).toBe(
      "The Transfer stopped because of an unexpected error: Disk on fire; if it keeps happening, export diagnostics in Settings.",
    );
  });

  it("words a reason as it was stored before, in History", () => {
    // The texts of older versions are the same as today's: the core never rewords one.
    expect(failureMessage("Couldn't be sent: invalid file names", "sender", "Mum")).toBe(
      "Mum refused the Offer because some file or folder names are not valid; rename them and send again.",
    );
    expect(failureMessage("The other Device went away.", "receiver", "Mum")).toBe(
      "The connection to Mum stopped before the files were ready, and a Transfer cannot resume from that point; send it again.",
    );
  });
});

describe("warnings", () => {
  const messageKeys = (keys: readonly MessageKey[]) => keys.forEach((k) => expect(hasMessage(k), k).toBe(true));

  it("words the Offer's warnings (E1 to E4), singular and plural", () => {
    for (const keys of Object.values(OFFER_WARNINGS)) messageKeys(keys);
    expectWhole(t(OFFER_WARNINGS.no_room[0], { needed: "2 KiB", free: "512 B" }));
    expectWhole(t(OFFER_WARNINGS.paths_too_long[0]));
    for (const count of [1, 2]) {
      expectWhole(adjustedNamesText(count));
      expectWhole(adjustedNamesText(count, true));
      expectWhole(skippedLinksText(count));
    }
    expect(adjustedNamesText(1)).not.toBe(adjustedNamesText(2));
    expect(adjustedNamesText(1)).not.toBe(adjustedNamesText(1, true));
  });

  it("words why Accept is off (E5) and the hints on Home (H6, H7)", () => {
    messageKeys([...Object.values(NEARBY_HINTS), "offer.acceptBlocked"]);
    expect(t("offer.acceptBlocked")).toBe("Accept is not available until the problems above are solved.");
  });

  it("words every reason local discovery is down (H3 to H5), for a Visible and for a Hidden Device", () => {
    const reasons = Object.keys(DISCOVERY_MESSAGES) as UnavailableReason[];
    expect(reasons.sort()).toEqual(["no_interface", "other", "port_in_use"]);
    const said = new Set<string>();
    for (const reason of reasons) {
      for (const hidden of [false, true]) {
        const hint = discoveryHint({ state: "unavailable", reason }, hidden);
        expect(hint, `${reason} ${hidden}`).not.toBeNull();
        expect(hasMessage(hint!.message)).toBe(true);
        expectWhole(t(hint!.message));
        said.add(t(hint!.message));
      }
    }
    // One sentence for each cause, for each Visibility.
    expect(said.size).toBe(6);
    expect(discoveryHint({ state: "working" }, false)).toBeNull();
  });

  it("words every reason a folder is refused (F1 to F7)", () => {
    const kinds = Object.keys(SAVE_FOLDER_ERRORS) as SaveFolderErrorKind[];
    expect(kinds.sort()).toEqual(["cannot_create", "not_a_folder", "not_absolute", "not_text", "not_writable", "other"]);
    const said = new Set<string>();
    for (const kind of kinds) {
      expect(hasMessage(SAVE_FOLDER_ERRORS[kind])).toBe(true);
      const sentence = saveFolderFailure({ kind, message: "the Device said so" });
      expectWhole(sentence);
      said.add(sentence);
    }
    expect(said.size).toBe(kinds.length);
    expect(saveFolderFailure({ kind: "other", message: "the Device said so" })).toContain("(the Device said so)");
  });

  it("words every reason an update was not installed (G7 to G14), and a check that failed", () => {
    const kinds = Object.keys(UPDATE_ERRORS) as UpdateError[];
    const said = new Set<string>();
    for (const kind of kinds) {
      expect(hasMessage(UPDATE_ERRORS[kind])).toBe(true);
      expectWhole(updateErrorMessage(kind));
      said.add(updateErrorMessage(kind));
    }
    expect(said.size).toBe(kinds.length);
    // Anything that is not a kind is a failed install, not the raw text.
    expect(updateErrorMessage("No such file /home/me/x.AppImage")).toBe(updateErrorMessage("install_failed"));
  });

  it("words every kind of identity failure (J3 to J13)", () => {
    const kinds = Object.keys(IDENTITY_ERRORS) as IdentityErrorKind[];
    const said = new Set<string>();
    for (const kind of kinds) {
      expect(hasMessage(IDENTITY_ERRORS[kind])).toBe(true);
      const sentence = identityFailure({ kind, message: "the Device said so" });
      expectWhole(sentence);
      said.add(sentence);
    }
    expect(said.size).toBe(kinds.length);
  });
});
