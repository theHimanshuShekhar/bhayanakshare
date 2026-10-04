//! Transfer vocabulary shared by the protocol, persistence and the Device API.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A random 128-bit Transfer ID, chosen by the Sender. Hex in text and JSON, raw bytes on
/// the wire.
#[derive(Clone, Copy, PartialEq, Eq, Hash, specta::Type)]
#[specta(type = String)] // serialized as hex in JSON
pub struct TransferId([u8; 16]);

impl TransferId {
    pub fn random() -> Self {
        Self(rand::random())
    }

    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl fmt::Display for TransferId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&data_encoding::HEXLOWER.encode(&self.0))
    }
}

impl fmt::Debug for TransferId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TransferId({self})")
    }
}

impl FromStr for TransferId {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        let bytes = data_encoding::HEXLOWER.decode(s.as_bytes()).map_err(|_| ())?;
        Ok(Self(bytes.try_into().map_err(|_| ())?))
    }
}

impl Serialize for TransferId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if s.is_human_readable() { s.collect_str(self) } else { self.0.serialize(s) }
    }
}

impl<'de> Deserialize<'de> for TransferId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        if d.is_human_readable() {
            let text = String::deserialize(d)?;
            text.parse().map_err(|()| serde::de::Error::custom("invalid transfer id"))
        } else {
            Ok(Self(<[u8; 16]>::deserialize(d)?))
        }
    }
}

/// Which side of a Transfer a Device plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Sender,
    Receiver,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sender => "sender",
            Self::Receiver => "receiver",
        }
    }
}

impl FromStr for Role {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "sender" => Ok(Self::Sender),
            "receiver" => Ok(Self::Receiver),
            _ => Err(()),
        }
    }
}

/// How long an Offer waits for an answer before it expires (spec section 4).
pub const OFFER_TTL_MS: i64 = 10 * 60 * 1000;

/// How long a Transfer may go without progress before both sides give up on it (spec
/// section 4).
pub const STALL_TTL_MS: i64 = 24 * 60 * 60 * 1000;

/// Where a Transfer is in its lifecycle (spec section 4, the part the skeleton covers).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransferState {
    Offered,
    Accepted,
    Declined,
    /// The Receiver is fetching the content.
    Transferring,
    /// The Receiver lost the Sender part-way through and is redialling it (Receiver only;
    /// the Sender just keeps showing Transferring).
    Reconnecting,
    /// The Receiver has everything and is moving it into the save folder.
    Saving,
    /// `saved_to` is set on the Receiver only.
    Completed { saved_to: Option<String> },
    Failed { reason: String },
    /// Nobody answered the Offer within 10 minutes ([`OFFER_TTL_MS`]).
    Expired,
    /// One side stopped the Transfer before it completed; `by` is which.
    Cancelled { by: Role },
}

impl TransferState {
    /// The name stored in the database.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Offered => "offered",
            Self::Accepted => "accepted",
            Self::Declined => "declined",
            Self::Transferring => "transferring",
            Self::Reconnecting => "reconnecting",
            Self::Saving => "saving",
            Self::Completed { .. } => "completed",
            Self::Failed { .. } => "failed",
            Self::Expired => "expired",
            Self::Cancelled { .. } => "cancelled",
        }
    }

    /// The detail columns stored next to the label: `(saved_to, error)`. A Cancelled
    /// Transfer keeps who cancelled in the `error` column.
    pub fn details(&self) -> (Option<&str>, Option<&str>) {
        match self {
            Self::Completed { saved_to } => (saved_to.as_deref(), None),
            Self::Failed { reason } => (None, Some(reason)),
            Self::Cancelled { by } => (None, Some(by.as_str())),
            _ => (None, None),
        }
    }

    pub fn from_parts(label: &str, saved_to: Option<String>, error: Option<String>) -> Option<Self> {
        Some(match label {
            "offered" => Self::Offered,
            "accepted" => Self::Accepted,
            "declined" => Self::Declined,
            "transferring" => Self::Transferring,
            "reconnecting" => Self::Reconnecting,
            "saving" => Self::Saving,
            "completed" => Self::Completed { saved_to },
            "failed" => Self::Failed { reason: error.unwrap_or_default() },
            "expired" => Self::Expired,
            "cancelled" => Self::Cancelled { by: error?.parse().ok()? },
            _ => return None,
        })
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Declined
                | Self::Completed { .. }
                | Self::Failed { .. }
                | Self::Expired
                | Self::Cancelled { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_id_is_hex_in_json_and_raw_bytes_on_the_wire() {
        let id = TransferId::from_bytes([0xab; 16]);
        assert_eq!(serde_json::to_string(&id).unwrap(), format!("\"{}\"", "ab".repeat(16)));
        assert_eq!(serde_json::from_str::<TransferId>(&format!("\"{id}\"")).unwrap(), id);
        assert_eq!(postcard::to_stdvec(&id).unwrap(), vec![0xab; 16]);
        assert_eq!(postcard::from_bytes::<TransferId>(&[0xab; 16]).unwrap(), id);
        assert_eq!("ab".repeat(16).parse::<TransferId>().unwrap(), id);
    }

    #[test]
    fn transfer_id_rejects_bad_text() {
        assert!("zz".parse::<TransferId>().is_err());
        assert!("ab".parse::<TransferId>().is_err());
    }

    #[test]
    fn state_survives_its_database_representation() {
        let states = [
            TransferState::Offered,
            TransferState::Declined,
            TransferState::Reconnecting,
            TransferState::Completed { saved_to: Some("/x/y".into()) },
            TransferState::Failed { reason: "nope".into() },
            TransferState::Expired,
            TransferState::Cancelled { by: Role::Sender },
            TransferState::Cancelled { by: Role::Receiver },
        ];
        for state in states {
            let (saved_to, error) = state.details();
            let back = TransferState::from_parts(
                state.label(),
                saved_to.map(str::to_owned),
                error.map(str::to_owned),
            );
            assert_eq!(back, Some(state));
        }
        assert_eq!(TransferState::from_parts("bogus", None, None), None);
    }
}
