// The only place the UI talks to the outside: the commands and events the Rust shell
// generated (bindings.ts), plus the file picker and "show in folder". Everything else gets an
// `Api`, so tests can hand the UI a stand-in.

import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  commands,
  events,
  type DeviceEvent,
  type MyId,
  type SpaceCheck,
  type TransferId,
} from "./bindings";

export type { DeviceEvent, MyId, SpaceCheck, TransferId };

export interface Api {
  myId(): Promise<MyId>;
  /** The folder accepted files are saved to. */
  saveFolder(): Promise<string>;
  /** Offers the file at `path` to the Device with ID `to`; resolves to the Transfer ID. */
  sendFile(to: string, path: string): Promise<string>;
  /** Whether a pending Offer fits in `folder` (the save folder when null). */
  checkOffer(id: TransferId, folder: string | null): Promise<SpaceCheck>;
  /** Accepts into `folder` for this Offer only (the save folder when null). */
  acceptOffer(id: TransferId, folder: string | null): Promise<unknown>;
  declineOffer(id: TransferId): Promise<unknown>;
  /** Asks the user for a file; null if they cancel. */
  pickFile(): Promise<string | null>;
  /** Asks the user for a folder; null if they cancel. */
  pickFolder(): Promise<string | null>;
  showInFolder(path: string): Promise<void>;
  copyText(text: string): Promise<void>;
  /** Calls `handler` for every Device event, in order. Resolves to the unsubscribe function. */
  onDeviceEvent(handler: (event: DeviceEvent) => void): Promise<() => void>;
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
  sendFile: commands.sendFile,
  checkOffer: commands.checkOffer,
  acceptOffer: commands.acceptOffer,
  declineOffer: commands.declineOffer,
  pickFile: async () => {
    const picked = await open({ multiple: false, directory: false });
    return typeof picked === "string" ? picked : null;
  },
  pickFolder: async () => {
    const picked = await open({ multiple: false, directory: true });
    return typeof picked === "string" ? picked : null;
  },
  showInFolder: revealItemInDir,
  copyText,
  onDeviceEvent: async (handler) => {
    const unlisten = await events.deviceEvent.listen((e) => handler(e.payload));
    // Everything emitted before this point is held by the shell until now.
    await commands.eventsReady();
    return unlisten;
  },
};
