// Every UI string goes through t(), so more languages can be added later (English only in v1).

const en = {
  "app.name": "BhayanakShare",
  "nav.label": "Main",
  "tab.home": "Home",
  "tab.history": "History",
  "tab.contacts": "Contacts",
  "tab.settings": "Settings",
  "home.myId": "My ID",
  "home.fingerprint": "Fingerprint",
  "home.loading": "Starting this Device…",
  "home.error": "This Device could not start.",
  "home.placeholder": "Your Contacts and Nearby Devices will appear here.",
  "history.placeholder": "Your Transfer History will appear here.",
  "contacts.placeholder": "Your Contacts will appear here.",
  "settings.placeholder": "Settings will appear here.",
} as const;

export type MessageKey = keyof typeof en;

export function t(key: MessageKey): string {
  return en[key];
}
