// Sample data for the screen previews: the Devices of a made-up household, their Contacts, and
// what has been sent between them. Device IDs are 52 characters of base32 whose first 8 are the
// Fingerprint, as the real ones are; nothing here belongs to a real Device.

import type { Contact, Role } from "../src/api";
import type { HistoryEntry, HistoryTransfer, NearbyDevice, TransferRecord, TransferState } from "../src/bindings";
import { contact, type Unstamped } from "../src/testApi";

/** A Device ID: its Fingerprint, then the rest of its 52 characters. */
const deviceId = (fingerprint: string, rest: string) => fingerprint + rest;

/** This Device, and the people it knows. */
export const ME = deviceId("R7TM4KQD", "OZNDAB4IBJVUHGAWTHXJYBDN2ZIZCRHRZRDB4V4DT4KO");
export const ASHA = deviceId("K3QF7XNA", "AZAT3CBYXJGA66Q37G5SYPYPIDCRWJ3L6RHFVLWAWOS6");
export const WORK = deviceId("WD4H6ZPT", "LSWHZKC2DU7ZHOTGBYNDSCH2UQVJXKLZM6U7MWRJ3BAB");
export const MUM = deviceId("M2PVJ5RC", "LXUCWLDA2IIXPMIB2LFXF4357JVWJXT56SIG2RHN3WFU");
export const DAD = deviceId("GOESTNBW", "Q36CJZCSSR4FT5JGOPGAHCZFE7EUKO5GWMU3SKOX6MNX");
export const MEERA = deviceId("Y3LC7HGA", "MPW5ZNR2NEI4RYTWFMJBOK24NW7KZ66ZAOYVOEG4RLHB");
export const GUEST = deviceId("J5WD2RFK", "MKQCYZM2WYGD6ABW2K54BBFAATO3PVC3E6O4MWJ35JLR");

export const MY_DEVICE_NAME = "Priya's desktop";
export const SAVE_FOLDER = "/home/priya/Downloads/BhayanakShare";

/** What a Device calls itself when it announces. */
export const announced: Record<string, string> = {
  [ASHA]: "Asha's laptop",
  [WORK]: "DESKTOP-4F7QK2",
  [MUM]: "Mum's phone",
  [MEERA]: "Meera's tablet",
};

/** Four Contacts: named by their own Device Name, by a Nickname, with Auto-accept, and unnamed. */
export const CONTACTS: Contact[] = [
  contact({ id: ASHA, device_name: "Asha's laptop", added_at: 1 }),
  contact({ id: WORK, nickname: "Work desktop", device_name: "DESKTOP-4F7QK2", added_at: 2 }),
  contact({ id: MUM, nickname: "Mum", device_name: "Mum's phone", auto_accept: true, added_at: 3 }),
  contact({ id: DAD, added_at: 4 }),
];

/** On the same network: two Contacts, a stranger with a name and one that announces none. */
export const NEARBY: NearbyDevice[] = [
  { id: ASHA, name: "Asha's laptop" },
  { id: WORK, name: "DESKTOP-4F7QK2" },
  { id: MEERA, name: "Meera's tablet" },
  { id: GUEST, name: null },
];

export interface Who {
  id: string;
  /** What it announced when it connected. */
  name: string | null;
}
export const who = (id: string): Who => ({ id, name: announced[id] ?? null });

const MIB = 1 << 20;
const GIB = 1 << 30;
export const mib = (n: number) => Math.round(n * MIB);
export const gib = (n: number) => Math.round(n * GIB);

/** The nth Transfer's ID. */
export const transferId = (n: number) => n.toString(16).padStart(2, "0").repeat(16);
export const BATCH_ID = "ba".repeat(16);

export type TransferEvent = Extract<Unstamped, { type: "transfer" }>;

/** A Transfer event as the Device would send it; an Offer lapses 8 minutes 42 seconds from now. */
export function transfer(
  n: number,
  role: Role,
  state: TransferState,
  other: Who,
  over: Partial<TransferEvent> = {},
): TransferEvent {
  return {
    type: "transfer",
    transfer_id: transferId(n),
    role,
    peer: other.id,
    peer_name: other.name,
    kind: "files",
    text: null,
    name: "Invoice-2026-10.pdf",
    items: ["Invoice-2026-10.pdf"],
    file_count: 1,
    skipped_links: 0,
    adjusted_names: 0,
    batch_id: null,
    size: mib(1.8),
    expires_at: Date.now() + 8 * 60_000 + 42_000,
    state,
    ...over,
  };
}

/** The folder of holiday photos that most of the Transfer rows carry. */
export const TRIP = {
  name: "Trip photos",
  items: ["Trip photos"],
  file_count: 214,
  size: gib(1.4),
};

/** History is dated from here. The capture script stops the clock at the same moment, so the
 * countdown of an Offer reads the same in every capture. */
const NOW = Date.UTC(2026, 9, 10, 12, 0, 0);
const ago = (minutes: number) => NOW - minutes * 60_000;

function record(over: Partial<TransferRecord>): TransferRecord {
  const made = recordOf(over);
  return made.state.kind === "declined" || made.state.kind === "expired" ? { ...made, accepted_at: null } : made;
}

function recordOf(over: Partial<TransferRecord>): TransferRecord {
  const created = over.created_at ?? ago(120);
  return {
    id: transferId(1),
    role: "receiver",
    peer: ASHA,
    peer_name: "Asha's laptop",
    name: "Invoice-2026-10.pdf",
    kind: "files",
    size: mib(1.8),
    text: null,
    items: ["Invoice-2026-10.pdf"],
    file_count: 1,
    skipped_links: 0,
    adjusted_names: 0,
    batch_id: null,
    state: { kind: "completed", saved_to: `${SAVE_FOLDER}/Invoice-2026-10.pdf` },
    created_at: created,
    accepted_at: created + 4_000,
    updated_at: created + 61_000,
    ...over,
  };
}

const row = (over: Partial<TransferRecord>, saved_present: boolean | null = null): HistoryEntry => ({
  kind: "transfer",
  transfer: { record: record(over), saved_present },
});

/** A Batch this Device sent to four Receivers, each ending its own way. */
function batch(): HistoryEntry {
  const to = (n: number, who: Who, state: TransferState): HistoryTransfer => ({
    saved_present: null,
    record: record({
      id: transferId(40 + n),
      role: "sender",
      peer: who.id,
      peer_name: who.name,
      name: "Team offsite agenda.pdf",
      items: ["Team offsite agenda.pdf"],
      size: mib(0.6),
      batch_id: BATCH_ID,
      created_at: ago(300),
      state,
    }),
  });
  return {
    kind: "batch",
    batch_id: BATCH_ID,
    transfers: [
      to(1, who(ASHA), { kind: "completed", saved_to: null }),
      to(2, who(WORK), { kind: "completed", saved_to: null }),
      to(3, who(MUM), { kind: "declined" }),
      to(4, who(MEERA), { kind: "failed", reason: "Meera's tablet could not be reached." }),
    ],
  };
}

/** What History holds: received and sent, every way a Transfer can end, and a Batch. */
export const HISTORY: HistoryEntry[] = [
  row(
    {
      id: transferId(10),
      name: "Trip photos",
      items: ["Trip photos"],
      file_count: 214,
      size: gib(1.4),
      adjusted_names: 2,
      created_at: ago(120),
      state: { kind: "completed", saved_to: `${SAVE_FOLDER}/Trip photos` },
    },
    true,
  ),
  row({
    id: transferId(11),
    role: "sender",
    peer: WORK,
    peer_name: "DESKTOP-4F7QK2",
    name: "build-2026.10.1.zip",
    items: ["build-2026.10.1.zip"],
    size: mib(212),
    created_at: ago(180),
    state: { kind: "failed", reason: "The connection to Work desktop was lost and could not be restored." },
  }),
  row({
    id: transferId(12),
    peer: MUM,
    peer_name: "Mum's phone",
    kind: "text",
    name: "",
    items: [],
    file_count: 0,
    size: 52,
    text: "The wifi password at the cafe is blue-heron-2210",
    state: { kind: "completed", saved_to: null },
    created_at: ago(240),
  }),
  batch(),
  row(
    {
      id: transferId(13),
      peer: WORK,
      peer_name: "DESKTOP-4F7QK2",
      name: "Quarterly report.pdf",
      items: ["Quarterly report.pdf"],
      size: mib(48),
      created_at: ago(1_500),
      state: { kind: "completed", saved_to: `${SAVE_FOLDER}/Quarterly report.pdf` },
    },
    false,
  ),
  row({
    id: transferId(14),
    role: "sender",
    peer: MUM,
    peer_name: "Mum's phone",
    name: "Lecture slides.key",
    items: ["Lecture slides.key"],
    size: mib(86),
    created_at: ago(1_900),
    state: { kind: "expired" },
  }),
  row({
    id: transferId(15),
    role: "sender",
    peer: MEERA,
    peer_name: "Meera's tablet",
    name: "Recipe scans",
    items: ["Recipe scans"],
    file_count: 9,
    size: mib(31),
    created_at: ago(2_900),
    state: { kind: "declined" },
  }),
  row({
    id: transferId(16),
    peer: DAD,
    peer_name: "Dad's PC",
    name: "Holiday itinerary.docx",
    items: ["Holiday itinerary.docx"],
    size: mib(0.2),
    created_at: ago(4_300),
    state: { kind: "cancelled", by: "sender" },
  }),
  row({
    id: transferId(17),
    role: "sender",
    peer: ASHA,
    peer_name: "Asha's laptop",
    name: "Flat keys.jpg",
    items: ["Flat keys.jpg"],
    size: mib(3.4),
    created_at: ago(5_700),
    state: { kind: "completed", saved_to: null },
  }),
];

/**
 * History as the Device narrows it: by the other Device, by what this Device did and by a search
 * of the item names. With a Device chosen, a Batch gives way to that Receiver's own Transfer.
 */
export function narrowed(entries: HistoryEntry[], peer: string | null, direction: Role | null, search: string | null) {
  const wanted = search?.toLowerCase() ?? null;
  const fits = (x: HistoryTransfer) =>
    (peer === null || x.record.peer === peer) &&
    (direction === null || x.record.role === direction) &&
    (wanted === null || x.record.items.concat(x.record.name).some((i) => i.toLowerCase().includes(wanted)));
  return entries.flatMap((entry): HistoryEntry[] => {
    if (entry.kind === "transfer") return fits(entry.transfer) ? [entry] : [];
    const kept = entry.transfers.filter(fits);
    if (kept.length === 0) return [];
    return peer === null ? [{ ...entry, transfers: kept }] : kept.map((transfer) => ({ kind: "transfer", transfer }));
  });
}
