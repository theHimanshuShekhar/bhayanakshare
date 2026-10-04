//! The single ordered event stream a Device emits.

use std::sync::Mutex;

use serde::Serialize;
use tokio::sync::mpsc;

use crate::{
    clock::UnixMillis,
    identity::DeviceId,
    transfer::{Role, TransferId, TransferState},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Event {
    /// Position in the stream, counting from 0 with no gaps.
    pub seq: u64,
    /// When the Device emitted it, by the injected clock.
    pub at: UnixMillis,
    #[serde(flatten)]
    pub kind: EventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    /// A Transfer was created or moved to a new state, on either side.
    Transfer(TransferEvent),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TransferEvent {
    pub transfer_id: TransferId,
    /// This Device's role in the Transfer.
    pub role: Role,
    /// The other Device.
    pub peer: DeviceId,
    pub name: String,
    pub size: u64,
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
            name: format!("f{i}"),
            size: i,
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
