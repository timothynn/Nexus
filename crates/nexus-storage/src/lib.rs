//! Durable `SQLite` storage for sessions, runs, and replayable execution events.

use std::{
    path::Path,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use nexus_core::SessionId;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredEvent {
    pub sequence: u64,
    pub kind: String,
    pub payload: serde_json::Value,
    pub created_at_ms: i64,
}

/// Aggregate view of one persisted event stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamSummary {
    pub id: String,
    pub events: u64,
    pub first_at_ms: i64,
    pub last_at_ms: i64,
}

#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn create(&self, id: SessionId) -> Result<(), StorageError>;
}

pub struct SqliteStore {
    connection: Mutex<Connection>,
}

impl SqliteStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let connection =
            Connection::open(path).map_err(|error| StorageError::Backend(error.to_string()))?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions (id TEXT PRIMARY KEY, created_at_ms INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS events (
               session_id TEXT NOT NULL,
               sequence INTEGER NOT NULL,
               kind TEXT NOT NULL,
               payload TEXT NOT NULL,
               created_at_ms INTEGER NOT NULL,
               PRIMARY KEY(session_id, sequence)
             );",
        ).map_err(|error| StorageError::Backend(error.to_string()))?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn append_event(
        &self,
        session_id: &str,
        kind: &str,
        payload: &serde_json::Value,
    ) -> Result<StoredEvent, StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::Backend("SQLite mutex poisoned".to_owned()))?;
        let sequence: u64 = connection
            .query_row(
                "SELECT COALESCE(MAX(sequence) + 1, 0) FROM events WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .map_err(|error| StorageError::Backend(error.to_string()))?;
        let event = StoredEvent {
            sequence,
            kind: kind.to_owned(),
            payload: payload.clone(),
            created_at_ms: now_ms(),
        };
        connection.execute(
            "INSERT INTO events(session_id, sequence, kind, payload, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session_id, event.sequence, event.kind, serde_json::to_string(&event.payload).map_err(|error| StorageError::Backend(error.to_string()))?, event.created_at_ms],
        ).map_err(|error| StorageError::Backend(error.to_string()))?;
        Ok(event)
    }

    /// Summarizes every event stream whose id starts with `prefix`, newest first.
    pub fn list_streams(&self, prefix: &str) -> Result<Vec<StreamSummary>, StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::Backend("SQLite mutex poisoned".to_owned()))?;
        let mut statement = connection
            .prepare(
                "SELECT session_id, COUNT(*), MIN(created_at_ms), MAX(created_at_ms)
                 FROM events WHERE substr(session_id, 1, length(?1)) = ?1
                 GROUP BY session_id ORDER BY MAX(created_at_ms) DESC, session_id ASC",
            )
            .map_err(|error| StorageError::Backend(error.to_string()))?;
        let rows = statement
            .query_map(params![prefix], |row| {
                Ok(StreamSummary {
                    id: row.get(0)?,
                    events: row.get(1)?,
                    first_at_ms: row.get(2)?,
                    last_at_ms: row.get(3)?,
                })
            })
            .map_err(|error| StorageError::Backend(error.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| StorageError::Backend(error.to_string()))
    }

    pub fn replay(&self, session_id: &str) -> Result<Vec<StoredEvent>, StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::Backend("SQLite mutex poisoned".to_owned()))?;
        let mut statement = connection.prepare(
            "SELECT sequence, kind, payload, created_at_ms FROM events WHERE session_id = ?1 ORDER BY sequence ASC",
        ).map_err(|error| StorageError::Backend(error.to_string()))?;
        let rows = statement
            .query_map(params![session_id], |row| {
                let payload: String = row.get(2)?;
                Ok(StoredEvent {
                    sequence: row.get(0)?,
                    kind: row.get(1)?,
                    payload: serde_json::from_str(&payload).unwrap_or(serde_json::Value::Null),
                    created_at_ms: row.get(3)?,
                })
            })
            .map_err(|error| StorageError::Backend(error.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| StorageError::Backend(error.to_string()))
    }
}

#[async_trait]
impl SessionStore for SqliteStore {
    async fn create(&self, id: SessionId) -> Result<(), StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::Backend("SQLite mutex poisoned".to_owned()))?;
        connection
            .execute(
                "INSERT OR IGNORE INTO sessions(id, created_at_ms) VALUES (?1, ?2)",
                params![id.0.to_string(), now_ms()],
            )
            .map_err(|error| StorageError::Backend(error.to_string()))?;
        Ok(())
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("storage error: {0}")]
    Backend(String),
}

#[cfg(test)]
mod tests {
    use super::SqliteStore;

    #[test]
    fn events_can_be_replayed_in_order() {
        let store = SqliteStore::open(":memory:").expect("open memory db");
        store
            .append_event("run-1", "started", &serde_json::json!({}))
            .expect("append");
        store
            .append_event("run-1", "completed", &serde_json::json!({"ok": true}))
            .expect("append");
        let events = store.replay("run-1").expect("replay");
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].kind, "completed");
    }

    #[test]
    fn streams_are_listed_by_prefix() {
        let store = SqliteStore::open(":memory:").expect("open memory db");
        store
            .append_event("mission-a", "x", &serde_json::json!({}))
            .expect("append");
        store
            .append_event("mission-a", "y", &serde_json::json!({}))
            .expect("append");
        store
            .append_event("run-1", "x", &serde_json::json!({}))
            .expect("append");
        let streams = store.list_streams("mission-").expect("list");
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].id, "mission-a");
        assert_eq!(streams[0].events, 2);
        assert_eq!(store.list_streams("").expect("list").len(), 2);
    }
}
