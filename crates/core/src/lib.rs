//! BhayanakShare core: everything except the windows. The Tauri shell and every test drive it
//! through the [`Device`] API: commands in, one ordered [`EventStream`] out.

pub mod clock;
mod contacts;
mod db;
mod device;
mod device_name;
mod error;
mod event;
mod fsmove;
mod gate;
mod identity;
mod names;
pub mod protocol;
mod receiver;
mod sender;
mod session;
mod space;
pub mod store;
mod transfer;

pub use clock::{Clock, ManualClock, SystemClock, UnixMillis};
pub use contacts::{Contact, KnownAddress, MAX_NAME_CHARS};
pub use db::{DbError, TransferRecord};
pub use device::{Device, DeviceAddr, DeviceConfig, Network};
pub use error::Error;
pub use event::{Event, EventKind, EventStream, ProgressEvent, TransferEvent};
pub use identity::{DEVICE_ID_LEN, DeviceId, DeviceIdError, KeySource};
pub use names::{NameError, validate_file_name};
pub use receiver::INCOMING_DIR;
pub use space::{FreeSpace, SpaceCheck, SystemFreeSpace};
pub use transfer::{OFFER_TTL_MS, Role, TransferId, TransferState};
