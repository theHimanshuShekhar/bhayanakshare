// The only place the UI talks to the outside: the commands and events the Rust shell
// generated (bindings.ts), plus the file picker and "show in folder". Everything else gets an
// `Api`, so tests can hand the UI a stand-in.

import { open } from "@tauri-apps/plugin-dialog";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  commands,
  events,
  type BatchId,
  type Contact,
  type DeviceEvent,
  type MyId,
  type ShellEvent,
  type SpaceCheck,
  type TransferId,
  type Visibility,
} from "./bindings";

export type { BatchId, Contact, DeviceEvent, MyId, ShellEvent, SpaceCheck, TransferId, Visibility };

export interface Api {
  myId(): Promise<MyId>;
  /** The folder accepted files are saved to. */
  saveFolder(): Promise<string>;
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
  /** Whether a pending Offer fits in `folder` (the save folder when null). */
  checkOffer(id: TransferId, folder: string | null): Promise<SpaceCheck>;
  /** Accepts into `folder` for this Offer only (the save folder when null). */
  acceptOffer(id: TransferId, folder: string | null): Promise<unknown>;
  declineOffer(id: TransferId): Promise<unknown>;
  /** Stops a Transfer on either side, until it starts saving. */
  cancelTransfer(id: TransferId): Promise<unknown>;
  /** Sends an expired Offer again; resolves to the new Transfer ID. */
  resendTransfer(id: TransferId): Promise<string>;
  /** Who can see this Device as a Nearby Device. */
  visibility(): Promise<Visibility>;
  /** Changes who can see this Device as a Nearby Device; it takes effect at once. */
  setVisibility(visibility: Visibility): Promise<unknown>;
  /** Whether this Device starts when the user logs in. */
  autostartEnabled(): Promise<boolean>;
  setAutostart(on: boolean): Promise<unknown>;
  /** The user confirmed quitting: the Device saves its progress, then the app exits. */
  quitApp(): Promise<unknown>;
  /** Every Contact, in the order they were added. */
  contacts(): Promise<Contact[]>;
  /** Saves a Device as a Contact; `deviceName` is the name it goes by, if known. */
  addContact(id: string, deviceName: string | null): Promise<Contact>;
  /** Sets the Nickname; null or empty goes back to the Device Name. */
  setNickname(id: string, nickname: string | null): Promise<Contact>;
  setAutoAccept(id: string, on: boolean): Promise<Contact>;
  /** Forgets a Contact; its Transfer records stay. */
  removeContact(id: string): Promise<unknown>;
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
  /** Calls `handler` for what the shell has to say (files to send, quitting). */
  onShellEvent(handler: (event: ShellEvent) => void): Promise<() => void>;
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

export const tauriApi: Api = {
  myId: commands.myId,
  saveFolder: commands.saveFolder,
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
  visibility: commands.visibility,
  setVisibility: commands.setVisibility,
  autostartEnabled: commands.autostartEnabled,
  setAutostart: commands.setAutostart,
  quitApp: commands.quitApp,
  contacts: commands.contacts,
  addContact: commands.addContact,
  setNickname: commands.setNickname,
  setAutoAccept: commands.setAutoAccept,
  removeContact: commands.removeContact,
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
};
