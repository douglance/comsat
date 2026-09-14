//! Durable Code Mode persistence for the native CLI.
//!
//! Reads never create the database: inspecting Code Mode state on a machine
//! that has never run a program returns nothing rather than a new data
//! directory. The first write opens, creates, and migrates it.

use std::path::PathBuf;
use std::sync::Arc;
use std::{fs, sync::Mutex};

use async_trait::async_trait;
use comsat_store_sqlite::SqliteCodeModeStore;
use incurs_codemode::{ArtifactRef, ArtifactStore, ExecutionState, RuntimeStore, Snippet};
use serde_json::Value;

pub struct LazyCodeModeStore {
    path: PathBuf,
    opened: Mutex<Option<SqliteCodeModeStore>>,
}

impl LazyCodeModeStore {
    pub const fn new(path: PathBuf) -> Self {
        Self {
            path,
            opened: Mutex::new(None),
        }
    }

    /// The store, or `None` when nothing has been persisted yet.
    fn read(&self) -> Result<Option<SqliteCodeModeStore>, String> {
        if !self.path.exists() {
            return Ok(None);
        }
        self.open().map(Some)
    }

    /// The store, creating the database when it does not exist.
    fn write(&self) -> Result<SqliteCodeModeStore, String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        self.open()
    }

    fn open(&self) -> Result<SqliteCodeModeStore, String> {
        let cached = {
            let opened = self
                .opened
                .lock()
                .map_err(|_| "Code Mode store lock was poisoned".to_owned())?;
            opened.clone()
        };
        if let Some(store) = cached {
            return Ok(store);
        }
        let store = SqliteCodeModeStore::open(&self.path)?;
        let mut opened = self
            .opened
            .lock()
            .map_err(|_| "Code Mode store lock was poisoned".to_owned())?;
        *opened = Some(store.clone());
        drop(opened);
        Ok(store)
    }
}

#[async_trait]
impl RuntimeStore for LazyCodeModeStore {
    async fn get_execution(&self, id: &str) -> Result<Option<ExecutionState>, String> {
        match self.read()? {
            Some(store) => store.get_execution(id).await,
            None => Ok(None),
        }
    }

    async fn put_execution(&self, execution: &ExecutionState) -> Result<(), String> {
        self.write()?.put_execution(execution).await
    }

    async fn list_executions(&self) -> Result<Vec<ExecutionState>, String> {
        match self.read()? {
            Some(store) => store.list_executions().await,
            None => Ok(Vec::new()),
        }
    }

    async fn delete_execution(&self, id: &str) -> Result<(), String> {
        match self.read()? {
            Some(store) => RuntimeStore::delete_execution(&store, id).await,
            None => Ok(()),
        }
    }

    async fn get_snippet(&self, name: &str) -> Result<Option<Snippet>, String> {
        match self.read()? {
            Some(store) => store.get_snippet(name).await,
            None => Ok(None),
        }
    }

    async fn put_snippet(&self, snippet: &Snippet) -> Result<(), String> {
        self.write()?.put_snippet(snippet).await
    }

    async fn list_snippets(&self) -> Result<Vec<Snippet>, String> {
        match self.read()? {
            Some(store) => store.list_snippets().await,
            None => Ok(Vec::new()),
        }
    }

    async fn delete_snippet(&self, name: &str) -> Result<bool, String> {
        match self.read()? {
            Some(store) => store.delete_snippet(name).await,
            None => Ok(false),
        }
    }
}

#[async_trait]
impl ArtifactStore for LazyCodeModeStore {
    async fn put(&self, execution_id: &str, value: &Value) -> Result<ArtifactRef, String> {
        self.write()?.put(execution_id, value).await
    }

    async fn get(&self, execution_id: &str, artifact_id: &str) -> Result<Option<Value>, String> {
        match self.read()? {
            Some(store) => ArtifactStore::get(&store, execution_id, artifact_id).await,
            None => Ok(None),
        }
    }

    async fn delete_execution(&self, execution_id: &str) -> Result<(), String> {
        match self.read()? {
            Some(store) => ArtifactStore::delete_execution(&store, execution_id).await,
            None => Ok(()),
        }
    }
}

/// One durable store shared by every Code Mode command in this process.
pub fn store(path: PathBuf) -> Arc<LazyCodeModeStore> {
    Arc::new(LazyCodeModeStore::new(path))
}
