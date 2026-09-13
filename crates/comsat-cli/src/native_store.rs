use std::{env, fs, path::PathBuf};

use comsat_store::{
    ClaimDueWatch, ClaimPendingDelivery, ClaimedWatch, CompleteDelivery, CompleteWatchRun,
    CreateWatch, DeleteWatch, Delivery, HistoryRequest, ListDeliveries, RecordObservation,
    StartWatchRun, Store, StoreResult, UpsertSource, Watch, WatchRun, WatchRunOutcome,
};
use comsat_store_sqlite::SqliteStore;

pub struct LazySqliteStore {
    path: PathBuf,
}

impl LazySqliteStore {
    pub const fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn exists(&self) -> bool {
        self.path.exists()
    }

    fn open_read(&self) -> StoreResult<Option<SqliteStore>> {
        if self.exists() {
            Ok(Some(SqliteStore::open(&self.path)?))
        } else {
            Ok(None)
        }
    }

    fn open_write(&self) -> StoreResult<SqliteStore> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| comsat_store::StoreError::Backend(error.to_string()))?;
        }
        SqliteStore::open(&self.path)
    }
}

#[async_trait::async_trait]
impl Store for LazySqliteStore {
    async fn upsert_source(&self, request: UpsertSource) -> StoreResult<()> {
        self.open_write()?.upsert_source(request).await
    }

    async fn create_watch(&self, request: CreateWatch) -> StoreResult<Watch> {
        self.open_write()?.create_watch(request).await
    }

    async fn list_watches(&self, tenant_id: &str) -> StoreResult<Vec<Watch>> {
        let Some(store) = self.open_read()? else {
            return Ok(Vec::new());
        };
        store.list_watches(tenant_id).await
    }

    async fn delete_watch(&self, request: DeleteWatch) -> StoreResult<bool> {
        let Some(store) = self.open_read()? else {
            return Ok(false);
        };
        store.delete_watch(request).await
    }

    async fn claim_due_watch(&self, request: ClaimDueWatch) -> StoreResult<Option<ClaimedWatch>> {
        let Some(store) = self.open_read()? else {
            return Ok(None);
        };
        store.claim_due_watch(request).await
    }

    async fn start_watch_run(&self, request: StartWatchRun) -> StoreResult<WatchRun> {
        self.open_write()?.start_watch_run(request).await
    }

    async fn complete_watch_run(&self, request: CompleteWatchRun) -> StoreResult<WatchRunOutcome> {
        self.open_write()?.complete_watch_run(request).await
    }

    async fn history(&self, request: HistoryRequest) -> StoreResult<Vec<RecordObservation>> {
        let Some(store) = self.open_read()? else {
            return Ok(Vec::new());
        };
        store.history(request).await
    }

    async fn claim_pending_delivery(
        &self,
        request: ClaimPendingDelivery,
    ) -> StoreResult<Option<Delivery>> {
        let Some(store) = self.open_read()? else {
            return Ok(None);
        };
        store.claim_pending_delivery(request).await
    }

    async fn complete_delivery(&self, request: CompleteDelivery) -> StoreResult<Delivery> {
        self.open_write()?.complete_delivery(request).await
    }

    async fn list_deliveries(&self, request: ListDeliveries) -> StoreResult<Vec<Delivery>> {
        let Some(store) = self.open_read()? else {
            return Ok(Vec::new());
        };
        store.list_deliveries(request).await
    }
}

pub fn database_path() -> PathBuf {
    let base = env::var_os("COMSAT_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| env::var_os("XDG_DATA_HOME").map(|root| PathBuf::from(root).join("comsat")))
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share/comsat")))
        .unwrap_or_else(|| env::temp_dir().join("comsat"));
    base.join("comsat.sqlite3")
}
