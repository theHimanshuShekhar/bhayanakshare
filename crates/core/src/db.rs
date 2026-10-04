//! SQLite (WAL) persistence: settings, Contacts and Transfer records.

use std::{
    net::SocketAddr,
    path::Path,
    sync::{Arc, Mutex},
};

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::{
    clock::UnixMillis,
    contacts::{Contact, KnownAddress},
    identity::DeviceId,
    transfer::{Role, TransferId, TransferState},
};

const SCHEMA_VERSION: i32 = 2;

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
    pub name: String,
    pub size: u64,
    pub state: TransferState,
    pub created_at: UnixMillis,
    pub updated_at: UnixMillis,
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
                 (id, role, peer, name, size, state, saved_to, error, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
                    t.updated_at
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
            Ok(())
        })
        .await
    }

    pub async fn transfers(&self) -> Result<Vec<TransferRecord>, DbError> {
        self.run(|c| {
            let mut stmt = c.prepare(
                "SELECT id, role, peer, name, size, state, saved_to, error, created_at, updated_at
                 FROM transfers ORDER BY created_at, rowid",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    r.get::<_, i64>(8)?,
                    r.get::<_, i64>(9)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (id, role, peer, name, size, state, saved_to, error, created_at, updated_at) =
                    row?;
                let corrupt = |what: &str, v: &str| DbError::Corrupt(format!("{what} {v:?}"));
                out.push(TransferRecord {
                    id: id.parse().map_err(|_| corrupt("transfer id", &id))?,
                    role: role.parse().map_err(|_| corrupt("role", &role))?,
                    peer,
                    name,
                    size: size as u64,
                    state: TransferState::from_parts(&state, saved_to, error)
                        .ok_or_else(|| corrupt("state", &state))?,
                    created_at,
                    updated_at,
                });
            }
            Ok(out)
        })
        .await
    }
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
    async fn a_newer_schema_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        Connection::open(&path).unwrap().pragma_update(None, "user_version", 99).unwrap();
        assert!(matches!(Db::open(&path).await, Err(DbError::NewerSchema(99))));
    }
}
