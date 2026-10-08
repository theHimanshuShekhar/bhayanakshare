import { act } from "@testing-library/react";
import { vi } from "vitest";
import type { Api, Contact, DeviceEvent, DiscoveryStatus, Role, ShellEvent, UpdateAction, Visibility } from "./api";
import type { HistoryEntry, NearbyDevice, TransferState } from "./bindings";

// The stand-in for the Rust shell that the UI tests share (App.test.tsx, a11y.test.tsx).

export const MY_ID = "A".repeat(52);
/** The Device an identity file in these tests holds. */
const FILE_ID = "B".repeat(52);
export const PEER_ID = "K3QF7XNA" + "B".repeat(44);
export const TRANSFER = "ab".repeat(16);
export const BATCH = "ba".repeat(16);
/** When the Offers in these tests lapse: 10 minutes after the stand-in's clock reads 0. */
export const EXPIRES_AT = 600_000;

/** A Device event without the stream position and time the stand-in fills in. */
export type Unstamped = DeviceEvent extends infer E
  ? E extends unknown
    ? Omit<E, "seq" | "at"> & { at?: number }
    : never
  : never;

export function contact(over: Partial<Contact> = {}): Contact {
  return {
    id: PEER_ID,
    nickname: null,
    device_name: null,
    auto_accept: false,
    last_known_address: { relay_url: null, direct: [] },
    added_at: 1,
    ...over,
  };
}

/** A stand-in for the Rust shell: records commands, and lets a test push Device events. */
export function fakeApi(overrides: Partial<Api> = {}, initialContacts: Contact[] = []) {
  let handler: (event: DeviceEvent) => void = () => {};
  let shellHandler: (event: ShellEvent) => void = () => {};
  let linkHandler: (url: string) => void = () => {};
  let autostart = true;
  let debugLogging = false;
  let publicDht = true;
  let deviceName = "Alice's desktop";
  let saveFolder = "/home/me/Downloads/BhayanakShare";
  let firstRun = false;
  let seq = 0;
  let contacts = initialContacts;
  let visibility: Visibility = "id_holders";
  let discovery: DiscoveryStatus = { state: "working" };
  let historyEntries: HistoryEntry[] = [];
  /** What each read of History asked for: the Device, the direction and the search. */
  const historyReads: [string | null, Role | null, string | null][] = [];
  const change =(id: string, over: (c: Contact) => Partial<Contact>) => {
    const changed = contacts.map((c) => (c.id === id ? { ...c, ...over(c) } : c));
    contacts = changed;
    return Promise.resolve(changed.find((c) => c.id === id)!);
  };
  const api = {
    myId: () => Promise.resolve({ id: MY_ID, fingerprint: "AAAA-AAAA" }),
    saveFolder: vi.fn(() => Promise.resolve(saveFolder)),
    setSaveFolder: vi.fn((path: string) => {
      saveFolder = path;
      return Promise.resolve(path);
    }),
    needsFirstRun: vi.fn(() => Promise.resolve(firstRun)),
    finishFirstRun: vi.fn(() => {
      firstRun = false;
      return Promise.resolve(null);
    }),
    sendFiles: vi.fn((_to: string, _paths: string[]) => Promise.resolve(TRANSFER)),
    sendBatch: vi.fn((_to: string[], _paths: string[]) => Promise.resolve(BATCH)),
    sendText: vi.fn((_to: string, _text: string) => Promise.resolve(TRANSFER)),
    sendTextBatch: vi.fn((_to: string[], _text: string) => Promise.resolve(BATCH)),
    cancelBatch: vi.fn((_id: string) => Promise.resolve(null)),
    retryTransfer: vi.fn((_id: string) => Promise.resolve("ef".repeat(16))),
    // Like the core: the folder it checked is the one given, else the save folder in use.
    checkOffer: vi.fn((_id: string, folder: string | null) =>
      Promise.resolve({ folder: folder ?? saveFolder, needed: 2048, free: 1_000_000, paths_too_long: false }),
    ),
    acceptOffer: vi.fn((_id: string, _folder: string | null) => Promise.resolve(null)),
    declineOffer: vi.fn(() => Promise.resolve(null)),
    cancelTransfer: vi.fn(() => Promise.resolve(null)),
    resendTransfer: vi.fn(() => Promise.resolve("cd".repeat(16))),
    deviceName: vi.fn(() => Promise.resolve(deviceName)),
    // Like the core: trimmed, and cut to 64 characters.
    setDeviceName: vi.fn((name: string) => {
      deviceName = name.trim().slice(0, 64);
      return Promise.resolve(deviceName);
    }),
    visibility: vi.fn(() => Promise.resolve(visibility)),
    setVisibility: vi.fn((v: Visibility) => {
      visibility = v;
      return Promise.resolve(null);
    }),
    discoveryStatus: vi.fn(() => Promise.resolve(discovery)),
    publicDht: vi.fn(() => Promise.resolve(publicDht)),
    setPublicDht: vi.fn((on: boolean) => {
      publicDht = on;
      return Promise.resolve(null);
    }),
    autostartEnabled: vi.fn(() => Promise.resolve(autostart)),
    setAutostart: vi.fn((on: boolean) => {
      autostart = on;
      return Promise.resolve(null);
    }),
    debugLogging: vi.fn(() => Promise.resolve(debugLogging)),
    setDebugLogging: vi.fn((on: boolean) => {
      debugLogging = on;
      return Promise.resolve(null);
    }),
    exportDiagnostics: vi.fn((_path: string) => Promise.resolve(null)),
    pickDiagnosticsSavePath: vi.fn((_name: string) =>
      Promise.resolve<string | null>("/home/me/diagnostics.zip"),
    ),
    quitApp: vi.fn(() => Promise.resolve(null)),
    appVersion: vi.fn(() => Promise.resolve("0.1.0")),
    checkForUpdate: vi.fn(() => Promise.resolve<UpdateAction>({ type: "none" })),
    pendingUpdate: vi.fn(() => Promise.resolve<UpdateAction>({ type: "none" })),
    installUpdate: vi.fn((_version: string) => Promise.resolve(null)),
    contacts: vi.fn(() => Promise.resolve(contacts)),
    addContact: vi.fn((id: string, deviceName: string | null) => {
      const added = contact({ id, device_name: deviceName, added_at: contacts.length + 1 });
      contacts = [...contacts, added];
      return Promise.resolve(added);
    }),
    setNickname: vi.fn((id: string, nickname: string | null) =>
      change(id, () => ({ nickname: nickname?.trim() ? nickname.trim() : null })),
    ),
    setAutoAccept: vi.fn((id: string, on: boolean) => change(id, () => ({ auto_accept: on }))),
    removeContact: vi.fn((id: string) => {
      contacts = contacts.filter((c) => c.id !== id);
      return Promise.resolve(null);
    }),
    history: vi.fn((peer: string | null, direction: Role | null, search: string | null) => {
      historyReads.push([peer, direction, search]);
      return Promise.resolve(historyEntries);
    }),
    deleteHistoryTransfer: vi.fn((_id: string) => Promise.resolve(null)),
    deleteHistoryBatch: vi.fn((_id: string) => Promise.resolve(null)),
    clearHistory: vi.fn(() => Promise.resolve(null)),
    exportIdentity: vi.fn((_path: string, _password: string) => Promise.resolve(null)),
    checkIdentityImport: vi.fn((_path: string, _password: string) =>
      Promise.resolve({ id: FILE_ID, fingerprint: "BBBB-BBBB" }),
    ),
    importIdentity: vi.fn((_path: string, _password: string) => Promise.resolve(null)),
    transfersInProgress: vi.fn(() => Promise.resolve(0)),
    pickIdentityFile: vi.fn(() => Promise.resolve<string | null>("/home/me/old-laptop.bhid")),
    pickIdentitySavePath: vi.fn((_name: string) => Promise.resolve<string | null>("/home/me/id.bhid")),
    pickFiles: vi.fn(() => Promise.resolve<string[] | null>(["/tmp/photo.jpg"])),
    pickFolder: vi.fn(() => Promise.resolve<string | null>("/mnt/big")),
    showInFolder: vi.fn(() => Promise.resolve()),
    openUrl: vi.fn((_url: string) => Promise.resolve()),
    copyText: vi.fn(() => Promise.resolve()),
    onDeviceEvent: (h: (event: DeviceEvent) => void) => {
      handler = h;
      return Promise.resolve(() => {});
    },
    onShellEvent: (h: (event: ShellEvent) => void) => {
      shellHandler = h;
      return Promise.resolve(() => {});
    },
    onOpenLink: (h: (url: string) => void) => {
      linkHandler = h;
      return Promise.resolve(() => {});
    },
    ...overrides,
  } satisfies Api;
  /** The shell says something: a second launch, a clicked notification, Quit. */
  const shell = (event: ShellEvent) => act(() => shellHandler(event));
  /** The user opens a link that the system hands to this app. */
  const openLink = (url: string) => act(() => linkHandler(url));
  const push = (event: Unstamped) =>
    act(() => handler({ seq: seq++, at: 1_000 * seq, ...event } as DeviceEvent));
  /** What an Offer holds when it is not just `photo.jpg`: a folder, several files, links skipped,
   * names adjusted. */
  type Contents = Partial<{
    kind: "files" | "text";
    text: string | null;
    name: string;
    items: string[];
    file_count: number;
    skipped_links: number;
    adjusted_names: number;
  }>;
  const transfer = (
    role: "sender" | "receiver",
    state: TransferState,
    peerName: string | null = null,
    contents: Contents = {},
  ) =>
    push({
      type: "transfer",
      transfer_id: TRANSFER,
      role,
      peer: PEER_ID,
      peer_name: peerName,
      kind: "files",
      text: null,
      name: "photo.jpg",
      items: ["photo.jpg"],
      file_count: 1,
      skipped_links: 0,
      adjusted_names: 0,
      batch_id: null,
      size: 2048,
      expires_at: EXPIRES_AT,
      state,
      ...contents,
    });
  /** The Device reports the Nearby Devices as they are now. */
  const nearby = (...devices: NearbyDevice[]) => push({ type: "nearby", devices });
  /** Local discovery changes status: the getter has it from now on and an event says so. */
  const discoveryChanges = (status: DiscoveryStatus) => {
    discovery = status;
    return push({ type: "discovery_status", status });
  };
  /** Local discovery has this status from the start, as the getter says, with no event: as
   * after a reload of the UI. */
  const setDiscovery = (status: DiscoveryStatus) => {
    discovery = status;
  };
  /** What the Device's History holds from now on, newest first. */
  const setHistory = (entries: HistoryEntry[]) => {
    historyEntries = entries;
  };
  return { api, push, transfer, nearby, discoveryChanges, setDiscovery, shell, openLink, setHistory, historyReads };
}
