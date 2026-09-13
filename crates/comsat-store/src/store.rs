use async_trait::async_trait;

use crate::{
    ClaimDueWatch, ClaimPendingDelivery, ClaimedWatch, CompleteDelivery, CompleteWatchRun,
    CreateWatch, DeleteWatch, Delivery, HistoryRequest, ListDeliveries, RecordObservation,
    StartWatchRun, StoreResult, UpsertSource, Watch, WatchRun, WatchRunOutcome,
};

#[async_trait]
pub trait Store: Send + Sync {
    async fn upsert_source(&self, request: UpsertSource) -> StoreResult<()>;

    async fn create_watch(&self, request: CreateWatch) -> StoreResult<Watch>;

    async fn list_watches(&self, tenant_id: &str) -> StoreResult<Vec<Watch>>;

    async fn delete_watch(&self, request: DeleteWatch) -> StoreResult<bool>;

    async fn claim_due_watch(&self, request: ClaimDueWatch) -> StoreResult<Option<ClaimedWatch>>;

    async fn start_watch_run(&self, request: StartWatchRun) -> StoreResult<WatchRun>;

    async fn complete_watch_run(&self, request: CompleteWatchRun) -> StoreResult<WatchRunOutcome>;

    async fn history(&self, request: HistoryRequest) -> StoreResult<Vec<RecordObservation>>;

    async fn claim_pending_delivery(
        &self,
        request: ClaimPendingDelivery,
    ) -> StoreResult<Option<Delivery>>;

    async fn complete_delivery(&self, request: CompleteDelivery) -> StoreResult<Delivery>;

    async fn list_deliveries(&self, request: ListDeliveries) -> StoreResult<Vec<Delivery>>;
}
