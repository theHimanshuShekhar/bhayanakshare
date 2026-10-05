//! SQLite (WAL) persistence: settings, Contacts and Transfer records.

use std::{
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
    transfer::{Role, TransferId, TransferState},
};

const SCHEMA_VERSION: i32 = 5;

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

/// One row of the Transfer table; the seed of Transfer History.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TransferRecord {
    pub id: TransferId,
    pub role: Role,
    /// The other Device's ID (base32).
    pub peer: String,
    /// The first of `items`: what the Transfer is called where there is room for one name.
    pub name: String,
    pub size: u64,
    /// The names at the top of what was offered: the files and folders the user picked.
    pub items: Vec<String>,
    pub file_count: u64,
    /// Symlinks the Sender left out.
    pub skipped_links: u32,
    /// Names a Receiver changed to make them safe to write (spec section 6).
    pub adjusted_names: u32,
    pub state: TransferState,
    pub created_at: UnixMillis,
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
                  progress_at, items, file_count, skipped_links, adjusted_names)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?9, ?11, ?12, ?13, ?14)",
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
                    t.adjusted_names
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
                 SET state = ?2, saved_to = ?3, error = ?4, updated_at = ?5
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

    pub async fn transfers(&self) -> Result<Vec<TransferRecord>, DbError> {
        self.run(|c| {
            let mut stmt =
                c.prepare(&format!("SELECT {COLUMNS} FROM transfers ORDER BY created_at, rowid"))?;
            let rows = stmt.query_map([], read_row)?;
            rows.map(|row| record(row?)).collect()
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
                    r.get::<_, i64>(14)?,
                    r.get::<_, Option<String>>(15)?,
                    r.get::<_, Option<String>>(16)?,
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

const COLUMNS: &str = "id, role, peer, name, size, state, saved_to, error, created_at, updated_at, items, file_count, skipped_links, adjusted_names";

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
    ) = row;
    let corrupt = |what: &str, v: &str| DbError::Corrupt(format!("{what} {v:?}"));
    Ok(TransferRecord {
        id: id.parse().map_err(|_| corrupt("transfer id", &id))?,
        role: role.parse().map_err(|_| corrupt("role", &role))?,
        peer,
        name,
        size: size as u64,
        items: items.split('\n').filter(|item| !item.is_empty()).map(str::to_owned).collect(),
        file_count: file_count as u64,
        skipped_links,
        adjusted_names,
        state: TransferState::from_parts(&state, saved_to, error)
            .ok_or_else(|| corrupt("state", &state))?,
        created_at,
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
            name: "a.txt".into(),
            size: 12,
            items: vec!["a.txt".into(), "photos".into()],
            file_count: 3,
            skipped_links: 2,
            adjusted_names: 4,
            state,
            created_at: 100,
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
    async fn a_newer_schema_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        Connection::open(&path).unwrap().pragma_update(None, "user_version", 99).unwrap();
        assert!(matches!(Db::open(&path).await, Err(DbError::NewerSchema(99))));
    }
}
