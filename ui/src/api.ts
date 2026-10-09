// The only place the UI talks to the outside: the commands and events the Rust shell
// generated (bindings.ts), plus the file picker and "show in folder". Everything else gets an
// `Api`, so tests can hand the UI a stand-in.

import { getVersion } from "@tauri-apps/api/app";
import { getCurrent, onOpenUrl } from "@tauri-apps/plugin-deep-link";
import { open, save } from "@tauri-apps/plugin-dialog";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import { t } from "./i18n";
import {
  commands,
  events,
  type BatchId,
  type Contact,
  type DeviceEvent,
  type DiscoveryStatus,
  type HistoryEntry,
  type IdentityError,
  type IdentityOwner,
  type MyId,
  type Role,
  type SaveFolderError,
  type ShellEvent,
  type SpaceCheck,
  type TransferId,
  type UnavailableReason,
  type UpdateAction,
  type UpdateError,
  type Visibility,
} from "./bindings";

export type {
  BatchId,
  Contact,
  DeviceEvent,
  DiscoveryStatus,
  HistoryEntry,
  IdentityError,
  IdentityOwner,
  MyId,
  Role,
  SaveFolderError,
  ShellEvent,
  SpaceCheck,
  TransferId,
  UnavailableReason,
  UpdateAction,
  UpdateError,
  Visibility,
};

export interface Api {
  myId(): Promise<MyId>;
  /** The folder accepted files are saved to unless an Offer names another. */
  saveFolder(): Promise<string>;
  /** Makes `path`, an absolute path, the save folder from the next Offer on (made if missing, and
   * it must be writable); resolves to the folder as kept. Rejects with a `SaveFolderError`. */
  setSaveFolder(path: string): Promise<string>;
  /** Whether first run is still to be done: its screen is shown instead of the tabs. */
  needsFirstRun(): Promise<boolean>;
  /** Records that the user has been through first run. */
  finishFirstRun(): Promise<unknown>;
  /** Offers the files and folders at `paths` to the Device with ID `to` as one Transfer;
   * resolves to the Transfer ID. */
  sendFiles(to: string, paths: string[]): Promise<string>;
  /** Offers the files and folders at `paths` to every Device with an ID in `to`, as a Batch
   * (one Transfer each); resolves to the Batch ID. */
  sendBatch(to: string[], paths: string[]): Promise<string>;
  /** Offers `text` to the Device with ID `to`; resolves to the Transfer ID. */
  sendText(to: string, text: string): Promise<string>;
  /** Offers `text` to every Device with an ID in `to`, as a Batch; resolves to the Batch ID. */
  sendTextBatch(to: string[], text: string): Promise<string>;
  /** Stops every Transfer of a Batch that is still running. */
  cancelBatch(id: BatchId): Promise<unknown>;
  /** Sends a Failed Transfer of a Batch again with a new Offer; resolves to the new Transfer ID. */
  retryTransfer(id: TransferId): Promise<string>;
  /** Whether a pending Offer fits in `folder` (the save folder when null); says which folder it
   * checked, where the Offer would be saved. */
  checkOffer(id: TransferId, folder: string | null): Promise<SpaceCheck>;
  /** Accepts into `folder` for this Offer only (the save folder when null). */
  acceptOffer(id: TransferId, folder: string | null): Promise<unknown>;
  declineOffer(id: TransferId): Promise<unknown>;
  /** Stops a Transfer on either side, until it starts saving. */
  cancelTransfer(id: TransferId): Promise<unknown>;
  /** Sends an expired Offer again; resolves to the new Transfer ID. */
  resendTransfer(id: TransferId): Promise<string>;
  /** This Device's name, as other Devices see it. */
  deviceName(): Promise<string>;
  /** Renames this Device, at once; resolves to the name as kept (trimmed, shortened if too
   * long). Rejects with the reason it was refused, as when it is empty. */
  setDeviceName(name: string): Promise<string>;
  /** Who can see this Device as a Nearby Device. */
  visibility(): Promise<Visibility>;
  /** Changes who can see this Device as a Nearby Device; it takes effect at once. */
  setVisibility(visibility: Visibility): Promise<unknown>;
  /** Whether local discovery is working. Changes arrive as `discovery_status` events; this is for
   * a UI that was not listening for the last. */
  discoveryStatus(): Promise<DiscoveryStatus>;
  /** Whether this Device uses the public DHT, besides n0's servers, to publish its address and
   * find its Contacts'. */
  publicDht(): Promise<boolean>;
  /** Turns the public DHT on or off; it takes effect at once and is kept. */
  setPublicDht(on: boolean): Promise<unknown>;
  /** Whether this Device starts when the user logs in. */
  autostartEnabled(): Promise<boolean>;
  setAutostart(on: boolean): Promise<unknown>;
  /** Whether debug logging is on. */
  debugLogging(): Promise<boolean>;
  /** Turns debug logging on or off; it takes effect at once and is kept. */
  setDebugLogging(on: boolean): Promise<unknown>;
  /** Writes the log files and the app version, as a zip, to the file at `path`. */
  exportDiagnostics(path: string): Promise<unknown>;
  /** Asks the user where to save the diagnostics zip, suggesting `name`; null if they cancel. */
  pickDiagnosticsSavePath(name: string): Promise<string | null>;
  /** The user confirmed quitting: the Device saves its progress, then the app exits. */
  quitApp(): Promise<unknown>;
  /** The version of this app. */
  appVersion(): Promise<string>;
  /** Looks for a newer release now. Rejects when the check fails, as it does offline. */
  checkForUpdate(): Promise<UpdateAction>;
  /** The newer release the last check found, if any. */
  pendingUpdate(): Promise<UpdateAction>;
  /** Installs `version`, the release the user agreed to, and restarts the app (the Windows installer
   * or an AppImage). The caller has the user's agreement. Rejects with an `UpdateError` if it could not be
   * installed, as when `version` is no longer the release found. */
  installUpdate(version: string): Promise<unknown>;
  /** Every Contact, in the order they were added. */
  contacts(): Promise<Contact[]>;
  /** Saves a Device as a Contact; `deviceName` is the name it goes by, if known. */
  addContact(id: string, deviceName: string | null): Promise<Contact>;
  /** Sets the Nickname; null or empty goes back to the Device Name. */
  setNickname(id: string, nickname: string | null): Promise<Contact>;
  setAutoAccept(id: string, on: boolean): Promise<Contact>;
  /** Forgets a Contact; its Transfer records stay. */
  removeContact(id: string): Promise<unknown>;
  /**
   * Transfer History, newest first, narrowed by the other Device's ID, by the role this Device
   * played (`direction`) and by a search of the item names; null leaves that out.
   */
  history(peer: string | null, direction: Role | null, search: string | null): Promise<HistoryEntry[]>;
  /** Deletes one Transfer that has ended from History. */
  deleteHistoryTransfer(id: TransferId): Promise<unknown>;
  /** Deletes the Transfers of a Batch that have ended from History. */
  deleteHistoryBatch(id: BatchId): Promise<unknown>;
  /** Deletes every Transfer that has ended from History; the ones still going stay. */
  clearHistory(): Promise<unknown>;
  /** Writes this Device's identity, protected by `password`, to the file at `path`. Rejects with
   * an `IdentityError`. */
  exportIdentity(path: string, password: string): Promise<unknown>;
  /** Whose identity the file at `path` holds, if `password` opens it. Changes nothing. Rejects
   * with an `IdentityError`. */
  checkIdentityImport(path: string, password: string): Promise<IdentityOwner>;
  /** Replaces this Device's identity with the one in the file, and restarts the app. Rejects
   * with an `IdentityError`. */
  importIdentity(path: string, password: string): Promise<unknown>;
  /** How many Transfers are in progress: the ones replacing the identity stops. */
  transfersInProgress(): Promise<number>;
  /** Asks the user for an identity file; null if they cancel. */
  pickIdentityFile(): Promise<string | null>;
  /** Asks the user where to save an identity file, suggesting `name`; null if they cancel. */
  pickIdentitySavePath(name: string): Promise<string | null>;
  /** Asks the user for one or more files; null if they cancel. */
  pickFiles(): Promise<string[] | null>;
  /** Asks the user for a folder; null if they cancel. */
  pickFolder(): Promise<string | null>;
  showInFolder(path: string): Promise<void>;
  /** Opens a web page in the user's browser. */
  openUrl(url: string): Promise<void>;
  copyText(text: string): Promise<void>;
  /** Calls `handler` for every Device event, in order. Resolves to the unsubscribe function. */
  onDeviceEvent(handler: (event: DeviceEvent) => void): Promise<() => void>;
  /** Calls `handler` for what the shell has to say (files to send, quitting, updates). */
  onShellEvent(handler: (event: ShellEvent) => void): Promise<() => void>;
  /** Calls `handler` with every `bhayanakshare://` link the user opens, including the one that
   * started the app. Resolves to the unsubscribe function. */
  onOpenLink(handler: (url: string) => void): Promise<() => void>;
}

/** The async clipboard API where the webview has it, else the older copy command. */
async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
    return;
  } catch {
    // Not available here (or refused); try the older way below.
  }
  const field = document.createElement("textarea");
  field.value = text;
  field.setAttribute("readonly", "");
  field.style.position = "fixed";
  field.style.opacity = "0";
  document.body.append(field);
  field.select();
  const copied = document.execCommand("copy");
  field.remove();
  if (!copied) throw new Error("copy failed");
}

/** Set once the link that started the app has been handed over: the plugin keeps reporting it,
 * and a UI that mounts again (or a reloaded page) must not open a dismissed link again. */
let startupLinkTaken = false;

export const tauriApi: Api = {
  myId: commands.myId,
  saveFolder: commands.saveFolder,
  setSaveFolder: commands.setSaveFolder,
  needsFirstRun: commands.needsFirstRun,
  finishFirstRun: commands.finishFirstRun,
  sendFiles: commands.sendFiles,
  sendBatch: commands.sendBatch,
  sendText: commands.sendText,
  sendTextBatch: commands.sendTextBatch,
  cancelBatch: commands.cancelBatch,
  retryTransfer: commands.retryTransfer,
  checkOffer: commands.checkOffer,
  acceptOffer: commands.acceptOffer,
  declineOffer: commands.declineOffer,
  cancelTransfer: commands.cancelTransfer,
  resendTransfer: commands.resendTransfer,
  deviceName: commands.deviceName,
  setDeviceName: commands.setDeviceName,
  visibility: commands.visibility,
  setVisibility: commands.setVisibility,
  discoveryStatus: commands.discoveryStatus,
  publicDht: commands.publicDht,
  setPublicDht: commands.setPublicDht,
  autostartEnabled: commands.autostartEnabled,
  setAutostart: commands.setAutostart,
  debugLogging: commands.debugLogging,
  setDebugLogging: commands.setDebugLogging,
  exportDiagnostics: commands.exportDiagnostics,
  pickDiagnosticsSavePath: (name) =>
    save({ defaultPath: name, filters: [{ name: t("diagnostics.fileType"), extensions: ["zip"] }] }),
  quitApp: commands.quitApp,
  appVersion: getVersion,
  checkForUpdate: commands.checkForUpdate,
  pendingUpdate: commands.pendingUpdate,
  installUpdate: commands.installUpdate,
  contacts: commands.contacts,
  addContact: commands.addContact,
  setNickname: commands.setNickname,
  setAutoAccept: commands.setAutoAccept,
  removeContact: commands.removeContact,
  history: commands.history,
  deleteHistoryTransfer: commands.deleteHistoryTransfer,
  deleteHistoryBatch: commands.deleteHistoryBatch,
  clearHistory: commands.clearHistory,
  exportIdentity: commands.exportIdentity,
  checkIdentityImport: commands.checkIdentityImport,
  importIdentity: commands.importIdentity,
  transfersInProgress: commands.transfersInProgress,
  pickIdentityFile: async () => {
    const picked = await open({
      multiple: false,
      directory: false,
      filters: [
        { name: t("identity.fileType"), extensions: ["bhid"] },
        { name: t("identity.fileTypeAll"), extensions: ["*"] },
      ],
    });
    return typeof picked === "string" ? picked : null;
  },
  pickIdentitySavePath: (name) =>
    save({ defaultPath: name, filters: [{ name: t("identity.fileType"), extensions: ["bhid"] }] }),
  pickFiles: async () => {
    const picked = await open({ multiple: true, directory: false });
    return Array.isArray(picked) && picked.length > 0 ? picked : null;
  },
  pickFolder: async () => {
    const picked = await open({ multiple: false, directory: true });
    return typeof picked === "string" ? picked : null;
  },
  showInFolder: revealItemInDir,
  openUrl: (url) => openUrl(url),
  copyText,
  onDeviceEvent: async (handler) => {
    const unlisten = await events.deviceEvent.listen((e) => handler(e.payload));
    // Everything emitted before this point is held by the shell until now.
    await commands.eventsReady();
    return unlisten;
  },
  onShellEvent: async (handler) => events.shellEvent.listen((e) => handler(e.payload)),
  onOpenLink: async (handler) => {
    let live = true;
    const deliver = (urls: string[] | null) => urls?.forEach((url) => handler(url));
    const unlisten = await onOpenUrl((urls) => {
      if (live) deliver(urls);
    });
    if (!startupLinkTaken) {
      // A link that started the app arrived before anyone was listening.
      getCurrent().then(
        (urls) => {
          if (!live || startupLinkTaken) return;
          startupLinkTaken = true;
          deliver(urls);
        },
        () => {},
      );
    }
    return () => {
      live = false;
      unlisten();
    };
  },
};
