# BhayanakShare

A cross-platform desktop app for sending files directly from one Device to another over a peer-to-peer connection, AirDrop-style.

## Language

**Device**:
One install of BhayanakShare on one machine. The endpoint of every transfer; a person with two machines has two Devices.
_Avoid_: System, machine, node, peer, user

**Device ID**:
The permanent, shareable identifier of a Device, derived from its cryptographic key. It is what one Device gives to another so they can connect.
_Avoid_: UUID, user ID, node ID, address

**Fingerprint**:
A short form of a Device ID (8 characters, e.g. `K3QF-7XNA`) shown for checking by eye. Never used to dial.
_Avoid_: Short ID, code

**Share link**:
The link `bhayanakshare://add/<Device ID>?name=<Device Name>` that hands a Device ID to someone, also shown as a QR code holding the same text. Opening, pasting or scanning it starts adding a Contact. The name in it is only a suggestion by whoever made it, editable and never what identifies the Device: the Fingerprint is still checked. It hands over a Device, not content; sending content is a Transfer.
_Avoid_: Invite link, pairing code

**Contact**:
A Device whose Device ID another Device has saved. The relationship is one-sided: Alice having Bob as a Contact says nothing about whether Bob has Alice.
_Avoid_: Friend, buddy, pairing

**Transfer**:
One sending of content from a Sender Device to a Receiver Device: either files and folders (structure preserved) or a piece of text. The Receiver accepts or declines it before any content moves, unless Auto-accept applies.
_Avoid_: Share, send, job, upload

**Nearby Device**:
A Device found automatically on the same local network, whether or not it is a Contact.
_Avoid_: Peer, neighbour, local device

**Device Name**:
A human-readable label the owner gives their Device, shown to others in place of the Device ID.
_Avoid_: Username, display name, hostname

**Visibility**:
A Device's setting for who can see it as a Nearby Device: Everyone; ID holders (shown as "People who have my ID"), meaning only Devices that already hold its Device ID; or Hidden. It governs discovery only. A Device can still receive a Transfer from anyone who has its Device ID, and a Hidden Device still answers LAN lookups from ID holders, though it neither sees nor is seen Nearby itself. The Device Name is visible to the same audience.
_Avoid_: Discoverable, privacy mode, Contacts only

**Nickname**:
A local name a Device gives one of its Contacts, shown in place of that Contact's Device Name.
_Avoid_: Alias, label

**Batch**:
The group of Transfers created when a Sender sends the same content to several Receivers at once: one Transfer per Receiver. Receivers never learn about each other. The Sender sees and controls a Batch as a group, but each Transfer in it succeeds, fails or is cancelled on its own.
_Avoid_: Multi-send, broadcast, group transfer

**Sender** / **Receiver**:
The two roles a Device can play in a Transfer.

**Offer**:
The Sender's proposal of a Transfer, describing its content (names, sizes, kind) so the Receiver can accept or decline before anything moves.
_Avoid_: Request, invite, prompt

**Transfer History**:
A Device's persistent record of the Transfers it has sent and received.
_Avoid_: Log, activity, inbox

**Auto-accept**:
A per-Contact setting on the Receiver that accepts Transfers from that Contact without prompting. Off by default.

**Identity export**:
A password-protected file holding one Device's secret key, and so its Device ID, and nothing else: no Contacts, no Transfer History, no settings. Importing it on another install (or after a reinstall) gives that install the same Device ID, and replaces the one it had. The install it came from must not keep running with it.
_Avoid_: Backup, account export, key file
