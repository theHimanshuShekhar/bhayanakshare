//! The Device API: the one seam the shell and every integration test use.
//!
//! A Device is created from a [`DeviceConfig`]. Commands are methods on [`Device`]; everything
//! that happens comes back, in order, on the single [`EventStream`] returned alongside it.

use std::{
    collections::HashMap,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use iroh::{
    Endpoint, EndpointAddr, RelayMode, TransportAddr, endpoint::presets, protocol::Router,
};
use iroh_blobs::BlobsProtocol;
use tokio::sync::oneshot;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use crate::{
    clock::{Clock, UnixMillis},
    db::{Db, TransferRecord},
    error::Error,
    event::{EventKind, EventSink, EventStream, TransferEvent},
    gate::Gate,
    identity::{DeviceId, KeySource},
    names::validate_file_name,
    protocol, receiver, sender, store,
    transfer::{Role, TransferId, TransferState},
};

/// Which network a Device lives on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
    /// Production: n0's public relays and address lookup.
    Internet,
    /// Tests: bind to 127.0.0.1 only, no relays, no address lookup. Other Devices are
    /// reached only through the addresses handed to the sender.
    Localhost,
}

pub struct DeviceConfig {
    /// Settings, Transfer records, the Sender's blob store and (by default) the key.
    pub data_dir: PathBuf,
    /// Where accepted files land.
    pub save_dir: PathBuf,
    pub key_source: KeySource,
    pub clock: Arc<dyn Clock>,
    pub network: Network,
}

/// How to reach a Device: its ID, plus direct socket addresses if they are known. The ID alone
/// is enough when discovery can resolve it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceAddr {
    pub id: DeviceId,
    pub direct: Vec<SocketAddr>,
}

impl From<DeviceId> for DeviceAddr {
    fn from(id: DeviceId) -> Self {
        Self { id, direct: Vec::new() }
    }
}

impl DeviceAddr {
    pub(crate) fn to_endpoint_addr(&self) -> EndpointAddr {
        EndpointAddr::from_parts(
            self.id.endpoint_id(),
            self.direct.iter().copied().map(TransportAddr::Ip),
        )
    }
}

/// The answer a Receiver gives to an Offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Decision {
    Accept,
    Decline,
}

/// Who and what a running Transfer is about; carried by every event it emits.
#[derive(Debug, Clone)]
pub(crate) struct TransferInfo {
    pub id: TransferId,
    pub role: Role,
    pub peer: DeviceId,
    pub name: String,
    pub size: u64,
}

/// State shared by the Device handle and the tasks it spawns.
pub(crate) struct Shared {
    pub id: DeviceId,
    pub endpoint: Endpoint,
    pub blobs: iroh_blobs::api::Store,
    /// Decides who the blobs provider serves.
    pub gate: Arc<Gate>,
    pub db: Db,
    pub clock: Arc<dyn Clock>,
    pub save_dir: PathBuf,
    pub events: EventSink,
    /// Offers waiting for the user, by Transfer ID.
    pub pending: Mutex<HashMap<TransferId, oneshot::Sender<Decision>>>,
    pub tasks: TaskTracker,
    /// Cancelled on shutdown; Transfer tasks stop, cleanup tasks run to the end.
    pub cancel: CancellationToken,
}

impl Shared {
    pub fn now(&self) -> UnixMillis {
        self.clock.now()
    }

    /// Records a new Transfer and announces it.
    pub async fn begin(&self, t: &TransferInfo, state: TransferState) -> Result<(), Error> {
        let now = self.now();
        self.db
            .insert_transfer(TransferRecord {
                id: t.id,
                role: t.role,
                peer: t.peer.to_string(),
                name: t.name.clone(),
                size: t.size,
                state: state.clone(),
                created_at: now,
                updated_at: now,
            })
            .await?;
        self.announce(t, state, now);
        Ok(())
    }

    /// Persists a state change, then announces it. A database failure is logged, not fatal:
    /// the Transfer itself can still finish.
    pub async fn transition(&self, t: &TransferInfo, state: TransferState) {
        let now = self.now();
        if let Err(e) = self.db.update_transfer(t.id, state.clone(), now).await {
            tracing::warn!(transfer = %t.id, "could not record Transfer state: {e}");
        }
        self.announce(t, state, now);
    }

    fn announce(&self, t: &TransferInfo, state: TransferState, now: UnixMillis) {
        self.events.emit(
            now,
            EventKind::Transfer(TransferEvent {
                transfer_id: t.id,
                role: t.role,
                peer: t.peer,
                name: t.name.clone(),
                size: t.size,
                state,
            }),
        );
    }
}

struct Inner {
    shared: Arc<Shared>,
    router: Router,
    /// The Sender's global blob store; released at shutdown.
    store: Mutex<Option<store::Store>>,
}

/// A running Device. Cheap to clone; all clones are the same Device.
#[derive(Clone)]
pub struct Device {
    inner: Arc<Inner>,
}

impl Device {
    /// Starts a Device: opens its database and blob store, loads (or creates) its key, binds
    /// the network endpoint and starts accepting.
    pub async fn start(config: DeviceConfig) -> Result<(Self, EventStream), Error> {
        let DeviceConfig { data_dir, save_dir, key_source, clock, network } = config;
        let io = |what: &'static str, dir: &Path| {
            let ctx = format!("{what} {}", dir.display());
            move |e| Error::io(ctx, e)
        };
        tokio::fs::create_dir_all(&data_dir).await.map_err(io("creating", &data_dir))?;
        tokio::fs::create_dir_all(&save_dir).await.map_err(io("creating", &save_dir))?;
        let save_dir = std::path::absolute(&save_dir).map_err(io("resolving", &save_dir))?;

        let secret = key_source
            .load_or_create()
            .map_err(|e| Error::io("loading the secret key", e))?;
        let db = Db::open(&data_dir.join("bhayanakshare.db")).await?;
        let store = store::open(&data_dir.join("blobs")).await?;
        let blobs: iroh_blobs::api::Store = (**store).clone();

        let endpoint = match network {
            Network::Internet => Endpoint::builder(presets::N0),
            Network::Localhost => Endpoint::builder(presets::Minimal)
                .relay_mode(RelayMode::Disabled)
                .clear_ip_transports()
                .bind_addr("127.0.0.1:0")
                .map_err(|e| Error::network("binding", e))?,
        }
        .secret_key(secret)
        .bind()
        .await
        .map_err(|e| Error::network("binding the network endpoint", e))?;

        let (events, stream) = EventSink::new();
        let gate = Arc::new(Gate::default());
        let shared = Arc::new(Shared {
            id: DeviceId::from_endpoint_id(endpoint.id()),
            endpoint: endpoint.clone(),
            blobs: blobs.clone(),
            gate: gate.clone(),
            db,
            clock,
            save_dir,
            events,
            pending: Mutex::default(),
            tasks: TaskTracker::new(),
            cancel: CancellationToken::new(),
        });
        let router = Router::builder(endpoint)
            .accept(protocol::ALPN, receiver::Handler::new(shared.clone()))
            .accept(iroh_blobs::ALPN, BlobsProtocol::new(&blobs, Some(gate.events())))
            .spawn();

        let inner = Inner { shared, router, store: Mutex::new(Some(store)) };
        Ok((Self { inner: Arc::new(inner) }, stream))
    }

    pub fn device_id(&self) -> DeviceId {
        self.inner.shared.id
    }

    /// This Device's ID plus the direct addresses it is listening on.
    pub fn addr(&self) -> DeviceAddr {
        let endpoint = &self.inner.shared.endpoint;
        let mut direct: Vec<SocketAddr> = endpoint.addr().ip_addrs().copied().collect();
        if direct.is_empty() {
            direct = endpoint.bound_sockets();
        }
        DeviceAddr { id: self.device_id(), direct }
    }

    /// Offers the file at `path` to the Device at `to`. Returns once the Transfer exists;
    /// everything after that is reported on the event stream.
    pub async fn send_file(
        &self,
        to: impl Into<DeviceAddr>,
        path: &Path,
    ) -> Result<TransferId, Error> {
        let sh = &self.inner.shared;
        if sh.cancel.is_cancelled() {
            return Err(Error::ShuttingDown);
        }
        let to = to.into();
        let path = std::path::absolute(path).map_err(|e| Error::io("resolving the path", e))?;
        let meta = tokio::fs::metadata(&path).await.map_err(|_| Error::NotAFile(path.clone()))?;
        if !meta.is_file() {
            return Err(Error::NotAFile(path));
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| Error::NotAFile(path.clone()))?
            .to_owned();
        validate_file_name(&name)?;

        let info = TransferInfo {
            id: TransferId::random(),
            role: Role::Sender,
            peer: to.id,
            name,
            size: meta.len(),
        };
        sh.begin(&info, TransferState::Offered).await?;
        let id = info.id;
        let sh = sh.clone();
        self.inner.shared.tasks.spawn(sender::run(sh, info, to, path));
        Ok(id)
    }

    /// Accepts a pending Offer; the content is then fetched and saved.
    pub async fn accept(&self, id: TransferId) -> Result<(), Error> {
        self.decide(id, Decision::Accept)
    }

    /// Declines a pending Offer. Nothing is saved.
    pub async fn decline(&self, id: TransferId) -> Result<(), Error> {
        self.decide(id, Decision::Decline)
    }

    fn decide(&self, id: TransferId, decision: Decision) -> Result<(), Error> {
        let tx = self
            .inner
            .shared
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id)
            .ok_or(Error::UnknownTransfer(id))?;
        // The Offer's task may have just ended (the Sender went away); treat it as gone.
        tx.send(decision).map_err(|_| Error::UnknownTransfer(id))
    }

    /// A persisted setting, if it has been set.
    pub async fn setting(&self, key: &str) -> Result<Option<String>, Error> {
        Ok(self.inner.shared.db.setting(key).await?)
    }

    pub async fn set_setting(&self, key: &str, value: &str) -> Result<(), Error> {
        Ok(self.inner.shared.db.set_setting(key, value).await?)
    }

    /// Every Transfer this Device has sent or received, oldest first.
    pub async fn transfers(&self) -> Result<Vec<TransferRecord>, Error> {
        Ok(self.inner.shared.db.transfers().await?)
    }

    /// Stops Transfers in progress, waits for store cleanup to finish and closes the network
    /// endpoint. Safe to call more than once.
    pub async fn shutdown(&self) {
        let sh = &self.inner.shared;
        sh.cancel.cancel();
        sh.tasks.close();
        sh.tasks.wait().await;
        if let Err(e) = self.inner.router.shutdown().await {
            tracing::warn!("network shutdown: {e}");
        }
        // The router has shut the Sender's store down; now release its directory.
        self.inner.store.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}
