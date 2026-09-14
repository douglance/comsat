//! Durable Code Mode persistence. Incurs supplies the runtime and takes its
//! storage through traits, so COMSAT keeps execution lifecycle, artifacts, and
//! snippets in the same SQLite database as the rest of native state, where they
//! outlive the process that produced them.

use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use incurs_codemode::{ArtifactRef, ArtifactStore, ExecutionState, RuntimeStore, Snippet};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use sha2::{Digest, Sha256};

const PREVIEW_BYTES: usize = 512;

/// SQLite-backed durable store for the Code Mode runtime.
#[derive(Clone)]
pub struct SqliteCodeModeStore {
    connection: Arc<Mutex<Connection>>,
}

impl SqliteCodeModeStore {
    /// Opens the store, applying the shared schema so a fresh database works.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let store = crate::SqliteStore::open(path).map_err(|error| error.to_string())?;
        store
            .apply_migrations()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            connection: store.raw_connection(),
        })
    }

    pub fn in_memory() -> Result<Self, String> {
        let store = crate::SqliteStore::in_memory().map_err(|error| error.to_string())?;
        store
            .apply_migrations()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            connection: store.raw_connection(),
        })
    }

    fn with_connection<T>(
        &self,
        operation: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<T, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "Code Mode store lock was poisoned".to_owned())?;
        operation(&connection).map_err(|error| error.to_string())
    }
}

#[async_trait]
impl RuntimeStore for SqliteCodeModeStore {
    async fn get_execution(&self, id: &str) -> Result<Option<ExecutionState>, String> {
        let state: Option<String> = self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT state FROM codemode_executions WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .optional()
        })?;
        state.map(|state| decode(&state)).transpose()
    }

    async fn put_execution(&self, execution: &ExecutionState) -> Result<(), String> {
        let state = encode(execution)?;
        let updated_at = i64::try_from(execution.updated_at).unwrap_or(i64::MAX);
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO codemode_executions(id, updated_at, state) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET updated_at = excluded.updated_at, state = excluded.state",
                params![execution.id, updated_at, state],
            )
        })?;
        Ok(())
    }

    async fn list_executions(&self) -> Result<Vec<ExecutionState>, String> {
        let states: Vec<String> = self.with_connection(|connection| {
            let mut statement = connection
                .prepare("SELECT state FROM codemode_executions ORDER BY updated_at DESC")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect()
        })?;
        states.iter().map(|state| decode(state)).collect()
    }

    async fn delete_execution(&self, id: &str) -> Result<(), String> {
        self.with_connection(|connection| {
            connection.execute(
                "DELETE FROM codemode_artifacts WHERE execution_id = ?1",
                params![id],
            )?;
            connection.execute("DELETE FROM codemode_executions WHERE id = ?1", params![id])
        })?;
        Ok(())
    }

    async fn get_snippet(&self, name: &str) -> Result<Option<Snippet>, String> {
        let snippet: Option<String> = self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT snippet FROM codemode_snippets WHERE name = ?1",
                    params![name],
                    |row| row.get(0),
                )
                .optional()
        })?;
        snippet.map(|snippet| decode(&snippet)).transpose()
    }

    async fn put_snippet(&self, snippet: &Snippet) -> Result<(), String> {
        let encoded = encode(snippet)?;
        let saved_at = i64::try_from(snippet.saved_at).unwrap_or(i64::MAX);
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO codemode_snippets(name, saved_at, snippet) VALUES (?1, ?2, ?3)
                 ON CONFLICT(name) DO UPDATE SET saved_at = excluded.saved_at, snippet = excluded.snippet",
                params![snippet.name, saved_at, encoded],
            )
        })?;
        Ok(())
    }

    async fn list_snippets(&self) -> Result<Vec<Snippet>, String> {
        let snippets: Vec<String> = self.with_connection(|connection| {
            let mut statement =
                connection.prepare("SELECT snippet FROM codemode_snippets ORDER BY name")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect()
        })?;
        snippets.iter().map(|snippet| decode(snippet)).collect()
    }

    async fn delete_snippet(&self, name: &str) -> Result<bool, String> {
        let deleted = self.with_connection(|connection| {
            connection.execute(
                "DELETE FROM codemode_snippets WHERE name = ?1",
                params![name],
            )
        })?;
        Ok(deleted > 0)
    }
}

#[async_trait]
impl ArtifactStore for SqliteCodeModeStore {
    async fn put(&self, execution_id: &str, value: &Value) -> Result<ArtifactRef, String> {
        let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
        let id = format!(
            "{:x}",
            Sha256::digest([execution_id.as_bytes(), &bytes].concat())
        );
        let text = String::from_utf8_lossy(&bytes).into_owned();
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO codemode_artifacts(execution_id, artifact_id, value) VALUES (?1, ?2, ?3)
                 ON CONFLICT(execution_id, artifact_id) DO UPDATE SET value = excluded.value",
                params![execution_id, id, text],
            )
        })?;
        Ok(ArtifactRef {
            id,
            execution_id: execution_id.to_owned(),
            bytes: bytes.len(),
            preview: preview(&text),
        })
    }

    async fn get(&self, execution_id: &str, artifact_id: &str) -> Result<Option<Value>, String> {
        let value: Option<String> = self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT value FROM codemode_artifacts WHERE execution_id = ?1 AND artifact_id = ?2",
                    params![execution_id, artifact_id],
                    |row| row.get(0),
                )
                .optional()
        })?;
        value.map(|value| decode(&value)).transpose()
    }

    async fn delete_execution(&self, execution_id: &str) -> Result<(), String> {
        self.with_connection(|connection| {
            connection.execute(
                "DELETE FROM codemode_artifacts WHERE execution_id = ?1",
                params![execution_id],
            )
        })?;
        Ok(())
    }
}

fn encode<T: serde::Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(|error| error.to_string())
}

fn decode<T: for<'de> serde::Deserialize<'de>>(value: &str) -> Result<T, String> {
    serde_json::from_str(value).map_err(|error| error.to_string())
}

/// Bounded human-readable preview, cut on a character boundary.
fn preview(text: &str) -> String {
    if text.len() <= PREVIEW_BYTES {
        return text.to_owned();
    }
    let mut end = PREVIEW_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[cfg(test)]
mod tests;
