//! The single ordered event stream a Device emits.

use std::sync::Mutex;

use serde::Serialize;
use tokio::sync::mpsc;

use crate::{
    clock::UnixMillis,
    discovery::{DiscoveryStatus, NearbyDevice},
    identity::DeviceId,
    transfer::{BatchId, Role, TransferId, TransferKind, TransferState},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct Event {
    /// Position in the stream, counting from 0 with no gaps.
    pub seq: u64,
    /// When the Device emitted it, by the injected clock.
    pub at: UnixMillis,
    #[serde(flatten)]
    pub kind: EventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    /// A Transfer was created or moved to a new state, on either side.
    Transfer(TransferEvent),
    /// How much of a Transfer's content a Receiver has downloaded so far.
    Progress(ProgressEvent),
    /// A Transfer's files started or finished being hashed.
    Preparing(PreparingEvent),
    /// The list of Nearby Devices changed.
    Nearby(NearbyEvent),
    /// LAN discovery started working or stopped being able to.
    DiscoveryStatus(DiscoveryStatusEvent),
    /// Another Device runs a version of BhayanakShare that cannot exchange Transfers with this
    /// one, and was refused before any Offer.
    VersionMismatch(VersionMismatchEvent),
}

/// Which side of a version mismatch has to update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum Outdated {
    /// This Device runs the older version: the user can update it ("Update now").
    ThisDevice,
    /// The other Device runs the older version: its owner has to update.
    Peer,
}

/// A connection with another Device was refused for the versions of BhayanakShare. Sent on
/// both sides, whoever dialled, and for a resume as well as for a new Offer. A Sender also
/// sees its Transfer fail, with the same advice in the reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct VersionMismatchEvent {
    pub peer: DeviceId,
    /// What the other Device calls itself, as it announced in its `Hello`. Untrusted text.
    pub peer_name: Option<String>,
    /// The other Device's app version, as it announced. Untrusted text.
    pub peer_app_version: Option<String>,
    pub outdated: Outdated,
}

/// Whether a Transfer is Preparing: the Sender is hashing the files, which it does while the
/// Receiver decides (spec section 4). It overlays the Offered and Accepted states, which are
/// announced as before; once the Transfer is in any other state there is nothing to prepare.
/// Sent for Transfers of files only, never for text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct PreparingEvent {
    pub transfer_id: TransferId,
    /// True when hashing starts on the Sender or the Offer arrives on the Receiver; false once
    /// the Sender is done, or the Receiver has been told so (`HashReady`).
    pub preparing: bool,
}

/// The Devices found on the LAN, after a change: one appeared, left, or announced a new name.
/// Always the whole list, so a listener never has to track individual changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct NearbyEvent {
    pub devices: Vec<NearbyDevice>,
}

/// Whether LAN discovery is working, after a change. Not sent while it stays as it is, so a
/// Device whose discovery works never sends one; [`Device::discovery_status`] has the current
/// state for a listener that was not there for the last.
///
/// [`Device::discovery_status`]: crate::Device::discovery_status
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct DiscoveryStatusEvent {
    pub status: DiscoveryStatus,
}

/// Download progress of an accepted Transfer, reported by its Receiver while it is
/// Transferring. The first report and the last (`bytes == total`) are always sent; the ones
/// between are spaced out by the Device's clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct ProgressEvent {
    pub transfer_id: TransferId,
    /// Bytes of the file received so far.
    pub bytes: u64,
    /// The file's size, as offered.
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct TransferEvent {
    pub transfer_id: TransferId,
    /// This Device's role in the Transfer.
    pub role: Role,
    /// The other Device.
    pub peer: DeviceId,
    /// What the other Device calls itself, as it announced when connecting; absent until
    /// known (a Sender learns it only once it has reached the Receiver). Untrusted text: show
    /// it next to the Fingerprint.
    pub peer_name: Option<String>,
    /// What the Transfer carries.
    pub kind: TransferKind,
    /// The first of `items`, for places with room for one name; empty for text.
    pub name: String,
    /// Bytes of files, or of text.
    pub size: u64,
    /// The text of a `Text` Transfer, whole, on every event of it. Untrusted when received:
    /// show it as plain text, never as markup.
    pub text: Option<String>,
    /// The names at the top of what was offered: the files and folders the Sender picked, each
    /// once. All of them are listed, however many; a screen shows as many as fit.
    pub items: Vec<String>,
    pub file_count: u64,
    /// Symlinks the Sender found in the folders it picked and left out.
    pub skipped_links: u32,
    /// Names the Receiver changed to make them safe to write on every system (a Sender has
    /// none to report).
    pub adjusted_names: u32,
    /// The Batch this Transfer is part of, on the Sender that made it (a Receiver has none).
    /// Transfers with the same ID belong to one Batch row; each has its own state.
    pub batch_id: Option<BatchId>,
    /// When the Offer lapses if nobody answers it, by this Device's clock. The same on every
    /// event of the Transfer, so a late subscriber can show the countdown.
    pub expires_at: UnixMillis,
    pub state: TransferState,
}

/// The receiving end of a Device's events. There is exactly one per Device.
#[derive(Debug)]
pub struct EventStream(mpsc::UnboundedReceiver<Event>);

impl EventStream {
    /// The next event, or `None` once the Device has shut down and the stream is drained.
    pub async fn next(&mut self) -> Option<Event> {
        self.0.recv().await
    }

    /// An event that is already waiting, without blocking.
    pub fn try_next(&mut self) -> Option<Event> {
        self.0.try_recv().ok()
    }
}

/// Stamps and orders events. Sequence numbers are assigned under the same lock as the send,
/// so the stream is in sequence order however many tasks emit.
pub(crate) struct EventSink {
    tx: mpsc::UnboundedSender<Event>,
    next_seq: Mutex<u64>,
}

impl EventSink {
    pub(crate) fn new() -> (Self, EventStream) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx, next_seq: Mutex::new(0) }, EventStream(rx))
    }

    pub(crate) fn emit(&self, at: UnixMillis, kind: EventKind) {
        let mut next = self.next_seq.lock().unwrap_or_else(|e| e.into_inner());
        // The stream may already be dropped (a Device nobody listens to); that is fine.
        let _ = self.tx.send(Event { seq: *next, at, kind });
        *next += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn event(i: u64) -> EventKind {
        EventKind::Transfer(TransferEvent {
            transfer_id: TransferId::from_bytes([1; 16]),
            role: Role::Sender,
            peer: DeviceId::from_endpoint_id(iroh::SecretKey::generate().public()),
            peer_name: None,
            kind: TransferKind::Files,
            name: format!("f{i}"),
            size: i,
            text: None,
            items: vec![format!("f{i}")],
            file_count: 1,
            skipped_links: 0,
            adjusted_names: 0,
            batch_id: None,
            expires_at: 0,
            state: TransferState::Offered,
        })
    }

    #[tokio::test]
    async fn events_arrive_in_sequence_order_from_many_tasks() {
        let (sink, mut stream) = EventSink::new();
        let sink = Arc::new(sink);
        let mut tasks = Vec::new();
        for t in 0..8u64 {
            let sink = sink.clone();
            tasks.push(tokio::spawn(async move {
                for i in 0..50 {
                    sink.emit(0, event(t * 50 + i));
                }
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        for want in 0..400 {
            assert_eq!(stream.next().await.unwrap().seq, want);
        }
        assert!(stream.try_next().is_none());
    }
}
