//! Access control for the Device's iroh-blobs provider (spec section 8).
//!
//! Only the Receiver that accepted a Transfer may fetch its content, and only while the
//! Transfer runs: the Sender takes out a [`Grant`] for the pair (Receiver, root hash) when
//! the Receiver says yes and drops it when the Transfer ends. The provider asks this gate
//! about every connection and every request; anything not granted is refused with
//! `AbortReason::Permission`.
//!
//! Only GET of a granted root hash is ever served. PUSH, GET_MANY and OBSERVE are always
//! refused, and so is a GET of a hash inside the Collection: the Receiver asks for the root
//! and iroh-blobs walks the Collection itself.
//!
//! iroh-blobs 0.103 reads only `EventMask::get` for every kind of request (see
//! `EventSender::request`). The `push`, `get_many` and `observe` fields are ignored, and with
//! no event sender at all any peer can PUSH data into the store. So the hook has to turn each
//! of those requests away itself.

use std::{
    collections::{HashMap, hash_map::Entry},
    sync::{Arc, Mutex},
};

use iroh::EndpointId;
use iroh_blobs::{
    Hash,
    provider::events::{
        AbortReason, ConnectMode, EventMask, EventResult, EventSender, ObserveMode,
        ProviderMessage, RequestMode, ThrottleMode,
    },
};

use crate::identity::DeviceId;

/// Which events the provider sends us. Every request kind is intercepted (the provider
/// decides that from `get` alone, see above); the other fields say the same thing for the
/// versions that read them.
const MASK: EventMask = EventMask {
    connected: ConnectMode::Intercept,
    get: RequestMode::Intercept,
    get_many: RequestMode::Disabled,
    push: RequestMode::Disabled,
    observe: ObserveMode::Intercept,
    throttle: ThrottleMode::None,
};

#[derive(Default)]
pub(crate) struct Gate {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// Transfers running, per (Receiver, root hash): the same file sent twice to the same
    /// Receiver has the same root, and each Transfer ends on its own.
    grants: HashMap<(EndpointId, Hash), usize>,
    /// Who is on each open connection, from the provider's `ClientConnected` event.
    peers: HashMap<u64, EndpointId>,
}

/// Lets one Receiver fetch one root hash for as long as it is held.
pub(crate) struct Grant {
    gate: Arc<Gate>,
    key: (EndpointId, Hash),
}

impl Gate {
    /// Allows `peer` to GET `root`. Take it out before the Receiver can learn it is allowed.
    pub fn allow(self: &Arc<Self>, peer: EndpointId, root: Hash) -> Grant {
        *self.state().grants.entry((peer, root)).or_default() += 1;
        Grant { gate: self.clone(), key: (peer, root) }
    }

    /// The provider hooked up to this gate. Runs until the provider is dropped.
    pub fn events(self: &Arc<Self>) -> EventSender {
        let (sender, mut messages) = EventSender::channel(32, MASK);
        let gate = self.clone();
        // Not a tracked task: it ends when the provider drops its sender at shutdown, and
        // the Device waits for its tracked tasks before it shuts the provider down.
        tokio::spawn(async move {
            while let Some(message) = messages.recv().await {
                gate.answer(message).await;
            }
        });
        sender
    }

    async fn answer(&self, message: ProviderMessage) {
        // A failed reply means the requester is already gone.
        match message {
            ProviderMessage::ClientConnected(m) => {
                let verdict = self.connected(m.inner.connection_id, m.inner.endpoint_id);
                m.tx.send(verdict).await.ok();
            }
            ProviderMessage::ConnectionClosed(m) => self.closed(m.inner.connection_id),
            ProviderMessage::GetRequestReceived(m) => {
                let verdict = self.get(m.inner.connection_id, m.inner.request.hash);
                m.tx.send(verdict).await.ok();
            }
            ProviderMessage::GetManyRequestReceived(m) => {
                m.tx.send(refuse("GET_MANY", m.inner.connection_id)).await.ok();
            }
            ProviderMessage::PushRequestReceived(m) => {
                m.tx.send(refuse("PUSH", m.inner.connection_id)).await.ok();
            }
            ProviderMessage::ObserveRequestReceived(m) => {
                m.tx.send(refuse("OBSERVE", m.inner.connection_id)).await.ok();
            }
            // Not asked for by `MASK`; answered anyway so nothing can wait on us forever.
            ProviderMessage::Throttle(m) => {
                m.tx.send(Ok(())).await.ok();
            }
            ProviderMessage::ClientConnectedNotify(_)
            | ProviderMessage::GetRequestReceivedNotify(_)
            | ProviderMessage::GetManyRequestReceivedNotify(_)
            | ProviderMessage::PushRequestReceivedNotify(_)
            | ProviderMessage::ObserveRequestReceivedNotify(_) => {}
        }
    }

    /// A peer connected: remember who is on the connection, so its requests can be
    /// attributed. A connection that does not say who it is gets nothing.
    fn connected(&self, connection_id: u64, peer: Option<EndpointId>) -> EventResult {
        let peer = peer.ok_or(AbortReason::Permission)?;
        self.state().peers.insert(connection_id, peer);
        Ok(())
    }

    fn closed(&self, connection_id: u64) {
        self.state().peers.remove(&connection_id);
    }

    /// A GET on `connection_id` for `hash`: only the peer's own running Transfer.
    fn get(&self, connection_id: u64, hash: Hash) -> EventResult {
        let state = self.state();
        match state.peers.get(&connection_id) {
            Some(peer) if state.grants.contains_key(&(*peer, hash)) => Ok(()),
            peer => {
                // By Fingerprint: the Debug of an `EndpointId` is the whole ID.
                let peer = peer.map(|peer| DeviceId::from_endpoint_id(*peer));
                tracing::debug!(?peer, %hash, "GET refused: not an accepted Transfer of this peer");
                Err(AbortReason::Permission)
            }
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn refuse(kind: &str, connection_id: u64) -> EventResult {
    tracing::debug!(connection_id, "{kind} refused: the provider only serves GET");
    Err(AbortReason::Permission)
}

impl Drop for Grant {
    fn drop(&mut self) {
        if let Entry::Occupied(mut running) = self.gate.state().grants.entry(self.key) {
            *running.get_mut() -= 1;
            if *running.get() == 0 {
                running.remove();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use iroh::SecretKey;

    use super::*;

    fn peer() -> EndpointId {
        SecretKey::generate().public()
    }

    fn hash(n: u8) -> Hash {
        Hash::new([n])
    }

    #[test]
    fn a_granted_peer_may_get_its_root_and_nothing_else() {
        let gate = Arc::new(Gate::default());
        let (bob, carol) = (peer(), peer());
        let _bobs = gate.allow(bob, hash(1));
        let _carols = gate.allow(carol, hash(2));

        gate.connected(7, Some(bob)).unwrap();
        assert!(gate.get(7, hash(1)).is_ok());
        assert_eq!(gate.get(7, hash(2)), Err(AbortReason::Permission), "another peer's root");
        assert_eq!(gate.get(7, hash(3)), Err(AbortReason::Permission), "an unknown hash");
    }

    #[test]
    fn a_connection_that_does_not_say_who_it_is_is_refused() {
        let gate = Arc::new(Gate::default());
        let _grant = gate.allow(peer(), hash(1));

        assert_eq!(gate.connected(1, None), Err(AbortReason::Permission));
        // It is not remembered, so it cannot ask for anything either.
        assert_eq!(gate.get(1, hash(1)), Err(AbortReason::Permission));
    }

    #[test]
    fn a_peer_without_a_grant_gets_nothing() {
        let gate = Arc::new(Gate::default());
        let _bobs = gate.allow(peer(), hash(1));

        gate.connected(1, Some(peer())).unwrap();
        assert_eq!(gate.get(1, hash(1)), Err(AbortReason::Permission));
    }

    #[test]
    fn a_request_on_an_unknown_connection_is_refused() {
        let gate = Arc::new(Gate::default());
        let _grant = gate.allow(peer(), hash(1));
        assert_eq!(gate.get(9, hash(1)), Err(AbortReason::Permission));
    }

    #[test]
    fn dropping_the_grant_stops_requests_on_connections_already_open() {
        let gate = Arc::new(Gate::default());
        let bob = peer();
        let grant = gate.allow(bob, hash(1));
        gate.connected(1, Some(bob)).unwrap();
        assert!(gate.get(1, hash(1)).is_ok());

        drop(grant);

        assert_eq!(gate.get(1, hash(1)), Err(AbortReason::Permission));
    }

    #[test]
    fn two_transfers_of_the_same_content_to_one_peer_end_separately() {
        let gate = Arc::new(Gate::default());
        let bob = peer();
        let first = gate.allow(bob, hash(1));
        let second = gate.allow(bob, hash(1));
        gate.connected(1, Some(bob)).unwrap();

        drop(first);
        assert!(gate.get(1, hash(1)).is_ok(), "the second Transfer is still running");
        drop(second);
        assert_eq!(gate.get(1, hash(1)), Err(AbortReason::Permission));
        assert!(gate.state().grants.is_empty());
    }

    #[test]
    fn a_closed_connection_is_forgotten() {
        let gate = Arc::new(Gate::default());
        let bob = peer();
        let _grant = gate.allow(bob, hash(1));
        gate.connected(1, Some(bob)).unwrap();
        gate.closed(1);
        assert!(gate.state().peers.is_empty());
        assert_eq!(gate.get(1, hash(1)), Err(AbortReason::Permission));
    }
}
