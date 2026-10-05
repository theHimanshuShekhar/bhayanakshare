//! The Device API: the one seam the shell and every integration test use.
//!
//! A Device is created from a [`DeviceConfig`]. Commands are methods on [`Device`]; everything
//! that happens comes back, in order, on the single [`EventStream`] returned alongside it.

use std::{
    collections::HashMap,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
    time::Duration,
};

use iroh::{
    Endpoint, EndpointAddr, RelayMode, TransportAddr, address_lookup::MemoryLookup,
    endpoint::presets, protocol::Router,
};
use iroh_blobs::BlobsProtocol;
use tokio::sync::{Semaphore, oneshot};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use crate::{
    clock::{Clock, UnixMillis},
    contacts::{Contact, KnownAddress, clean_name},
    db::{Db, TransferRecord, Unfinished},
    device_name,
    discovery::{Discovery, NearbyDevice, Visibility},
    error::Error,
    event::{EventKind, EventSink, EventStream, ProgressEvent, TransferEvent},
    gate::Gate,
    identity::{DeviceId, KeySource},
    protocol, receiver, scan, sender,
    session::RESTARTED,
    space::{FreeSpace, SpaceCheck},
    store,
    transfer::{BatchId, OFFER_TTL_MS, Role, TransferId, TransferState},
};

/// Which network a Device lives on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
    /// Production: n0's public relays and address lookup.
    Internet,
    /// Tests: bind to 127.0.0.1 only, no relays, no address lookup, no LAN discovery. Other
    /// Devices are reached only through the addresses handed to the sender.
    Localhost,
    /// LAN discovery tests: like `Localhost`, but Devices also find each other over multicast
    /// on the loopback interface, as Devices on a real LAN do.
    LocalhostLan,
}

pub struct DeviceConfig {
    /// Settings, Transfer records, the Sender's blob store and (by default) the key.
    pub data_dir: PathBuf,
    /// Where accepted files land.
    pub save_dir: PathBuf,
    pub key_source: KeySource,
    pub clock: Arc<dyn Clock>,
    pub network: Network,
    /// How much room a save folder has; [`crate::SystemFreeSpace`] outside tests.
    pub free_space: Arc<dyn FreeSpace>,
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

/// What [`Device::send_batch`] made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentBatch {
    pub id: BatchId,
    /// One Transfer per Receiver, in the order the Receivers were given.
    pub transfers: Vec<TransferId>,
}

/// The answer a Receiver gives to an Offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Decision {
    /// Save into this folder, which has already been checked for room.
    Accept(PathBuf),
    Decline,
}

/// An Offer waiting for the user.
pub(crate) struct PendingOffer {
    pub peer: DeviceId,
    pub size: u64,
    /// Bytes in the longest path the Offer would create, names as adjusted, relative to the
    /// save folder: what the path-length check needs.
    pub longest_path: usize,
    pub decide: oneshot::Sender<Decision>,
}

/// Who and what a running Transfer is about; carried by every event it emits.
#[derive(Debug, Clone)]
pub(crate) struct TransferInfo {
    pub id: TransferId,
    pub role: Role,
    pub peer: DeviceId,
    /// What the peer called itself in its Hello, once known.
    pub peer_name: Option<String>,
    /// The first of `items`.
    pub name: String,
    pub size: u64,
    /// The names at the top of the Offer's tree.
    pub items: Vec<String>,
    pub file_count: u64,
    pub skipped_links: u32,
    /// Names the Receiver adjusted to make them safe to write.
    pub adjusted_names: u32,
    /// The Batch this Transfer was sent in; a Sender's only.
    pub batch: Option<BatchId>,
    /// When the Offer lapses if nobody has answered it, by this Device's clock.
    pub expires_at: UnixMillis,
}

impl TransferInfo {
    /// A Transfer read back from the database, or `None` if its peer cannot be read.
    pub(crate) fn from_record(record: &TransferRecord) -> Option<Self> {
        Some(Self {
            id: record.id,
            role: record.role,
            peer: record.peer.parse().ok()?,
            // Learned again from the peer's Hello when it next connects.
            peer_name: None,
            name: record.name.clone(),
            size: record.size,
            items: record.items.clone(),
            file_count: record.file_count,
            skipped_links: record.skipped_links,
            adjusted_names: record.adjusted_names,
            batch: record.batch_id,
            // Both sides begin timing an Offer when they record it.
            expires_at: record.created_at + OFFER_TTL_MS,
        })
    }
}

/// State shared by the Device handle and the tasks it spawns.
pub(crate) struct Shared {
    pub id: DeviceId,
    pub endpoint: Endpoint,
    pub blobs: iroh_blobs::api::Store,
    /// Decides who the blobs provider serves.
    pub gate: Arc<Gate>,
    /// Finds Nearby Devices and announces this one on the LAN.
    pub discovery: Discovery,
    pub db: Db,
    pub clock: Arc<dyn Clock>,
    /// The folder an Offer is saved to unless the Receiver picks another.
    pub save_dir: PathBuf,
    pub free_space: Arc<dyn FreeSpace>,
    pub events: EventSink,
    /// Offers waiting for the user, by Transfer ID.
    pub pending: Mutex<HashMap<TransferId, PendingOffer>>,
    /// Stops a running Transfer on this Device's side, by Transfer ID. A Transfer is listed
    /// from its Offer until it ends, or (on the Receiver) until it starts Saving, which is
    /// too late to stop.
    pub cancels: Mutex<HashMap<TransferId, CancellationToken>>,
    /// Offers this Device sent that expired, with where they went and what they held, so the
    /// user can send them again in one step.
    pub expired: Mutex<HashMap<TransferId, (DeviceAddr, Vec<PathBuf>)>>,
    /// Transfers this Device sends that are waiting for, or following, their Receiver, by
    /// Transfer ID: where a Receiver that dials back with `Resume` is handed over.
    pub resumers: Mutex<HashMap<TransferId, sender::Resumer>>,
    /// The download slots of each Batch with a Transfer running, which its Transfers share.
    /// Weak: a Batch's entry lapses with its last running Transfer.
    pub batch_slots: Mutex<HashMap<BatchId, Weak<Semaphore>>>,
    /// Addresses of other Devices this Device was told about, for dialling them.
    pub lookup: MemoryLookup,
    pub tasks: TaskTracker,
    /// Cancelled on shutdown; Transfer tasks stop, cleanup tasks run to the end.
    pub cancel: CancellationToken,
}

impl Shared {
    pub fn now(&self) -> UnixMillis {
        self.clock.now()
    }

    /// Makes a running Transfer cancellable and returns the token its task watches.
    pub fn track(&self, id: TransferId) -> CancellationToken {
        let token = CancellationToken::new();
        self.cancels.lock().unwrap_or_else(|e| e.into_inner()).insert(id, token.clone());
        token
    }

    pub fn untrack(&self, id: TransferId) {
        self.cancels.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    }

    /// The download slots the Transfers of `batch` share (a Transfer sent on its own has slots
    /// of its own, which nothing else competes for).
    pub fn slots(&self, batch: Option<BatchId>) -> Arc<Semaphore> {
        let fresh = || Arc::new(Semaphore::new(sender::MAX_DOWNLOADS));
        let Some(batch) = batch else { return fresh() };
        let mut all = self.batch_slots.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(slots) = all.get(&batch).and_then(Weak::upgrade) {
            return slots;
        }
        all.retain(|_, slots| slots.strong_count() > 0);
        let slots = fresh();
        all.insert(batch, Arc::downgrade(&slots));
        slots
    }

    /// What Transfer `id`, an Offer of `needed` bytes whose longest path is `longest_path`
    /// bytes, would take from `folder`.
    pub async fn space_check(
        &self,
        id: TransferId,
        needed: u64,
        longest_path: usize,
        folder: &Path,
    ) -> Result<SpaceCheck, Error> {
        let probe = self.free_space.clone();
        let dir = folder.to_owned();
        // The probe is a blocking call into the operating system.
        let free = tokio::task::spawn_blocking(move || {
            if !dir.is_dir() {
                return Err(Error::NotAFolder(dir));
            }
            match probe.available(&dir) {
                Ok(free) => Ok(Some(free)),
                Err(e) if e.kind() == std::io::ErrorKind::Unsupported => Ok(None),
                Err(e) => Err(Error::io(format!("checking free space in {}", dir.display()), e)),
            }
        })
        .await
        .map_err(|e| Error::io("checking free space", std::io::Error::other(e)))??;
        Ok(SpaceCheck { needed, free, paths_too_long: !receiver::paths_fit(folder, id, longest_path) })
    }

    /// This Device's name as it is announced to others: the stored Device Name, else the
    /// hostname.
    pub async fn device_name(&self) -> String {
        match self.db.setting(device_name::SETTING).await {
            Ok(Some(name)) => device_name::sanitize(&name),
            Ok(None) => None,
            Err(e) => {
                tracing::warn!("could not read the Device Name: {e}");
                None
            }
        }
        .unwrap_or_else(device_name::default_name)
    }

    /// Notes how a connection to `peer` is reaching it and what it calls itself, if `peer` is a
    /// Contact. A failure to save it is logged: it is not worth failing a Transfer for.
    pub async fn remember_peer(
        &self,
        peer: DeviceId,
        conn: &iroh::endpoint::Connection,
        peer_name: Option<String>,
    ) {
        let seen = KnownAddress::of_connection(conn);
        if let Err(e) = self.db.update_contact_connection(peer, seen, peer_name).await {
            tracing::warn!("could not record a Contact's address and name: {e}");
        }
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
                items: t.items.clone(),
                file_count: t.file_count,
                skipped_links: t.skipped_links,
                adjusted_names: t.adjusted_names,
                batch_id: t.batch,
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

    /// Announces how many bytes of the Transfer's file have been received.
    pub fn progress(&self, t: &TransferInfo, bytes: u64) {
        let progress = ProgressEvent { transfer_id: t.id, bytes: bytes.min(t.size), total: t.size };
        self.events.emit(self.now(), EventKind::Progress(progress));
    }

    fn announce(&self, t: &TransferInfo, state: TransferState, now: UnixMillis) {
        self.events.emit(
            now,
            EventKind::Transfer(TransferEvent {
                transfer_id: t.id,
                role: t.role,
                peer: t.peer,
                peer_name: t.peer_name.clone(),
                name: t.name.clone(),
                size: t.size,
                items: t.items.clone(),
                file_count: t.file_count,
                skipped_links: t.skipped_links,
                adjusted_names: t.adjusted_names,
                batch_id: t.batch,
                expires_at: t.expires_at,
                state,
            }),
        );
    }
}

/// The direct addresses an endpoint is listening on, falling back to what it is bound to
/// while it has not learned any yet.
pub(crate) fn direct_addrs(endpoint: &Endpoint) -> Vec<SocketAddr> {
    let direct: Vec<SocketAddr> = endpoint.addr().ip_addrs().copied().collect();
    if direct.is_empty() { endpoint.bound_sockets() } else { direct }
}

/// Takes up the Transfers the last run left unfinished (spec section 4):
///
/// - An Offer nobody answered does not survive a restart: it expires, on either side.
/// - An accepted Transfer carries on. A Sender whose content was hashed waits for the
///   Receiver to dial back; a Receiver that has its save folder redials the Sender. The 24
///   hours without progress count from the last progress made before the restart.
/// - Anything else cannot be resumed and fails with a reason: an accepted Transfer whose
///   content hash had not been exchanged yet (a Sender that restarted while still hashing, a
///   Receiver that had not been told the hash).
///
/// Only the database is told about the ones that end: nothing was shown to the user in this
/// run, so there is no event to send.
async fn recover(sh: &Arc<Shared>) -> Result<(), Error> {
    for Unfinished { record, progress_at, root, save_dir } in sh.db.unfinished().await? {
        let resumed = match (record.role, &record.state, TransferInfo::from_record(&record), root) {
            (
                Role::Sender,
                // A Waiting Transfer with a hash is one whose Receiver came back from a lost
                // connection and was queued for a slot again; one still waiting for its first
                // slot has no hash, and fails like any other without one.
                TransferState::Accepted | TransferState::Waiting | TransferState::Transferring,
                Some(info),
                Some(root),
            ) => {
                sender::recover(sh, info, root, progress_at);
                true
            }
            (
                Role::Receiver,
                TransferState::Accepted
                | TransferState::Transferring
                | TransferState::Reconnecting
                | TransferState::Saving,
                Some(info),
                Some(root),
            ) => match (save_dir, sh.db.manifest(record.id).await?) {
                (Some(save_dir), Some(manifest)) => {
                    receiver::recover(sh, info, root, save_dir, manifest, progress_at);
                    true
                }
                _ => false,
            },
            _ => false,
        };
        if !resumed {
            let state = match record.state {
                TransferState::Offered => TransferState::Expired,
                _ => TransferState::Failed { reason: RESTARTED.into() },
            };
            sh.db.update_transfer(record.id, state, sh.now()).await?;
        }
    }
    Ok(())
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
        let DeviceConfig { data_dir, save_dir, key_source, clock, network, free_space } = config;
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

        let lookup = MemoryLookup::new();
        let endpoint = match network {
            Network::Internet => Endpoint::builder(presets::N0),
            Network::Localhost | Network::LocalhostLan => Endpoint::builder(presets::Minimal)
                .relay_mode(RelayMode::Disabled)
                .clear_ip_transports()
                .bind_addr("127.0.0.1:0")
                .map_err(|e| Error::network("binding", e))?,
        }
        .address_lookup(lookup.clone())
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
            discovery: Discovery::new(network, &endpoint),
            db,
            clock,
            save_dir,
            free_space,
            events,
            pending: Mutex::default(),
            cancels: Mutex::default(),
            expired: Mutex::default(),
            resumers: Mutex::default(),
            batch_slots: Mutex::default(),
            lookup,
            tasks: TaskTracker::new(),
            cancel: CancellationToken::new(),
        });
        // Before the Device accepts anyone: a Receiver dialling back to resume must find its
        // Transfer already waiting for it.
        recover(&shared).await?;
        let router = Router::builder(endpoint)
            .accept(protocol::ALPN, receiver::Handler::new(shared.clone()))
            .accept(iroh_blobs::ALPN, BlobsProtocol::new(&blobs, Some(gate.events())))
            .spawn();
        if network != Network::Localhost {
            shared.discovery.start(&shared).await;
        }

        let inner = Inner { shared, router, store: Mutex::new(Some(store)) };
        Ok((Self { inner: Arc::new(inner) }, stream))
    }

    pub fn device_id(&self) -> DeviceId {
        self.inner.shared.id
    }
    /// This Device's ID plus the direct addresses it is listening on.
    pub fn addr(&self) -> DeviceAddr {
        DeviceAddr { id: self.device_id(), direct: direct_addrs(&self.inner.shared.endpoint) }
    }

    /// Offers the file or folder at `path` to the Device at `to`. Returns once the Transfer
    /// exists; everything after that is reported on the event stream.
    pub async fn send_file(
        &self,
        to: impl Into<DeviceAddr>,
        path: &Path,
    ) -> Result<TransferId, Error> {
        self.send(to, &[path.to_owned()]).await
    }

    /// Offers the files and folders at `paths` to the Device at `to`, as one Transfer: the
    /// Receiver gets each under its own name, with folders' structure, empty folders,
    /// modification times and executable bits kept. Symlinks inside a folder are skipped and
    /// counted. A selection that breaks a limit, or has names that cannot be sent, is refused
    /// here, before anything is sent.
    pub async fn send(
        &self,
        to: impl Into<DeviceAddr>,
        paths: &[PathBuf],
    ) -> Result<TransferId, Error> {
        Ok(self.send_to(&[to.into()], paths, None).await?.remove(0))
    }

    /// Offers the files and folders at `paths` to every Device in `to` at once: a Batch, with
    /// one Transfer for each, in the order given. Each is an ordinary Transfer to its
    /// Receiver, whose Offer says nothing about the others; the files are hashed once, and
    /// at most 3 Receivers download at a time (the rest that accepted show Waiting). Every
    /// Transfer then succeeds, fails or is cancelled on its own. A selection that cannot be
    /// sent is refused here, before anything is sent.
    pub async fn send_batch(&self, to: &[DeviceAddr], paths: &[PathBuf]) -> Result<SentBatch, Error> {
        if to.is_empty() {
            return Err(Error::NoReceivers);
        }
        if let Some(twice) = to.iter().enumerate().find(|(i, a)| to[..*i].iter().any(|b| b.id == a.id)) {
            return Err(Error::DuplicateReceiver(twice.1.id));
        }
        let id = BatchId::random();
        let transfers = self.send_to(to, paths, Some(id)).await?;
        Ok(SentBatch { id, transfers })
    }

    /// Sends a Failed Transfer of a Batch again, to the same Receiver, as a new Transfer in the
    /// same Batch with a new Offer. Only a Transfer this Device sent in a Batch and that
    /// failed can be: a Declined one is the Receiver's answer, and one that is not over is
    /// still going. The Receiver is dialled by its ID, so a Device that cannot be found by
    /// its ID alone must have been given an address (`note_address`).
    pub async fn retry(&self, id: TransferId) -> Result<TransferId, Error> {
        let sh = &self.inner.shared;
        let not = |why: &'static str| Error::NotRetryable(id, why);
        let record = sh.db.transfer(id).await?.ok_or_else(|| not("this Device has no such Transfer"))?;
        if record.role != Role::Sender {
            return Err(not("this Device did not send it"));
        }
        let batch = record.batch_id.ok_or_else(|| not("it was not sent in a Batch"))?;
        match record.state {
            TransferState::Failed { .. } => {}
            TransferState::Declined => return Err(not("the Receiver declined it")),
            _ => return Err(not("only a Failed Transfer can be retried")),
        }
        let peer: DeviceId = record.peer.parse().map_err(|_| not("its Receiver is unknown"))?;
        // Oldest first: a later Transfer to the same Receiver in the Batch is a retry of this.
        let attempts = sh.db.batch_transfers(batch).await?;
        if attempts.iter().skip_while(|t| t.id != id).skip(1).any(|t| t.peer == record.peer) {
            return Err(not("it has been retried already"));
        }
        let roots =
            sh.db.batch_roots(batch).await?.ok_or_else(|| not("what it sent is no longer known"))?;
        Ok(self.send_to(&[DeviceAddr::from(peer)], &roots, Some(batch)).await?[0])
    }

    /// Scans `paths` once and starts a Transfer to each of `to`, in `batch` if there is one.
    async fn send_to(
        &self,
        to: &[DeviceAddr],
        paths: &[PathBuf],
        batch: Option<BatchId>,
    ) -> Result<Vec<TransferId>, Error> {
        let sh = &self.inner.shared;
        if sh.cancel.is_cancelled() {
            return Err(Error::ShuttingDown);
        }
        let roots = paths
            .iter()
            .map(|path| std::path::absolute(path).map_err(|e| Error::io("resolving the path", e)))
            .collect::<Result<Vec<_>, _>>()?;
        let scanned = roots.clone();
        let scan = tokio::task::spawn_blocking(move || scan::scan(&scanned))
            .await
            .map_err(|e| Error::io("reading the files", std::io::Error::other(e)))??;

        if let Some(batch) = batch {
            sh.db.insert_batch(batch, &roots).await?;
        }

        let items = scan.manifest.top_level_items();
        let template = TransferInfo {
            id: TransferId::random(),
            role: Role::Sender,
            peer: to[0].id,
            peer_name: None,
            name: items[0].clone(),
            // The scan has been validated, so the total fits.
            size: scan.manifest.total_size().unwrap_or(u64::MAX),
            items,
            file_count: scan.manifest.file_count(),
            skipped_links: scan.skipped_links,
            adjusted_names: 0,
            batch,
            expires_at: sh.now() + OFFER_TTL_MS,
        };
        let sources = scan.sources.clone();
        let out = Arc::new(sender::Outgoing::new(scan, roots, sh.slots(batch)));
        let mut ids = Vec::with_capacity(to.len());
        for to in to {
            let info = TransferInfo { id: TransferId::random(), peer: to.id, ..template.clone() };
            sh.begin(&info, TransferState::Offered).await?;
            // The files as they are now, for the check before they are served again.
            sh.db.insert_sources(info.id, sources.clone()).await?;
            ids.push(info.id);
            let cancel = sh.track(info.id);
            sh.tasks.spawn(sender::run(sh.clone(), info, to.clone(), out.clone(), cancel));
        }
        Ok(ids)
    }

    /// Sends an Offer that expired again, to the same Device, as a new Transfer. Fails if the
    /// files have gone in the meantime.
    pub async fn resend(&self, id: TransferId) -> Result<TransferId, Error> {
        let sh = &self.inner.shared;
        let (to, paths) = sh
            .expired
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
            .cloned()
            .ok_or(Error::NothingToResend(id))?;
        let new = self.send(to, &paths).await?;
        sh.expired.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        Ok(new)
    }

    /// Stops a Transfer on either side, any time before it starts Saving. The other Device is
    /// told and shows it Cancelled; a Receiver deletes what it has received.
    pub async fn cancel(&self, id: TransferId) -> Result<(), Error> {
        let token = self
            .inner
            .shared
            .cancels
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
            .cloned()
            .ok_or(Error::NotRunning(id))?;
        token.cancel();
        Ok(())
    }

    /// Stops every Transfer of a Batch that is still running, each as `cancel` would. The ones
    /// that already ended stay as they ended.
    pub async fn cancel_batch(&self, batch: BatchId) -> Result<(), Error> {
        let sh = &self.inner.shared;
        let transfers = sh.db.batch_transfers(batch).await?;
        if transfers.is_empty() {
            return Err(Error::UnknownBatch(batch));
        }
        let running = sh.cancels.lock().unwrap_or_else(|e| e.into_inner());
        for transfer in transfers {
            if let Some(token) = running.get(&transfer.id) {
                token.cancel();
            }
        }
        Ok(())
    }

    /// Whether a pending Offer fits in `folder` (the save folder when `None`).
    pub async fn check_offer(
        &self,
        id: TransferId,
        folder: Option<&Path>,
    ) -> Result<SpaceCheck, Error> {
        self.check_space(id, folder).await.map(|(_, check)| check)
    }

    /// Accepts a pending Offer into the save folder; the content is then fetched and saved.
    /// Fails, leaving the Offer pending, if it does not fit there.
    pub async fn accept(&self, id: TransferId) -> Result<(), Error> {
        self.accept_into(id, None).await
    }

    /// Accepts a pending Offer into `folder` for this Transfer only (the save folder when
    /// `None`). Fails, leaving the Offer pending, if it does not fit there or its paths would
    /// be too long.
    pub async fn accept_into(&self, id: TransferId, folder: Option<&Path>) -> Result<(), Error> {
        let (folder, check) = self.check_space(id, folder).await?;
        if let Some(free) = check.free.filter(|_| !check.fits()) {
            return Err(Error::NotEnoughSpace { needed: check.needed, free });
        }
        if check.paths_too_long {
            return Err(Error::PathsTooLong);
        }
        self.decide(id, Decision::Accept(folder))
    }

    /// The absolute `folder` and what a pending Offer would need from it.
    async fn check_space(
        &self,
        id: TransferId,
        folder: Option<&Path>,
    ) -> Result<(PathBuf, SpaceCheck), Error> {
        let sh = &self.inner.shared;
        let (needed, longest_path) = sh
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
            .map(|offer| (offer.size, offer.longest_path))
            .ok_or(Error::UnknownTransfer(id))?;
        let folder = match folder {
            Some(folder) => std::path::absolute(folder).map_err(|e| Error::io("resolving the folder", e))?,
            None => sh.save_dir.clone(),
        };
        let check = sh.space_check(id, needed, longest_path, &folder).await?;
        Ok((folder, check))
    }

    /// Declines a pending Offer. Nothing is saved.
    pub async fn decline(&self, id: TransferId) -> Result<(), Error> {
        self.decide(id, Decision::Decline)
    }

    fn decide(&self, id: TransferId, decision: Decision) -> Result<(), Error> {
        let offer = self
            .inner
            .shared
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id)
            .ok_or(Error::UnknownTransfer(id))?;
        // The Offer's task may have just ended (the Sender went away); treat it as gone.
        offer.decide.send(decision).map_err(|_| Error::UnknownTransfer(id))
    }

    /// This Device's name as other Devices see it: the Device Name if set, else the hostname.
    pub async fn device_name(&self) -> String {
        self.inner.shared.device_name().await
    }

    /// Renames this Device. The new name is announced from the next connection on, and on the
    /// LAN straight away if the Visibility announces it. Returns the name as stored (trimmed,
    /// and shortened if it was too long).
    pub async fn set_device_name(&self, name: &str) -> Result<String, Error> {
        let name = device_name::sanitize(name).ok_or(Error::EmptyDeviceName)?;
        let sh = &self.inner.shared;
        sh.db.set_setting(device_name::SETTING, &name).await?;
        sh.discovery.refresh(sh).await;
        Ok(name)
    }

    /// Who can see this Device as a Nearby Device. People who have its ID until changed.
    pub async fn visibility(&self) -> Visibility {
        Visibility::load(&self.inner.shared.db).await
    }

    /// Changes who can see this Device as a Nearby Device, and applies it straight away. It
    /// governs discovery only: anyone with the Device ID can still send to it.
    pub async fn set_visibility(&self, visibility: Visibility) -> Result<(), Error> {
        let sh = &self.inner.shared;
        if sh.cancel.is_cancelled() {
            return Err(Error::ShuttingDown);
        }
        visibility.store(&sh.db).await?;
        sh.discovery.refresh(sh).await;
        Ok(())
    }

    /// The Devices found on the LAN right now, whether or not they are Contacts. Changes
    /// arrive on the event stream as `Nearby` events, each holding the whole list.
    pub fn nearby(&self) -> Vec<NearbyDevice> {
        self.inner.shared.discovery.nearby()
    }

    /// Saves the Device with ID `id` as a Contact. `device_name` is the name it goes by, as far
    /// as the user knows it (a share link suggests one). The caller is expected to have had
    /// the user check the Fingerprint with the owner first.
    pub async fn add_contact(
        &self,
        id: DeviceId,
        device_name: Option<&str>,
    ) -> Result<Contact, Error> {
        let sh = &self.inner.shared;
        if id == sh.id {
            return Err(Error::OwnDeviceId);
        }
        let contact = Contact {
            id,
            nickname: None,
            device_name: clean_name(device_name).map_err(Error::InvalidContactName)?,
            auto_accept: false,
            last_known_address: KnownAddress::default(),
            added_at: sh.now(),
        };
        if !sh.db.insert_contact(contact.clone()).await? {
            return Err(Error::AlreadyContact(id));
        }
        sh.discovery.contacts_changed();
        Ok(contact)
    }

    /// Every Contact, in the order they were added.
    pub async fn contacts(&self) -> Result<Vec<Contact>, Error> {
        Ok(self.inner.shared.db.contacts().await?)
    }

    /// Sets the name this Device shows for a Contact; `None` (or an empty name) goes back to
    /// the Contact's own Device Name.
    pub async fn set_nickname(&self, id: DeviceId, nickname: Option<&str>) -> Result<Contact, Error> {
        let nickname = clean_name(nickname).map_err(Error::InvalidContactName)?;
        if !self.inner.shared.db.set_contact_nickname(id, nickname).await? {
            return Err(Error::UnknownContact(id));
        }
        self.contact(id).await
    }

    /// Turns Auto-accept on or off for a Contact.
    pub async fn set_auto_accept(&self, id: DeviceId, on: bool) -> Result<Contact, Error> {
        if !self.inner.shared.db.set_contact_auto_accept(id, on).await? {
            return Err(Error::UnknownContact(id));
        }
        self.contact(id).await
    }

    /// Forgets a Contact. Its Transfers stay in the Transfer records.
    pub async fn remove_contact(&self, id: DeviceId) -> Result<(), Error> {
        if !self.inner.shared.db.delete_contact(id).await? {
            return Err(Error::UnknownContact(id));
        }
        self.inner.shared.discovery.contacts_changed();
        Ok(())
    }

    async fn contact(&self, id: DeviceId) -> Result<Contact, Error> {
        self.inner.shared.db.contact(id).await?.ok_or(Error::UnknownContact(id))
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

    /// Tells this Device where to find another one, for when it dials it and discovery cannot
    /// say: the address is kept for this run and tried alongside whatever discovery finds.
    pub fn note_address(&self, addr: DeviceAddr) {
        self.inner.shared.lookup.add_endpoint_info(addr.to_endpoint_addr());
    }

    /// Stops the Device cleanly: Transfers in progress are left as they are, to resume on the
    /// next start, and every store they use is shut down, which flushes what it holds to
    /// disk; then the network endpoint closes. Flushing can take a long time after a large
    /// download, so this gives up waiting after `deadline` and returns anyway: stopping the
    /// process then is like a crash, which is safe but makes the next start re-check what was
    /// downloaded. Safe to call more than once.
    pub async fn shutdown(&self, deadline: Duration) {
        let sh = &self.inner.shared;
        let until = tokio::time::Instant::now() + deadline;
        sh.cancel.cancel();
        sh.tasks.close();
        let mut closed = true;
        if tokio::time::timeout_at(until, sh.tasks.wait()).await.is_err() {
            tracing::warn!("shutdown gave up on Transfer stores after {deadline:?}; they recover on the next start");
            closed = false;
        }
        sh.discovery.shutdown().await;
        // The router also shuts the Sender's store down.
        match tokio::time::timeout_at(until, self.inner.router.shutdown()).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!("network shutdown: {e}"),
            Err(_) => {
                tracing::warn!("shutdown gave up on the network after {deadline:?}");
                closed = false;
            }
        }
        // Release the Sender's store directory only once its store has really closed, or a
        // Device started on the same folders would open it a second time, which hangs.
        if closed {
            self.inner.store.lock().unwrap_or_else(|e| e.into_inner()).take();
        }
    }
}
