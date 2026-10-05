//! SQLite (WAL) persistence: settings, Contacts and Transfer records.

use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::{
    clock::UnixMillis,
    contacts::{Contact, KnownAddress},
    identity::DeviceId,
    manifest::Manifest,
    transfer::{BatchId, Role, TransferId, TransferKind, TransferState},
};

const SCHEMA_VERSION: i32 = 9;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database was written by a newer version (schema {0})")]
    NewerSchema(i32),
    #[error("database task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
    #[error("database holds an unrecognised value: {0}")]
    Corrupt(String),
}

/// One row of the Transfer table: an entry of Transfer History.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct TransferRecord {
    pub id: TransferId,
    pub role: Role,
    /// The other Device's ID (base32).
    pub peer: String,
    /// What the other Device called itself when it last connected for this Transfer; `None` if
    /// it never did (an Offer that could not be delivered). Untrusted text.
    pub peer_name: Option<String>,
    /// The first of `items`: what the Transfer is called where there is room for one name.
    pub name: String,
    /// What the Transfer carries: files, or text that went inline in the Offer.
    pub kind: TransferKind,
    /// Bytes of files, or of text.
    pub size: u64,
    /// The whole text of a `Text` Transfer. A Receiver keeps it only once it has accepted:
    /// text it declined, or that expired, is not kept. Untrusted when received.
    pub text: Option<String>,
    /// The names at the top of what was offered: the files and folders the user picked.
    pub items: Vec<String>,
    pub file_count: u64,
    /// Symlinks the Sender left out.
    pub skipped_links: u32,
    /// Names a Receiver changed to make them safe to write (spec section 6).
    pub adjusted_names: u32,
    /// The Batch a Sender made this Transfer in; `None` for a Transfer sent on its own and for
    /// every Receiver's (a Receiver is never told about the Batch).
    pub batch_id: Option<BatchId>,
    pub state: TransferState,
    /// When the Offer was made.
    pub created_at: UnixMillis,
    /// When the Receiver said yes (or Auto-accept did); `None` if it never was accepted.
    pub accepted_at: Option<UnixMillis>,
    /// When the state last changed: for a Transfer that has ended, when it ended.
    pub updated_at: UnixMillis,
}

/// A Transfer that has not ended, with what a restart needs to carry it on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Unfinished {
    pub record: TransferRecord,
    /// When the Transfer last made progress; the 24-hour stall clock runs from here.
    pub progress_at: UnixMillis,
    /// The Collection's root hash, once the content was ready and the Receiver had accepted.
    pub root: Option<[u8; 32]>,
    /// Receiver only: the folder the Offer was accepted into.
    pub save_dir: Option<PathBuf>,
}

/// A file a Sender offered, as it was when the Offer was made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Source {
    pub path: PathBuf,
    pub size: u64,
    /// Modification time in nanoseconds since the Unix epoch.
    pub mtime_ns: i64,
    /// Its path in the Offer's manifest.
    pub name: String,
}

/// The states that end a Transfer, as stored. Must list what [`TransferState::is_terminal`]
/// does; a test checks.
const ENDED: &str = "'declined', 'completed', 'failed', 'expired', 'cancelled'";

/// Which Transfers [`Db::delete_ended`] looks at.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Scope {
    Everything,
    Transfer(TransferId),
    Batch(BatchId),
}

/// A handle to the database. Cheap to clone; calls run on the blocking pool.
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

impl Db {
    pub async fn open(path: &Path) -> Result<Self, DbError> {
        let path = path.to_owned();
        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(path)?;
            conn.pragma_update(None, "journal_mode", "WAL")?;
            migrate(&conn)?;
            Ok(Self { conn: Arc::new(Mutex::new(conn)) })
        })
        .await?
    }

    async fn run<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, DbError> + Send + 'static,
    ) -> Result<T, DbError> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || f(&conn.lock().unwrap_or_else(|e| e.into_inner())))
            .await?
    }

    pub async fn setting(&self, key: &str) -> Result<Option<String>, DbError> {
        let key = key.to_owned();
        self.run(move |c| {
            Ok(c.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
                .optional()?)
        })
        .await
    }

    pub async fn set_setting(&self, key: &str, value: &str) -> Result<(), DbError> {
        let (key, value) = (key.to_owned(), value.to_owned());
        self.run(move |c| {
            c.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [key, value],
            )?;
            Ok(())
        })
        .await
    }

    /// Saves a Contact; `false` if that Device is already one (nothing is changed then).
    pub async fn insert_contact(&self, c: Contact) -> Result<bool, DbError> {
        self.run(move |conn| {
            let added = conn.execute(
                "INSERT INTO contacts (id, nickname, device_name, auto_accept, relay_url, direct_addrs, added_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(id) DO NOTHING",
                params![
                    c.id.to_string(),
                    c.nickname,
                    c.device_name,
                    c.auto_accept,
                    c.last_known_address.relay_url,
                    join_addrs(&c.last_known_address.direct),
                    c.added_at
                ],
            )?;
            Ok(added == 1)
        })
        .await
    }

    pub async fn contact(&self, id: DeviceId) -> Result<Option<Contact>, DbError> {
        self.run(move |c| {
            c.query_row(&format!("{SELECT_CONTACT} WHERE id = ?1"), [id.to_string()], read_contact)
                .optional()?
                .transpose()
        })
        .await
    }

    /// Every Contact, in the order they were added.
    pub async fn contacts(&self) -> Result<Vec<Contact>, DbError> {
        self.run(|c| {
            let mut stmt = c.prepare(&format!("{SELECT_CONTACT} ORDER BY added_at, rowid"))?;
            let rows = stmt.query_map([], read_contact)?;
            rows.map(|row| row?).collect()
        })
        .await
    }

    /// `false` if there is no such Contact.
    pub async fn set_contact_nickname(
        &self,
        id: DeviceId,
        nickname: Option<String>,
    ) -> Result<bool, DbError> {
        self.run(move |c| {
            let n = c.execute(
                "UPDATE contacts SET nickname = ?2 WHERE id = ?1",
                params![id.to_string(), nickname],
            )?;
            Ok(n == 1)
        })
        .await
    }

    /// `false` if there is no such Contact.
    pub async fn set_contact_auto_accept(&self, id: DeviceId, on: bool) -> Result<bool, DbError> {
        self.run(move |c| {
            let n = c.execute(
                "UPDATE contacts SET auto_accept = ?2 WHERE id = ?1",
                params![id.to_string(), on],
            )?;
            Ok(n == 1)
        })
        .await
    }

    /// `false` if there is no such Contact. Transfer records are not touched.
    pub async fn delete_contact(&self, id: DeviceId) -> Result<bool, DbError> {
        self.run(move |c| Ok(c.execute("DELETE FROM contacts WHERE id = ?1", [id.to_string()])? == 1))
            .await
    }

    /// Folds what a connection to `id` showed into its last known address, and takes the Device
    /// Name it announced, if any. Does nothing if `id` is not a Contact.
    pub async fn update_contact_connection(
        &self,
        id: DeviceId,
        seen: KnownAddress,
        device_name: Option<String>,
    ) -> Result<(), DbError> {
        self.run(move |c| {
            let key = id.to_string();
            let known = c
                .query_row(
                    "SELECT relay_url, direct_addrs FROM contacts WHERE id = ?1",
                    [&key],
                    |r| read_address(r, 0),
                )
                .optional()?;
            let Some(known) = known else { return Ok(()) };
            let merged = known?.updated_with(&seen);
            c.execute(
                "UPDATE contacts
                 SET relay_url = ?2, direct_addrs = ?3, device_name = COALESCE(?4, device_name)
                 WHERE id = ?1",
                params![key, merged.relay_url, join_addrs(&merged.direct), device_name],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn insert_transfer(&self, t: TransferRecord) -> Result<(), DbError> {
        self.run(move |c| {
            let (saved_to, error) = t.state.details();
            c.execute(
                "INSERT INTO transfers
                 (id, role, peer, name, size, state, saved_to, error, created_at, updated_at,
                  progress_at, items, file_count, skipped_links, adjusted_names, batch_id, kind, text,
                  peer_name, accepted_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?9, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
                params![
                    t.id.to_string(),
                    t.role.as_str(),
                    t.peer,
                    t.name,
                    t.size as i64,
                    t.state.label(),
                    saved_to,
                    error,
                    t.created_at,
                    t.updated_at,
                    t.items.join("\n"),
                    t.file_count as i64,
                    t.skipped_links,
                    t.adjusted_names,
                    t.batch_id.map(|batch| batch.to_string()),
                    t.kind.as_str(),
                    t.text,
                    t.peer_name,
                    t.accepted_at
                ],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn update_transfer(
        &self,
        id: TransferId,
        state: TransferState,
        now: UnixMillis,
    ) -> Result<(), DbError> {
        self.run(move |c| {
            let (saved_to, error) = state.details();
            c.execute(
                "UPDATE transfers
                 SET state = ?2, saved_to = ?3, error = ?4, updated_at = ?5,
                     accepted_at = CASE WHEN ?2 = 'accepted' THEN COALESCE(accepted_at, ?5) ELSE accepted_at END
                 WHERE id = ?1",
                params![id.to_string(), state.label(), saved_to, error, now],
            )?;
            if state.is_terminal() {
                // A Receiver kept the manifest only to carry on after a restart.
                c.execute("DELETE FROM transfer_manifests WHERE transfer_id = ?1", [id.to_string()])?;
            }
            Ok(())
        })
        .await
    }

    /// Keeps the text a Receiver accepted and completes the Transfer, in one write: a record
    /// is never Completed without its text, nor holds the text of a Transfer that was not
    /// accepted.
    pub async fn complete_text(&self, id: TransferId, text: &str, now: UnixMillis) -> Result<(), DbError> {
        let text = text.to_owned();
        self.run(move |c| {
            c.execute(
                "UPDATE transfers
                 SET text = ?2, state = 'completed', saved_to = NULL, error = NULL, updated_at = ?3
                 WHERE id = ?1",
                params![id.to_string(), text, now],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn transfers(&self) -> Result<Vec<TransferRecord>, DbError> {
        self.run(|c| {
            let mut stmt =
                c.prepare(&format!("SELECT {COLUMNS} FROM transfers ORDER BY created_at, rowid"))?;
            let rows = stmt.query_map([], read_row)?;
            rows.map(|row| record(row?)).collect()
        })
        .await
    }

    /// Notes what the other Device called itself, for a Transfer that learned it only after it
    /// began (a Sender learns it by reaching the Receiver).
    pub async fn set_peer_name(&self, id: TransferId, name: &str) -> Result<(), DbError> {
        let name = name.to_owned();
        self.run(move |c| {
            c.execute("UPDATE transfers SET peer_name = ?2 WHERE id = ?1", params![id.to_string(), name])?;
            Ok(())
        })
        .await
    }

    /// The Transfers with Device `peer` (any when `None`) in `role` (either when `None`), newest
    /// first.
    pub async fn history(
        &self,
        peer: Option<String>,
        role: Option<Role>,
    ) -> Result<Vec<TransferRecord>, DbError> {
        self.run(move |c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {COLUMNS} FROM transfers
                 WHERE (?1 IS NULL OR peer = ?1) AND (?2 IS NULL OR role = ?2)
                 ORDER BY created_at DESC, rowid DESC"
            ))?;
            let rows = stmt.query_map(params![peer, role.map(Role::as_str)], read_row)?;
            rows.map(|row| record(row?)).collect()
        })
        .await
    }

    /// Deletes the Transfers in `scope` that have ended, with what is kept only for them, and
    /// returns how many. One that is still running is left alone, whatever asked: only the
    /// `state` decides, in the same transaction as the delete. A Batch nothing is left in is
    /// forgotten too (what it was made from can no longer be retried).
    pub async fn delete_ended(&self, scope: Scope) -> Result<u64, DbError> {
        self.run(move |c| {
            let (filter, arg) = match scope {
                Scope::Everything => ("1", None),
                Scope::Transfer(id) => ("id = ?1", Some(id.to_string())),
                Scope::Batch(batch) => ("batch_id = ?1", Some(batch.to_string())),
            };
            let tx = c.unchecked_transaction()?;
            let doomed: Vec<(String, Option<String>)> = tx
                .prepare(&format!("SELECT id, batch_id FROM transfers WHERE state IN ({ENDED}) AND {filter}"))?
                .query_map(rusqlite::params_from_iter(&arg), |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<_, _>>()?;
            for (id, _) in &doomed {
                for table in ["transfer_sources", "transfer_manifests"] {
                    tx.execute(&format!("DELETE FROM {table} WHERE transfer_id = ?1"), [id])?;
                }
                tx.execute("DELETE FROM transfers WHERE id = ?1", [id])?;
            }
            for batch in doomed.iter().filter_map(|(_, batch)| batch.as_ref()).collect::<HashSet<_>>() {
                tx.execute(
                    "DELETE FROM batches WHERE id = ?1
                     AND NOT EXISTS (SELECT 1 FROM transfers WHERE batch_id = ?1)",
                    [batch],
                )?;
            }
            tx.commit()?;
            Ok(doomed.len() as u64)
        })
        .await
    }

    pub async fn transfer(&self, id: TransferId) -> Result<Option<TransferRecord>, DbError> {
        self.run(move |c| {
            let row = c
                .query_row(
                    &format!("SELECT {COLUMNS} FROM transfers WHERE id = ?1"),
                    [id.to_string()],
                    read_row,
                )
                .optional()?;
            row.map(record).transpose()
        })
        .await
    }

    /// The Transfers of a Batch, oldest first.
    pub async fn batch_transfers(&self, batch: BatchId) -> Result<Vec<TransferRecord>, DbError> {
        self.run(move |c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {COLUMNS} FROM transfers WHERE batch_id = ?1 ORDER BY created_at, rowid"
            ))?;
            let rows = stmt.query_map([batch.to_string()], read_row)?;
            rows.map(|row| record(row?)).collect()
        })
        .await
    }

    /// Keeps what a Batch was made from, so a Transfer in it can be sent again: the files and
    /// folders picked, or (a Batch of text, with no files) the text.
    pub async fn insert_batch(
        &self,
        batch: BatchId,
        roots: &[PathBuf],
        text: Option<&str>,
    ) -> Result<(), DbError> {
        let roots: Vec<String> = roots.iter().map(|root| root.to_string_lossy().into_owned()).collect();
        let bytes = postcard::to_stdvec(&roots)
            .map_err(|e| DbError::Corrupt(format!("batch roots do not encode: {e}")))?;
        let text = text.map(str::to_owned);
        self.run(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO batches (id, roots, text) VALUES (?1, ?2, ?3)",
                params![batch.to_string(), bytes, text],
            )?;
            Ok(())
        })
        .await
    }

    /// What [`Db::insert_batch`] kept; `None` for a Batch this Device does not know.
    pub async fn batch_roots(&self, batch: BatchId) -> Result<Option<Vec<PathBuf>>, DbError> {
        let bytes = self
            .run(move |c| {
                Ok(c.query_row("SELECT roots FROM batches WHERE id = ?1", [batch.to_string()], |r| {
                    r.get::<_, Vec<u8>>(0)
                })
                .optional()?)
            })
            .await?;
        bytes
            .map(|bytes| {
                postcard::from_bytes::<Vec<String>>(&bytes)
                    .map(|roots| roots.into_iter().map(PathBuf::from).collect())
                    .map_err(|e| DbError::Corrupt(format!("roots of batch {batch}: {e}")))
            })
            .transpose()
    }

    /// The text a Batch of text was made from; `None` for a Batch of files.
    pub async fn batch_text(&self, batch: BatchId) -> Result<Option<String>, DbError> {
        self.run(move |c| {
            Ok(c.query_row("SELECT text FROM batches WHERE id = ?1", [batch.to_string()], |r| {
                r.get::<_, Option<String>>(0)
            })
            .optional()?
            .flatten())
        })
        .await
    }

    /// Every Transfer that has not ended, oldest first.
    pub async fn unfinished(&self) -> Result<Vec<Unfinished>, DbError> {
        self.run(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {COLUMNS}, progress_at, root_hash, save_dir FROM transfers
                 WHERE state NOT IN ({ENDED}) ORDER BY created_at, rowid"
            ))?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    read_row(r)?,
                    r.get::<_, i64>(19)?,
                    r.get::<_, Option<String>>(20)?,
                    r.get::<_, Option<String>>(21)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (row, progress_at, root, save_dir) = row?;
                let root = root
                    .map(|hex| {
                        data_encoding::HEXLOWER
                            .decode(hex.as_bytes())
                            .ok()
                            .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
                            .ok_or_else(|| DbError::Corrupt(format!("root hash {hex:?}")))
                    })
                    .transpose()?;
                out.push(Unfinished {
                    record: record(row)?,
                    progress_at,
                    root,
                    save_dir: save_dir.map(PathBuf::from),
                });
            }
            Ok(out)
        })
        .await
    }

    /// Records an accepted Transfer's content hash and, for a Receiver, the folder it is
    /// saved into. The stall clock starts here.
    pub async fn start_transfer(
        &self,
        id: TransferId,
        root: Option<[u8; 32]>,
        save_dir: Option<&Path>,
        now: UnixMillis,
    ) -> Result<(), DbError> {
        let save_dir = save_dir.map(|dir| dir.to_string_lossy().into_owned());
        let root = root.map(|root| data_encoding::HEXLOWER.encode(&root));
        self.run(move |c| {
            c.execute(
                "UPDATE transfers
                 SET root_hash = COALESCE(?2, root_hash), save_dir = COALESCE(?3, save_dir),
                     progress_at = ?4
                 WHERE id = ?1",
                params![id.to_string(), root, save_dir, now],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn set_progress_at(&self, id: TransferId, at: UnixMillis) -> Result<(), DbError> {
        self.run(move |c| {
            c.execute(
                "UPDATE transfers SET progress_at = ?2 WHERE id = ?1",
                params![id.to_string(), at],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn insert_sources(
        &self,
        id: TransferId,
        sources: Vec<Source>,
    ) -> Result<(), DbError> {
        self.run(move |c| {
            // One transaction, not one per file: a folder can hold hundreds of thousands.
            let tx = c.unchecked_transaction()?;
            {
                let mut insert = tx.prepare(
                    "INSERT INTO transfer_sources (transfer_id, path, size, mtime_ns, name)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                )?;
                for s in sources {
                    insert.execute(params![
                        id.to_string(),
                        s.path.to_string_lossy(),
                        s.size as i64,
                        s.mtime_ns,
                        s.name
                    ])?;
                }
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// A Transfer's files in the order they were offered.
    pub async fn sources(&self, id: TransferId) -> Result<Vec<Source>, DbError> {
        self.run(move |c| {
            let mut stmt = c.prepare(
                "SELECT path, size, mtime_ns, name FROM transfer_sources WHERE transfer_id = ?1
                 ORDER BY rowid",
            )?;
            let rows = stmt.query_map([id.to_string()], |r| {
                let path = PathBuf::from(r.get::<_, String>(0)?);
                let name: String = r.get(3)?;
                Ok(Source {
                    // A Transfer from before folders was one file, called as the file is.
                    name: if name.is_empty() {
                        path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                    } else {
                        name
                    },
                    path,
                    size: r.get::<_, i64>(1)? as u64,
                    mtime_ns: r.get(2)?,
                })
            })?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .await
    }

    /// The files the Sender's blob store refers to for each of `hashes` (see `sender::import`).
    /// A hash the store has no file for is left out; the `name` of a file is empty.
    pub async fn blob_files(
        &self,
        hashes: Vec<[u8; 32]>,
    ) -> Result<HashMap<[u8; 32], Vec<Source>>, DbError> {
        self.run(move |c| {
            let mut stmt = c.prepare("SELECT path, size, mtime_ns FROM blob_files WHERE hash = ?1")?;
            let mut found = HashMap::new();
            for hash in hashes {
                let files = stmt
                    .query_map([hash.as_slice()], |r| {
                        Ok(Source {
                            path: PathBuf::from(r.get::<_, String>(0)?),
                            size: r.get::<_, i64>(1)? as u64,
                            mtime_ns: r.get(2)?,
                            name: String::new(),
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                if !files.is_empty() {
                    found.insert(hash, files);
                }
            }
            Ok(found)
        })
        .await
    }

    /// Records the files the blob store refers to, each for the hash of its content, and
    /// forgets every file recorded for the hashes in `forget` (the store holds its own copy of
    /// those now).
    pub async fn set_blob_files(
        &self,
        add: Vec<([u8; 32], Source)>,
        forget: Vec<[u8; 32]>,
    ) -> Result<(), DbError> {
        self.run(move |c| {
            // One transaction, not one per file: a folder can hold hundreds of thousands.
            let tx = c.unchecked_transaction()?;
            {
                let mut delete = tx.prepare("DELETE FROM blob_files WHERE hash = ?1")?;
                for hash in forget {
                    delete.execute([hash.as_slice()])?;
                }
                let mut insert = tx.prepare(
                    "INSERT OR REPLACE INTO blob_files (hash, path, size, mtime_ns) VALUES (?1, ?2, ?3, ?4)",
                )?;
                for (hash, s) in add {
                    insert.execute(params![hash.as_slice(), s.path.to_string_lossy(), s.size as i64, s.mtime_ns])?;
                }
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// Keeps the manifest of a Transfer a Receiver accepted, so it can carry on after a
    /// restart: the manifest says what to make of the files once they are fetched.
    pub async fn insert_manifest(&self, id: TransferId, manifest: &Manifest) -> Result<(), DbError> {
        let bytes = postcard::to_stdvec(manifest)
            .map_err(|e| DbError::Corrupt(format!("manifest does not encode: {e}")))?;
        self.run(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO transfer_manifests (transfer_id, manifest) VALUES (?1, ?2)",
                params![id.to_string(), bytes],
            )?;
            Ok(())
        })
        .await
    }

    /// The manifest kept by [`Db::insert_manifest`], until the Transfer ends.
    pub async fn manifest(&self, id: TransferId) -> Result<Option<Manifest>, DbError> {
        let bytes = self
            .run(move |c| {
                Ok(c.query_row(
                    "SELECT manifest FROM transfer_manifests WHERE transfer_id = ?1",
                    [id.to_string()],
                    |r| r.get::<_, Vec<u8>>(0),
                )
                .optional()?)
            })
            .await?;
        bytes
            .map(|bytes| {
                postcard::from_bytes(&bytes)
                    .map_err(|e| DbError::Corrupt(format!("manifest of {id}: {e}")))
            })
            .transpose()
    }
}

const COLUMNS: &str = "id, role, peer, name, size, state, saved_to, error, created_at, updated_at, items, file_count, skipped_links, adjusted_names, batch_id, kind, text, peer_name, accepted_at";

type TransferRow = (
    String,
    String,
    String,
    String,
    i64,
    String,
    Option<String>,
    Option<String>,
    i64,
    i64,
    String,
    i64,
    u32,
    u32,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    Option<i64>,
);

fn read_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<TransferRow> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
        r.get(8)?,
        r.get(9)?,
        r.get(10)?,
        r.get(11)?,
        r.get(12)?,
        r.get(13)?,
        r.get(14)?,
        r.get(15)?,
        r.get(16)?,
        r.get(17)?,
        r.get(18)?,
    ))
}

fn record(row: TransferRow) -> Result<TransferRecord, DbError> {
    let (
        id,
        role,
        peer,
        name,
        size,
        state,
        saved_to,
        error,
        created_at,
        updated_at,
        items,
        file_count,
        skipped_links,
        adjusted_names,
        batch_id,
        kind,
        text,
        peer_name,
        accepted_at,
    ) = row;
    let corrupt = |what: &str, v: &str| DbError::Corrupt(format!("{what} {v:?}"));
    Ok(TransferRecord {
        id: id.parse().map_err(|_| corrupt("transfer id", &id))?,
        role: role.parse().map_err(|_| corrupt("role", &role))?,
        peer,
        peer_name,
        name,
        kind: kind.parse().map_err(|_| corrupt("kind", &kind))?,
        size: size as u64,
        text,
        items: items.split('\n').filter(|item| !item.is_empty()).map(str::to_owned).collect(),
        file_count: file_count as u64,
        skipped_links,
        adjusted_names,
        batch_id: batch_id
            .map(|batch| batch.parse().map_err(|_| corrupt("batch id", &batch)))
            .transpose()?,
        state: TransferState::from_parts(&state, saved_to, error)
            .ok_or_else(|| corrupt("state", &state))?,
        created_at,
        accepted_at,
        updated_at,
    })
}

const SELECT_CONTACT: &str = "SELECT id, nickname, device_name, auto_accept, relay_url, direct_addrs, added_at FROM contacts";

/// Direct addresses are stored one per line.
fn join_addrs(addrs: &[SocketAddr]) -> String {
    addrs.iter().map(SocketAddr::to_string).collect::<Vec<_>>().join("\n")
}

fn read_address(r: &Row<'_>, first: usize) -> rusqlite::Result<Result<KnownAddress, DbError>> {
    let relay_url: Option<String> = r.get(first)?;
    let direct: String = r.get(first + 1)?;
    let direct = direct
        .lines()
        .map(|a| a.parse().map_err(|_| DbError::Corrupt(format!("address {a:?}"))))
        .collect::<Result<_, _>>();
    Ok(direct.map(|direct| KnownAddress { relay_url, direct }))
}

fn read_contact(r: &Row<'_>) -> rusqlite::Result<Result<Contact, DbError>> {
    let id: String = r.get(0)?;
    let address = read_address(r, 4)?;
    Ok((|| {
        Ok(Contact {
            id: id.parse().map_err(|_| DbError::Corrupt(format!("Device ID {id:?}")))?,
            nickname: r.get(1)?,
            device_name: r.get(2)?,
            auto_accept: r.get(3)?,
            last_known_address: address?,
            added_at: r.get(6)?,
        })
    })())
}

fn migrate(conn: &Connection) -> Result<(), DbError> {
    let version: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(DbError::NewerSchema(version));
    }
    if version < 1 {
        conn.execute_batch(
            "BEGIN;
             CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE transfers (
                 id TEXT PRIMARY KEY,
                 role TEXT NOT NULL,
                 peer TEXT NOT NULL,
                 name TEXT NOT NULL,
                 size INTEGER NOT NULL,
                 state TEXT NOT NULL,
                 saved_to TEXT,
                 error TEXT,
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL
             );
             PRAGMA user_version = 1;
             COMMIT;",
        )?;
    }
    if version < 2 {
        conn.execute_batch(
            "BEGIN;
             CREATE TABLE contacts (
                 id TEXT PRIMARY KEY,
                 nickname TEXT,
                 device_name TEXT,
                 auto_accept INTEGER NOT NULL DEFAULT 0,
                 relay_url TEXT,
                 direct_addrs TEXT NOT NULL DEFAULT '',
                 added_at INTEGER NOT NULL
             );
             PRAGMA user_version = 2;
             COMMIT;",
        )?;
    }
    if version < 3 {
        // Resume: when a Transfer last made progress, its content hash, the folder a
        // Receiver saves into, and the files a Sender offered as they were then.
        conn.execute_batch(
            "BEGIN;
             ALTER TABLE transfers ADD COLUMN progress_at INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE transfers ADD COLUMN root_hash TEXT;
             ALTER TABLE transfers ADD COLUMN save_dir TEXT;
             CREATE TABLE transfer_sources (
                 transfer_id TEXT NOT NULL,
                 path TEXT NOT NULL,
                 size INTEGER NOT NULL,
                 mtime_ns INTEGER NOT NULL,
                 PRIMARY KEY (transfer_id, path)
             );
             PRAGMA user_version = 3;
             COMMIT;",
        )?;
    }
    if version < 4 {
        // Folders: what an Offer holds (a Transfer from before was one file, so its one item
        // is its name), the name of each source file in the Offer (which is what is unique:
        // one file can be chosen twice, as itself and inside a folder), and the manifest a
        // Receiver keeps for a Transfer it has accepted.
        conn.execute_batch(
            "BEGIN;
             ALTER TABLE transfers ADD COLUMN items TEXT NOT NULL DEFAULT '';
             ALTER TABLE transfers ADD COLUMN file_count INTEGER NOT NULL DEFAULT 1;
             ALTER TABLE transfers ADD COLUMN skipped_links INTEGER NOT NULL DEFAULT 0;
             UPDATE transfers SET items = name;
             CREATE TABLE transfer_sources_new (
                 transfer_id TEXT NOT NULL,
                 name TEXT NOT NULL,
                 path TEXT NOT NULL,
                 size INTEGER NOT NULL,
                 mtime_ns INTEGER NOT NULL,
                 PRIMARY KEY (transfer_id, name)
             );
             INSERT INTO transfer_sources_new (transfer_id, name, path, size, mtime_ns)
                 SELECT transfer_id, '', path, size, mtime_ns FROM transfer_sources ORDER BY rowid;
             DROP TABLE transfer_sources;
             ALTER TABLE transfer_sources_new RENAME TO transfer_sources;
             CREATE TABLE transfer_manifests (
                 transfer_id TEXT PRIMARY KEY,
                 manifest BLOB NOT NULL
             );
             PRAGMA user_version = 4;
             COMMIT;",
        )?;
    }
    if version < 5 {
        // Received names: how many names a Receiver adjusted (none were, before).
        conn.execute_batch(
            "BEGIN;
             ALTER TABLE transfers ADD COLUMN adjusted_names INTEGER NOT NULL DEFAULT 0;
             PRAGMA user_version = 5;
             COMMIT;",
        )?;
    }
    if version < 6 {
        // Batches: the Batch a Sender's Transfer belongs to, and what each Batch was made of
        // (the paths picked), so a Failed Transfer can be sent again.
        conn.execute_batch(
            "BEGIN;
             ALTER TABLE transfers ADD COLUMN batch_id TEXT;
             CREATE TABLE batches (
                 id TEXT PRIMARY KEY,
                 roots BLOB NOT NULL
             );
             PRAGMA user_version = 6;
             COMMIT;",
        )?;
    }
    if version < 7 {
        // Text: what a Transfer carries, and the text itself (a Receiver's once accepted, a
        // Sender's as sent), plus the text a Batch was made from, for sending it again.
        conn.execute_batch(
            "BEGIN;
             ALTER TABLE transfers ADD COLUMN kind TEXT NOT NULL DEFAULT 'files';
             ALTER TABLE transfers ADD COLUMN text TEXT;
             ALTER TABLE batches ADD COLUMN text TEXT;
             PRAGMA user_version = 7;
             COMMIT;",
        )?;
    }
    if version < 8 {
        // Source files: the files the Sender's blob store refers to, by the hash of their
        // content, so one that has gone or changed can be told from one that is still good.
        conn.execute_batch(
            "BEGIN;
             CREATE TABLE blob_files (
                 hash BLOB NOT NULL,
                 path TEXT NOT NULL,
                 size INTEGER NOT NULL,
                 mtime_ns INTEGER NOT NULL,
                 PRIMARY KEY (hash, path)
             );
             PRAGMA user_version = 8;
             COMMIT;",
        )?;
    }
    if version < 9 {
        // History: what the other Device called itself, and when the Transfer was accepted
        // (the Offer and the end are `created_at` and `updated_at`).
        conn.execute_batch(
            "BEGIN;
             ALTER TABLE transfers ADD COLUMN peer_name TEXT;
             ALTER TABLE transfers ADD COLUMN accepted_at INTEGER;
             PRAGMA user_version = 9;
             COMMIT;",
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: u8, state: TransferState) -> TransferRecord {
        TransferRecord {
            id: TransferId::from_bytes([id; 16]),
            role: Role::Sender,
            peer: "PEER".into(),
            peer_name: None,
            name: "a.txt".into(),
            kind: TransferKind::Files,
            size: 12,
            text: None,
            items: vec!["a.txt".into(), "photos".into()],
            file_count: 3,
            skipped_links: 2,
            adjusted_names: 4,
            batch_id: None,
            state,
            created_at: 100,
            accepted_at: None,
            updated_at: 100,
        }
    }

    #[tokio::test]
    async fn database_uses_wal() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).await.unwrap();
        let mode = db
            .run(|c| Ok(c.pragma_query_value(None, "journal_mode", |r| r.get::<_, String>(0))?))
            .await
            .unwrap();
        assert_eq!(mode, "wal");
    }

    #[tokio::test]
    async fn settings_round_trip_and_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).await.unwrap();
        assert_eq!(db.setting("save_folder").await.unwrap(), None);
        db.set_setting("save_folder", "/a").await.unwrap();
        db.set_setting("save_folder", "/b").await.unwrap();
        assert_eq!(db.setting("save_folder").await.unwrap().as_deref(), Some("/b"));
    }

    #[tokio::test]
    async fn transfer_records_persist_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let db = Db::open(&path).await.unwrap();
        db.insert_transfer(record(1, TransferState::Offered)).await.unwrap();
        db.insert_transfer(record(2, TransferState::Offered)).await.unwrap();
        db.update_transfer(
            TransferId::from_bytes([1; 16]),
            TransferState::Completed { saved_to: Some("/saved/a.txt".into()) },
            200,
        )
        .await
        .unwrap();
        drop(db);

        let rows = Db::open(&path).await.unwrap().transfers().await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].state, TransferState::Completed { saved_to: Some("/saved/a.txt".into()) });
        assert_eq!(rows[0].updated_at, 200);
        assert_eq!(rows[1].state, TransferState::Offered);
    }

    #[tokio::test]
    async fn the_time_a_transfer_was_accepted_is_kept_once() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).await.unwrap();
        let id = TransferId::from_bytes([1; 16]);
        db.insert_transfer(record(1, TransferState::Offered)).await.unwrap();
        db.update_transfer(id, TransferState::Accepted, 150).await.unwrap();
        db.update_transfer(id, TransferState::Transferring, 160).await.unwrap();
        // A Transfer announced as Accepted again keeps its first time.
        db.update_transfer(id, TransferState::Accepted, 170).await.unwrap();
        db.update_transfer(id, TransferState::Completed { saved_to: None }, 180).await.unwrap();

        let done = db.transfer(id).await.unwrap().unwrap();
        assert_eq!((done.created_at, done.accepted_at, done.updated_at), (100, Some(150), 180));
    }

    #[tokio::test]
    async fn history_is_newest_first_and_narrowed_by_device_and_role() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).await.unwrap();
        for (id, peer, role, at) in [
            (1, "A", Role::Sender, 100),
            (2, "B", Role::Receiver, 200),
            (3, "A", Role::Receiver, 300),
            // The same moment as 3: the one recorded later is newer.
            (4, "A", Role::Sender, 300),
        ] {
            let t = TransferRecord { peer: peer.into(), role, created_at: at, ..record(id, TransferState::Offered) };
            db.insert_transfer(t).await.unwrap();
        }
        let ids = |rows: Vec<TransferRecord>| rows.iter().map(|t| t.id.as_bytes()[0]).collect::<Vec<_>>();

        assert_eq!(ids(db.history(None, None).await.unwrap()), [4, 3, 2, 1]);
        assert_eq!(ids(db.history(Some("A".into()), None).await.unwrap()), [4, 3, 1]);
        assert_eq!(ids(db.history(None, Some(Role::Receiver)).await.unwrap()), [3, 2]);
        assert_eq!(ids(db.history(Some("A".into()), Some(Role::Sender)).await.unwrap()), [4, 1]);
    }

    #[tokio::test]
    async fn only_transfers_that_have_ended_are_deleted_with_what_was_kept_for_them() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).await.unwrap();
        let (kept, gone) = (BatchId::random(), BatchId::random());
        let states = [
            (1, TransferState::Completed { saved_to: None }, Some(gone)),
            (2, TransferState::Failed { reason: "x".into() }, Some(gone)),
            (3, TransferState::Transferring, Some(kept)),
            (4, TransferState::Declined, Some(kept)),
            (5, TransferState::Offered, None),
            (6, TransferState::Cancelled { by: Role::Sender }, None),
        ];
        for (id, state, batch) in states {
            db.insert_transfer(TransferRecord { batch_id: batch, ..record(id, state) }).await.unwrap();
            let source = Source { path: "a".into(), size: 1, mtime_ns: 1, name: "a".into() };
            db.insert_sources(TransferId::from_bytes([id; 16]), vec![source]).await.unwrap();
        }
        db.insert_manifest(TransferId::from_bytes([3; 16]), &Manifest::default()).await.unwrap();
        db.insert_batch(gone, &[PathBuf::from("a")], None).await.unwrap();
        db.insert_batch(kept, &[PathBuf::from("a")], None).await.unwrap();

        // Asked for by name, a running Transfer stays. So does the Batch another of whose
        // Transfers is still running, though one that ended in it goes.
        assert_eq!(db.delete_ended(Scope::Transfer(TransferId::from_bytes([3; 16]))).await.unwrap(), 0);
        assert_eq!(db.delete_ended(Scope::Batch(kept)).await.unwrap(), 1);
        assert!(db.batch_roots(kept).await.unwrap().is_some());

        assert_eq!(db.delete_ended(Scope::Everything).await.unwrap(), 3);
        let left: Vec<_> = db.transfers().await.unwrap().iter().map(|t| t.id.as_bytes()[0]).collect();
        assert_eq!(left, [3, 5]);
        // What was kept for the ones that went, went too; the running ones keep theirs.
        assert!(db.sources(TransferId::from_bytes([1; 16])).await.unwrap().is_empty());
        assert_eq!(db.sources(TransferId::from_bytes([3; 16])).await.unwrap().len(), 1);
        assert!(db.manifest(TransferId::from_bytes([3; 16])).await.unwrap().is_some());
        assert_eq!(db.batch_roots(gone).await.unwrap(), None);
        assert!(db.batch_roots(kept).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn a_batch_keeps_its_transfers_and_what_it_was_made_from() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).await.unwrap();
        let (batch, other) = (BatchId::random(), BatchId::random());
        for (id, batch) in [(1, Some(batch)), (2, Some(batch)), (3, Some(other)), (4, None)] {
            db.insert_transfer(TransferRecord { batch_id: batch, ..record(id, TransferState::Offered) })
                .await
                .unwrap();
        }
        let roots = vec![PathBuf::from("/a/photos"), PathBuf::from("/a/b.txt")];
        db.insert_batch(batch, &roots, None).await.unwrap();

        let ids: Vec<_> = db.batch_transfers(batch).await.unwrap().iter().map(|t| t.id).collect();
        assert_eq!(ids, [TransferId::from_bytes([1; 16]), TransferId::from_bytes([2; 16])]);
        assert_eq!(db.transfer(ids[0]).await.unwrap().unwrap().batch_id, Some(batch));
        assert_eq!(db.batch_roots(batch).await.unwrap(), Some(roots));
        assert_eq!(db.batch_roots(other).await.unwrap(), None);
        assert_eq!(db.batch_text(batch).await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_batch_of_text_keeps_the_text_to_send_again() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).await.unwrap();
        let batch = BatchId::random();
        db.insert_batch(batch, &[], Some("héllo\n<b>world</b>")).await.unwrap();

        assert_eq!(db.batch_roots(batch).await.unwrap(), Some(vec![]));
        assert_eq!(db.batch_text(batch).await.unwrap().as_deref(), Some("héllo\n<b>world</b>"));
    }

    #[tokio::test]
    async fn accepted_text_is_kept_with_the_transfer_that_completes_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let db = Db::open(&path).await.unwrap();
        let id = TransferId::from_bytes([8; 16]);
        let offered = TransferRecord {
            role: Role::Receiver,
            kind: TransferKind::Text,
            name: String::new(),
            size: 5,
            items: vec![],
            file_count: 0,
            ..record(8, TransferState::Offered)
        };
        db.insert_transfer(offered).await.unwrap();
        // Until it is accepted the Receiver holds no text.
        assert_eq!(db.transfer(id).await.unwrap().unwrap().text, None);

        db.complete_text(id, "a\u{0}b <script>x</script>", 300).await.unwrap();
        drop(db);

        let back = Db::open(&path).await.unwrap().transfer(id).await.unwrap().unwrap();
        assert_eq!(back.kind, TransferKind::Text);
        assert_eq!(back.text.as_deref(), Some("a\u{0}b <script>x</script>"));
        assert_eq!(back.state, TransferState::Completed { saved_to: None });
        assert_eq!(back.updated_at, 300);
    }

    fn contact(n: u8) -> Contact {
        Contact {
            id: DeviceId::from_endpoint_id(iroh::SecretKey::from_bytes(&[n; 32]).public()),
            nickname: None,
            device_name: Some(format!("Device {n}")),
            auto_accept: false,
            last_known_address: KnownAddress::default(),
            added_at: 100 + i64::from(n),
        }
    }

    #[tokio::test]
    async fn contacts_persist_across_reopen_in_the_order_added() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let db = Db::open(&path).await.unwrap();
        let (a, b) = (contact(1), contact(2));
        assert!(db.insert_contact(b.clone()).await.unwrap());
        assert!(db.insert_contact(a.clone()).await.unwrap());
        // A second add does not overwrite.
        assert!(!db.insert_contact(Contact { nickname: Some("x".into()), ..a.clone() }).await.unwrap());
        assert!(db.set_contact_nickname(a.id, Some("Mum".into())).await.unwrap());
        assert!(db.set_contact_auto_accept(a.id, true).await.unwrap());
        let seen = KnownAddress {
            relay_url: Some("https://relay.example/".into()),
            direct: vec!["192.168.1.5:4000".parse().unwrap(), "[::1]:5000".parse().unwrap()],
        };
        db.update_contact_connection(a.id, seen.clone(), None).await.unwrap();
        drop(db);

        let db = Db::open(&path).await.unwrap();
        let all = db.contacts().await.unwrap();
        assert_eq!(all.len(), 2);
        // Added order is by `added_at`: a (101) before b (102).
        assert_eq!(all[0].id, a.id);
        assert_eq!(
            all[0],
            Contact { nickname: Some("Mum".into()), auto_accept: true, last_known_address: seen, ..a }
        );
        assert_eq!(all[1], b);
    }

    #[tokio::test]
    async fn an_address_update_for_a_stranger_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).await.unwrap();
        let seen = KnownAddress { relay_url: None, direct: vec!["10.0.0.1:1".parse().unwrap()] };
        db.update_contact_connection(contact(9).id, seen, Some("x".into())).await.unwrap();
        assert!(db.contacts().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_database_from_before_contacts_gains_the_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 CREATE TABLE transfers (
                     id TEXT PRIMARY KEY, role TEXT NOT NULL, peer TEXT NOT NULL,
                     name TEXT NOT NULL, size INTEGER NOT NULL, state TEXT NOT NULL,
                     saved_to TEXT, error TEXT,
                     created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
                 INSERT INTO settings VALUES ('k', 'v');
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        }
        let db = Db::open(&path).await.unwrap();
        assert_eq!(db.setting("k").await.unwrap().as_deref(), Some("v"));
        assert!(db.insert_contact(contact(1)).await.unwrap());
    }

    #[tokio::test]
    async fn unfinished_lists_exactly_the_transfers_that_have_not_ended() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).await.unwrap();
        let states = [
            TransferState::Offered,
            TransferState::Accepted,
            TransferState::Declined,
            TransferState::Transferring,
            TransferState::Reconnecting,
            TransferState::Saving,
            TransferState::Completed { saved_to: None },
            TransferState::Failed { reason: "x".into() },
            TransferState::Expired,
            TransferState::Cancelled { by: Role::Receiver },
        ];
        for (i, state) in states.iter().enumerate() {
            db.insert_transfer(record(i as u8, state.clone())).await.unwrap();
        }
        let open: Vec<TransferState> =
            db.unfinished().await.unwrap().into_iter().map(|u| u.record.state).collect();
        let want: Vec<TransferState> = states.into_iter().filter(|s| !s.is_terminal()).collect();
        assert_eq!(open, want);
    }

    #[tokio::test]
    async fn what_a_restart_needs_is_stored_with_the_transfer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let id = TransferId::from_bytes([4; 16]);
        let db = Db::open(&path).await.unwrap();
        db.insert_transfer(record(4, TransferState::Offered)).await.unwrap();
        assert_eq!(db.unfinished().await.unwrap()[0].progress_at, 100, "starts at creation");
        db.start_transfer(id, Some([7; 32]), Some(Path::new("/save")), 150).await.unwrap();
        db.set_progress_at(id, 175).await.unwrap();
        let source = Source {
            path: "/src/a.txt".into(),
            size: 12,
            mtime_ns: 1_700_000_000_123_456_789,
            name: "photos/a.txt".into(),
        };
        db.insert_sources(id, vec![source.clone()]).await.unwrap();
        drop(db);

        let db = Db::open(&path).await.unwrap();
        let [open] = db.unfinished().await.unwrap().try_into().unwrap();
        assert_eq!(open.progress_at, 175);
        assert_eq!(open.root, Some([7; 32]));
        assert_eq!(open.save_dir.as_deref(), Some(Path::new("/save")));
        assert_eq!(db.sources(id).await.unwrap(), [source]);
        assert_eq!(db.transfer(id).await.unwrap(), Some(open.record));
        assert_eq!(db.transfer(TransferId::from_bytes([5; 16])).await.unwrap(), None);
        // The sender side records the hash alone: the save folder stays unset.
        db.start_transfer(id, None, None, 200).await.unwrap();
        let [open] = db.unfinished().await.unwrap().try_into().unwrap();
        assert_eq!((open.root, open.progress_at), (Some([7; 32]), 200));
    }

    #[tokio::test]
    async fn a_version_1_database_is_upgraded_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 CREATE TABLE transfers (
                     id TEXT PRIMARY KEY, role TEXT NOT NULL, peer TEXT NOT NULL,
                     name TEXT NOT NULL, size INTEGER NOT NULL, state TEXT NOT NULL,
                     saved_to TEXT, error TEXT, created_at INTEGER NOT NULL,
                     updated_at INTEGER NOT NULL);
                 INSERT INTO transfers VALUES
                     ('01010101010101010101010101010101', 'sender', 'P', 'a', 1, 'offered',
                      NULL, NULL, 5, 5);
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        }
        let db = Db::open(&path).await.unwrap();
        let [open] = db.unfinished().await.unwrap().try_into().unwrap();
        assert_eq!((open.record.state, open.root, open.progress_at), (TransferState::Offered, None, 0));
    }

    #[tokio::test]
    async fn a_version_3_database_is_upgraded_in_place_and_keeps_its_transfers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 CREATE TABLE transfers (
                     id TEXT PRIMARY KEY, role TEXT NOT NULL, peer TEXT NOT NULL,
                     name TEXT NOT NULL, size INTEGER NOT NULL, state TEXT NOT NULL,
                     saved_to TEXT, error TEXT, created_at INTEGER NOT NULL,
                     updated_at INTEGER NOT NULL, progress_at INTEGER NOT NULL DEFAULT 0,
                     root_hash TEXT, save_dir TEXT);
                 CREATE TABLE contacts (
                     id TEXT PRIMARY KEY, nickname TEXT, device_name TEXT,
                     auto_accept INTEGER NOT NULL DEFAULT 0, relay_url TEXT,
                     direct_addrs TEXT NOT NULL DEFAULT '', added_at INTEGER NOT NULL);
                 CREATE TABLE transfer_sources (
                     transfer_id TEXT NOT NULL, path TEXT NOT NULL, size INTEGER NOT NULL,
                     mtime_ns INTEGER NOT NULL, PRIMARY KEY (transfer_id, path));
                 INSERT INTO transfers (id, role, peer, name, size, state, created_at, updated_at)
                     VALUES ('01010101010101010101010101010101', 'sender', 'P', 'a.txt', 7,
                             'transferring', 5, 5);
                 INSERT INTO transfer_sources VALUES
                     ('01010101010101010101010101010101', '/src/a.txt', 7, 99);
                 PRAGMA user_version = 3;",
            )
            .unwrap();
        }
        let db = Db::open(&path).await.unwrap();
        let id = TransferId::from_bytes([1; 16]);

        // What was one file before is one item, one file, no links; its source is still there,
        // named as the file is.
        let old = db.transfer(id).await.unwrap().unwrap();
        assert_eq!((old.items, old.file_count, old.skipped_links), (vec!["a.txt".to_owned()], 1, 0));
        assert_eq!(old.adjusted_names, 0);
        // Nothing from before text is text.
        assert_eq!((old.kind, old.text), (TransferKind::Files, None));
        // Nor was its Device's name or the time it was accepted written down.
        assert_eq!((old.peer_name, old.accepted_at), (None, None));
        let source = Source { path: "/src/a.txt".into(), size: 7, mtime_ns: 99, name: "a.txt".into() };
        assert_eq!(db.sources(id).await.unwrap(), [source]);

        // And a file can now be a source twice over, under two names.
        let other = TransferId::from_bytes([2; 16]);
        let twice = ["a.txt", "dir/a.txt"].map(|name| Source {
            path: "/src/a.txt".into(),
            size: 7,
            mtime_ns: 99,
            name: name.into(),
        });
        db.insert_sources(other, twice.to_vec()).await.unwrap();
        assert_eq!(db.sources(other).await.unwrap(), twice);
    }

    #[tokio::test]
    async fn what_an_offer_holds_persists_with_its_transfer() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).await.unwrap();
        db.insert_transfer(record(6, TransferState::Offered)).await.unwrap();
        let back = db.transfer(TransferId::from_bytes([6; 16])).await.unwrap().unwrap();
        assert_eq!(back.items, ["a.txt", "photos"]);
        assert_eq!((back.file_count, back.skipped_links, back.adjusted_names), (3, 2, 4));
    }

    #[tokio::test]
    async fn a_receivers_manifest_is_kept_until_its_transfer_ends() {
        use crate::manifest::Entry;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let id = TransferId::from_bytes([8; 16]);
        let manifest = Manifest {
            entries: vec![
                Entry::File { path: "a/b".into(), size: 3, mtime_ns: 77, executable: true },
                Entry::empty_dir("e"),
            ],
        };
        let db = Db::open(&path).await.unwrap();
        db.insert_transfer(record(8, TransferState::Accepted)).await.unwrap();
        assert_eq!(db.manifest(id).await.unwrap(), None);
        db.insert_manifest(id, &manifest).await.unwrap();
        drop(db);

        // It survives a restart ...
        let db = Db::open(&path).await.unwrap();
        assert_eq!(db.manifest(id).await.unwrap(), Some(manifest));
        // ... and goes when the Transfer ends, one way or another.
        db.update_transfer(id, TransferState::Transferring, 200).await.unwrap();
        assert!(db.manifest(id).await.unwrap().is_some());
        db.update_transfer(id, TransferState::Failed { reason: "x".into() }, 300).await.unwrap();
        assert_eq!(db.manifest(id).await.unwrap(), None);
    }

    #[tokio::test]
    async fn the_files_the_store_refers_to_are_kept_by_hash_and_forgotten_on_request() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).await.unwrap();
        let file = |path: &str, size| Source { path: path.into(), size, mtime_ns: 5, name: String::new() };
        let (a, b, c) = ([1; 32], [2; 32], [3; 32]);

        db.set_blob_files(vec![(a, file("/x/1", 10)), (a, file("/y/1", 10)), (b, file("/x/2", 20))], vec![])
            .await
            .unwrap();
        let found = db.blob_files(vec![a, b, c]).await.unwrap();
        assert_eq!(found[&a], [file("/x/1", 10), file("/y/1", 10)]);
        assert_eq!(found[&b], [file("/x/2", 20)]);
        assert!(!found.contains_key(&c), "nothing is known of a hash never noted");

        // A file noted again replaces what was known of it; a hash can be forgotten whole.
        db.set_blob_files(vec![(a, file("/x/1", 11))], vec![b]).await.unwrap();
        let found = db.blob_files(vec![a, b]).await.unwrap();
        assert_eq!(found[&a], [file("/x/1", 11), file("/y/1", 10)]);
        assert!(!found.contains_key(&b));
    }

    #[tokio::test]
    async fn a_newer_schema_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        Connection::open(&path).unwrap().pragma_update(None, "user_version", 99).unwrap();
        assert!(matches!(Db::open(&path).await, Err(DbError::NewerSchema(99))));
    }
}
