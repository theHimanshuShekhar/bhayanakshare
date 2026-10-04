//! SQLite (WAL) persistence: settings and Transfer records.

use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use rusqlite::{Connection, OptionalExtension, params};

use crate::{
    clock::UnixMillis,
    transfer::{Role, TransferId, TransferState},
};

const SCHEMA_VERSION: i32 = 1;

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

    #[tokio::test]
    async fn a_newer_schema_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        Connection::open(&path).unwrap().pragma_update(None, "user_version", 99).unwrap();
        assert!(matches!(Db::open(&path).await, Err(DbError::NewerSchema(99))));
    }
}
