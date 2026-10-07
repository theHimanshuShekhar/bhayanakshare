//! BhayanakShare core: everything except the windows. The Tauri shell and every test drive it
//! through the [`Device`] API: commands in, one ordered [`EventStream`] out.

mod beacon;
pub mod clock;
mod contacts;
mod db;
mod device;
mod dht;
mod device_name;
mod discovery;
mod error;
mod event;
mod fsmove;
mod gate;
mod history;
mod identity;
mod identity_file;
mod keyfile;
mod keystore;
mod logs;
pub mod manifest;
mod names;
pub mod protocol;
mod receiver;
mod responder;
mod save_folder;
mod scan;
mod sender;
mod session;
mod space;
pub mod store;
mod transfer;

pub use clock::{Clock, ManualClock, SystemClock, UnixMillis};
pub use contacts::{Contact, KnownAddress, MAX_NAME_CHARS};
pub use db::{DbError, TransferRecord};
pub use device::{Device, DeviceAddr, DeviceConfig, Network, SentBatch};
pub use discovery::{NearbyDevice, Visibility};
pub use error::Error;
pub use event::{
    Event, EventKind, EventStream, NearbyEvent, Outdated, PreparingEvent, ProgressEvent, TransferEvent,
    VersionMismatchEvent,
};
pub use history::{HistoryEntry, HistoryQuery, HistoryTransfer};
pub use identity::{DEVICE_ID_LEN, DeviceId, DeviceIdError, KeySource};
pub use identity_file::IdentityFileError;
pub use keystore::KeyError;
pub use logs::{LogFiles, MAX_LOG_BYTES, log_filter};
pub use names::{NameError, validate_file_name};
pub use receiver::INCOMING_DIR;
pub use save_folder::SaveFolderProblem;
pub use space::{FreeSpace, SpaceCheck, SystemFreeSpace};
pub use transfer::{BatchId, OFFER_TTL_MS, Role, STALL_TTL_MS, TransferId, TransferKind, TransferState};
