//! Transfer History: the Transfers a Device has sent and received, as the History screen lists
//! them (spec section 7). The records themselves are the Transfer table's (`db`); this is how
//! they are narrowed, searched and grouped.

use std::{collections::HashMap, path::Path};

use crate::{
    db::TransferRecord,
    identity::DeviceId,
    transfer::{BatchId, Role, TransferState},
};

/// What to list. Every field that is set narrows the list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryQuery {
    /// Only Transfers with this Device. A Batch is not grouped then: it shows as this Device's
    /// Transfer, as in a Contact's History.
    pub device: Option<DeviceId>,
    /// Only Transfers this Device sent (`Sender`) or received (`Receiver`).
    pub direction: Option<Role>,
    /// Only Transfers with an item whose name holds this text, ignoring case.
    pub search: Option<String>,
}

/// A Transfer in History.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct HistoryTransfer {
    pub record: TransferRecord,
    /// For files this Device received and saved: whether what was saved is still where it was
    /// put. `None` for every other Transfer, which has no saved location.
    pub saved_present: Option<bool>,
}

/// A row of History, newest first.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HistoryEntry {
    /// A Transfer sent on its own, a Receiver's, or one of a Batch seen as a Device's.
    Transfer { transfer: HistoryTransfer },
    /// A Sender's Batch, as one entry: its Transfers, oldest first, one for each time a
    /// Receiver was sent to (a retry follows the Transfer it retries).
    Batch { batch_id: BatchId, transfers: Vec<HistoryTransfer> },
}

impl HistoryTransfer {
    fn new(record: TransferRecord) -> Self {
        let saved_present = match (&record.state, record.role) {
            (TransferState::Completed { saved_to: Some(saved) }, Role::Receiver) => {
                Some(Path::new(saved).exists())
            }
            _ => None,
        };
        Self { record, saved_present }
    }
}

/// Whether one of the item names holds `needle`, which is already lower case.
fn names_match(record: &TransferRecord, needle: &str) -> bool {
    record.items.iter().any(|item| item.to_lowercase().contains(needle))
}

/// The entries for `records`, which are newest first and already narrowed by Device and
/// direction. `search` narrows them further. A Batch is one entry, where its oldest Transfer
/// is, unless `group` is off. Looks at the disk, for where received files were saved.
pub(crate) fn entries(records: Vec<TransferRecord>, search: Option<&str>, group: bool) -> Vec<HistoryEntry> {
    let needle = search.map(str::trim).filter(|s| !s.is_empty()).map(str::to_lowercase);
    let records: Vec<_> = records
        .into_iter()
        .filter(|record| needle.as_deref().is_none_or(|needle| names_match(record, needle)))
        .collect();

    // Where each Batch's oldest Transfer is: the entry takes that place.
    let mut oldest: HashMap<BatchId, usize> = HashMap::new();
    if group {
        for (i, record) in records.iter().enumerate() {
            if let Some(batch) = record.batch_id {
                oldest.insert(batch, i);
            }
        }
    }
    let mut members: HashMap<BatchId, Vec<HistoryTransfer>> = HashMap::new();
    let mut out = Vec::new();
    for (i, record) in records.into_iter().enumerate() {
        match record.batch_id.filter(|_| group) {
            None => out.push(HistoryEntry::Transfer { transfer: HistoryTransfer::new(record) }),
            Some(batch) => {
                members.entry(batch).or_default().push(HistoryTransfer::new(record));
                if oldest[&batch] == i {
                    let mut transfers = members.remove(&batch).unwrap_or_default();
                    transfers.reverse();
                    out.push(HistoryEntry::Batch { batch_id: batch, transfers });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer::{TransferId, TransferKind};

    fn record(n: u8, items: &[&str], batch: Option<BatchId>) -> TransferRecord {
        TransferRecord {
            id: TransferId::from_bytes([n; 16]),
            role: Role::Sender,
            peer: "P".into(),
            peer_name: None,
            name: items.first().copied().unwrap_or_default().into(),
            kind: TransferKind::Files,
            size: 1,
            text: None,
            items: items.iter().map(|&item| item.into()).collect(),
            file_count: 1,
            skipped_links: 0,
            adjusted_names: 0,
            batch_id: batch,
            state: TransferState::Offered,
            created_at: i64::from(n),
            accepted_at: None,
            updated_at: i64::from(n),
        }
    }

    /// The Transfer IDs' first bytes of each entry, a Batch's in brackets.
    fn shape(entries: &[HistoryEntry]) -> String {
        entries
            .iter()
            .map(|entry| match entry {
                HistoryEntry::Transfer { transfer } => transfer.record.id.as_bytes()[0].to_string(),
                HistoryEntry::Batch { transfers, .. } => {
                    let ids: Vec<_> = transfers.iter().map(|t| t.record.id.as_bytes()[0].to_string()).collect();
                    format!("[{}]", ids.join(" "))
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn a_batch_is_one_entry_where_its_oldest_transfer_is() {
        let (one, two) = (BatchId::random(), BatchId::random());
        // Newest first: 7 on its own, a retry (6) in `one`, a Batch `two`, the rest of `one`.
        let records = vec![
            record(7, &["a"], None),
            record(6, &["a"], Some(one)),
            record(5, &["b"], Some(two)),
            record(4, &["b"], Some(two)),
            record(3, &["a"], Some(one)),
            record(2, &["a"], Some(one)),
            record(1, &["c"], None),
        ];
        assert_eq!(shape(&entries(records.clone(), None, true)), "7 [4 5] [2 3 6] 1");
        // Not grouped, each is a Transfer of its own.
        assert_eq!(shape(&entries(records, None, false)), "7 6 5 4 3 2 1");
    }

    #[test]
    fn search_looks_at_item_names_ignoring_case_and_keeps_the_batch_whole() {
        let batch = BatchId::random();
        let records = vec![
            record(3, &["Résumé.pdf", "photos"], None),
            record(2, &["notes.txt"], Some(batch)),
            record(1, &["notes.txt"], Some(batch)),
        ];
        assert_eq!(shape(&entries(records.clone(), Some("RÉSUMÉ"), true)), "3");
        assert_eq!(shape(&entries(records.clone(), Some("  PHOTO "), true)), "3");
        assert_eq!(shape(&entries(records.clone(), Some("notes"), true)), "[1 2]");
        assert_eq!(shape(&entries(records.clone(), Some("nothing"), true)), "");
        // A blank search is no search.
        assert_eq!(shape(&entries(records, Some("   "), true)), "3 [1 2]");
    }

    #[test]
    fn a_saved_location_is_checked_for_a_received_file_only() {
        let dir = tempfile::tempdir().unwrap();
        let there = dir.path().join("there.txt");
        std::fs::write(&there, b"x").unwrap();
        let received = |path: &Path, role| TransferRecord {
            role,
            state: TransferState::Completed { saved_to: Some(path.to_string_lossy().into_owned()) },
            ..record(1, &["there.txt"], None)
        };
        assert_eq!(HistoryTransfer::new(received(&there, Role::Receiver)).saved_present, Some(true));
        assert_eq!(
            HistoryTransfer::new(received(&dir.path().join("gone.txt"), Role::Receiver)).saved_present,
            Some(false)
        );
        assert_eq!(HistoryTransfer::new(received(&there, Role::Sender)).saved_present, None);
        let text = TransferRecord { state: TransferState::Completed { saved_to: None }, ..record(2, &[], None) };
        assert_eq!(HistoryTransfer::new(text).saved_present, None);
    }
}
