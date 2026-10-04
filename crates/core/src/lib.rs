//! BhayanakShare core: everything except the windows. The Tauri shell and every test drive it
//! through the [`Device`] API: commands in, one ordered [`EventStream`] out.

pub mod clock;
mod db;
mod device;
mod error;
mod event;
mod fsmove;
mod identity;
mod names;
pub mod protocol;
mod receiver;
mod sender;
mod session;
pub mod store;
mod transfer;

pub use clock::{Clock, ManualClock, SystemClock, UnixMillis};
pub use db::{DbError, TransferRecord};
pub use device::{Device, DeviceAddr, DeviceConfig, Network};
pub use error::Error;
pub use event::{Event, EventKind, EventStream, TransferEvent};
pub use identity::{DEVICE_ID_LEN, DeviceId, DeviceIdError, KeySource};
pub use names::{NameError, validate_file_name};
pub use receiver::INCOMING_DIR;
pub use transfer::{Role, TransferId, TransferState};
