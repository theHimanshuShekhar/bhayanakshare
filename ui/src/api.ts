// The only place the UI talks to the Rust shell: typed commands in, one event stream out.
// The shapes mirror what bhayanakshare-core serialises.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface MyId {
  /** 52-character base32 Device ID. */
  id: string;
  /** First 8 characters, XXXX-XXXX. */
  fingerprint: string;
}

export type TransferState =
  | { kind: "offered" | "accepted" | "declined" | "transferring" | "saving" }
  | { kind: "completed"; saved_to: string | null }
  | { kind: "failed"; reason: string };

export interface DeviceEvent {
  /** Position in the stream, from 0 with no gaps. */
  seq: number;
  /** Unix milliseconds. */
  at: number;
  type: "transfer";
  transfer_id: string;
  role: "sender" | "receiver";
  /** The other Device's ID. */
  peer: string;
  name: string;
  size: number;
  state: TransferState;
}

export const myId = () => invoke<MyId>("my_id");

/** Offers the file at `path` to the Device with ID `to`; resolves to the Transfer ID. */
export const sendFile = (to: string, path: string) => invoke<string>("send_file", { to, path });

export const acceptOffer = (transferId: string) => invoke<void>("accept_offer", { transferId });

export const declineOffer = (transferId: string) => invoke<void>("decline_offer", { transferId });

export const onDeviceEvent = (handler: (event: DeviceEvent) => void): Promise<UnlistenFn> =>
  listen<DeviceEvent>("device-event", (e) => handler(e.payload));
