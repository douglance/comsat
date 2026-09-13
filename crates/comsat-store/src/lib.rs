//! Source-neutral persistence contracts for watches, records, and history.
#![forbid(unsafe_code)]
#![allow(
    clippy::module_name_repetitions,
    reason = "Public contract names intentionally repeat their storage domain."
)]

mod error;
mod model;
mod store;

pub use error::{StoreError, StoreResult};
pub use model::{
    ClaimDueWatch, ClaimPendingDelivery, ClaimedWatch, CompleteDelivery, CompleteWatchRun,
    CreateWatch, DELIVERY_RETRY_SECONDS, DeleteWatch, Delivery, HistoryRequest, ListDeliveries,
    MAX_DELIVERY_ATTEMPTS, RecordObservation, StartWatchRun, UpsertSource, Watch, WatchRun,
    WatchRunDelivery, WatchRunOutcome, empty_object, validate_tenant_id,
};
pub use store::Store;
