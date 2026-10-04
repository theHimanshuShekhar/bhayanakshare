// Every UI string goes through t(), so more languages can be added later (English only in v1).
// `{name}` marks a value filled in by the caller.

const en = {
  "app.name": "BhayanakShare",
  "nav.label": "Main",
  "tab.home": "Home",
  "tab.history": "History",
  "tab.contacts": "Contacts",
  "tab.settings": "Settings",

  "home.devices": "Devices",
  "home.placeholder": "Your Contacts and Nearby Devices will appear here.",
  "home.sendToId": "Send to ID…",
  "home.transfers": "Transfers",
  "home.noTransfers": "No Transfers yet.",
  "home.loading": "Starting this Device…",
  "home.error": "This Device could not start.",
  "history.placeholder": "Your Transfer History will appear here.",
  "contacts.placeholder": "Your Contacts will appear here.",
  "settings.placeholder": "Settings will appear here.",

  "myId.heading": "My ID",
  "myId.fingerprint": "Fingerprint",
  "myId.deviceId": "Device ID",
  "myId.copy": "Copy",
  "myId.copied": "Copied",
  "myId.copyFailed": "Could not copy. Select the Device ID and copy it by hand.",

  "send.heading": "Send to ID",
  "send.idLabel": "Device ID",
  "send.idHint": "Paste the Device ID of the Device to send to.",
  "send.chooseFile": "Choose file…",
  "send.cancel": "Cancel",
  "send.failed": "Could not send the file. {reason}",

  "offer.heading": "Incoming file",
  "offer.from": "From (Fingerprint)",
  "offer.file": "File",
  "offer.size": "Size",
  "offer.saveTo": "Will be saved to",
  "offer.accept": "Accept",
  "offer.decline": "Decline",
  "offer.failed": "That Offer is no longer waiting. {reason}",

  "transfer.to": "{name} to {peer}",
  "transfer.from": "{name} from {peer}",
  "transfer.sending.offered": "Waiting for {peer}…",
  "transfer.sending.accepted": "{peer} accepted. Sending…",
  "transfer.sending.declined": "{peer} declined.",
  "transfer.sending.completed": "Sent.",
  "transfer.sending.failed": "Could not send. {reason}",
  "transfer.receiving.offered": "Waiting for your answer.",
  "transfer.receiving.accepted": "Accepted. Starting…",
  "transfer.receiving.transferring": "Receiving…",
  "transfer.receiving.saving": "Saving…",
  "transfer.receiving.declined": "You declined.",
  "transfer.receiving.completed": "Received.",
  "transfer.receiving.failed": "Could not receive. {reason}",
  "transfer.savedTo": "Saved to {path}",
  "transfer.showInFolder": "Show in folder",
  "transfer.showInFolderLabel": "Show {name} in folder",
  "transfer.progress": "{percent}% of {size}",
  "transfer.progressLabel": "Progress of {name}",
  "transfer.rate": "{size}/s",
} as const;

export type MessageKey = keyof typeof en;

export type MessageParams = Record<string, string | number>;

export function t(key: MessageKey, params: MessageParams = {}): string {
  return en[key].replace(/\{(\w+)\}/g, (whole, name: string) =>
    name in params ? String(params[name]) : whole,
  );
}
