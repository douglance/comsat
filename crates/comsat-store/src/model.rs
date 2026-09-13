use comsat_types::{Query, Record, SourceId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{StoreError, StoreResult};

pub const MAX_DELIVERY_ATTEMPTS: u32 = 3;
pub const DELIVERY_RETRY_SECONDS: i64 = 300;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpsertSource {
    pub tenant_id: String,
    pub source_id: SourceId,
    pub enabled: bool,
    pub observed_at_epoch_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CreateWatch {
    pub tenant_id: String,
    pub watch_id: String,
    pub query: Query,
    pub source_ids: Vec<SourceId>,
    pub interval_seconds: u64,
    pub cursor: Value,
    pub enabled: bool,
    pub created_at_epoch_seconds: i64,
    pub first_due_epoch_seconds: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Watch {
    pub tenant_id: String,
    pub watch_id: String,
    pub query: Query,
    pub source_ids: Vec<SourceId>,
    pub interval_seconds: u64,
    pub cursor: Value,
    pub enabled: bool,
    pub created_at_epoch_seconds: i64,
    pub next_due_epoch_seconds: i64,
    pub lease_owner: Option<String>,
    pub lease_expires_epoch_seconds: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClaimDueWatch {
    pub tenant_id: String,
    pub now_epoch_seconds: i64,
    pub lease_owner: String,
    pub lease_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeleteWatch {
    pub tenant_id: String,
    pub watch_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClaimedWatch {
    pub watch: Watch,
    pub lease_owner: String,
    pub lease_expires_epoch_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StartWatchRun {
    pub tenant_id: String,
    pub watch_id: String,
    pub run_id: String,
    pub idempotency_key: String,
    pub lease_owner: String,
    pub started_at_epoch_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WatchRun {
    pub tenant_id: String,
    pub watch_id: String,
    pub run_id: String,
    pub idempotency_key: String,
    pub lease_owner: String,
    pub status: String,
    pub started_at_epoch_seconds: i64,
    pub finished_at_epoch_seconds: Option<i64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompleteWatchRun {
    pub tenant_id: String,
    pub watch_id: String,
    pub run_id: String,
    pub lease_owner: String,
    pub finished_at_epoch_seconds: i64,
    pub records: Vec<Record>,
    pub next_cursor: Value,
    pub error: Option<String>,
    pub delivery: Option<WatchRunDelivery>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WatchRunOutcome {
    pub run_id: String,
    pub status: String,
    pub records_seen: usize,
    pub records_inserted: usize,
    pub watch_records_inserted: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WatchRunDelivery {
    pub delivery_id: String,
    pub target: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HistoryRequest {
    pub tenant_id: String,
    pub watch_id: Option<String>,
    pub since_epoch_seconds: Option<i64>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecordObservation {
    pub record: Record,
    pub watch_id: Option<String>,
    pub first_observed_epoch_seconds: i64,
    pub last_observed_epoch_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClaimPendingDelivery {
    pub tenant_id: String,
    pub now_epoch_seconds: i64,
    pub lease_owner: String,
    pub lease_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompleteDelivery {
    pub tenant_id: String,
    pub delivery_id: String,
    pub lease_owner: String,
    pub finished_at_epoch_seconds: i64,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListDeliveries {
    pub tenant_id: String,
    pub status: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Delivery {
    pub tenant_id: String,
    pub delivery_id: String,
    pub watch_id: String,
    pub run_id: Option<String>,
    pub status: String,
    pub target: Value,
    pub payload: Value,
    pub created_at_epoch_seconds: i64,
    pub next_attempt_epoch_seconds: i64,
    pub attempts: u32,
    pub lease_owner: Option<String>,
    pub lease_expires_epoch_seconds: Option<i64>,
    pub delivered_at_epoch_seconds: Option<i64>,
    pub error: Option<String>,
}

pub fn validate_tenant_id(value: &str) -> StoreResult<()> {
    validate_bounded_token(value, 128, "tenant_id")
}

pub fn validate_storage_id(value: &str, name: &str) -> StoreResult<()> {
    validate_bounded_token(value, 160, name)
}

pub fn validate_json_object(value: &Value, name: &str) -> StoreResult<()> {
    if matches!(value, Value::Object(_)) {
        return Ok(());
    }
    Err(StoreError::Validation(format!(
        "{name} must be a JSON object"
    )))
}

pub fn empty_object() -> Value {
    Value::Object(Map::new())
}

impl UpsertSource {
    pub fn validate(&self) -> StoreResult<()> {
        validate_tenant_id(&self.tenant_id)
    }
}

impl CreateWatch {
    pub fn validate(&self) -> StoreResult<()> {
        validate_tenant_id(&self.tenant_id)?;
        validate_storage_id(&self.watch_id, "watch_id")?;
        validate_interval(self.interval_seconds)?;
        validate_json_object(&self.cursor, "cursor")?;
        self.query.validate()?;
        Ok(())
    }

    pub fn into_watch(self) -> Watch {
        let next_due = self
            .first_due_epoch_seconds
            .unwrap_or(self.created_at_epoch_seconds);
        Watch {
            tenant_id: self.tenant_id,
            watch_id: self.watch_id,
            query: self.query,
            source_ids: self.source_ids,
            interval_seconds: self.interval_seconds,
            cursor: self.cursor,
            enabled: self.enabled,
            created_at_epoch_seconds: self.created_at_epoch_seconds,
            next_due_epoch_seconds: next_due,
            lease_owner: None,
            lease_expires_epoch_seconds: None,
        }
    }
}

impl ClaimDueWatch {
    pub fn validate(&self) -> StoreResult<()> {
        validate_tenant_id(&self.tenant_id)?;
        validate_storage_id(&self.lease_owner, "lease_owner")?;
        validate_interval(self.lease_seconds)
    }
}

impl DeleteWatch {
    pub fn validate(&self) -> StoreResult<()> {
        validate_tenant_id(&self.tenant_id)?;
        validate_storage_id(&self.watch_id, "watch_id")
    }
}

impl StartWatchRun {
    pub fn validate(&self) -> StoreResult<()> {
        validate_tenant_id(&self.tenant_id)?;
        validate_storage_id(&self.watch_id, "watch_id")?;
        validate_storage_id(&self.run_id, "run_id")?;
        validate_storage_id(&self.idempotency_key, "idempotency_key")?;
        validate_storage_id(&self.lease_owner, "lease_owner")
    }
}

impl CompleteWatchRun {
    pub fn validate(&self) -> StoreResult<()> {
        validate_tenant_id(&self.tenant_id)?;
        validate_storage_id(&self.watch_id, "watch_id")?;
        validate_storage_id(&self.run_id, "run_id")?;
        validate_storage_id(&self.lease_owner, "lease_owner")?;
        validate_json_object(&self.next_cursor, "next_cursor")?;
        for record in &self.records {
            record.validate()?;
        }
        if let Some(delivery) = &self.delivery {
            delivery.validate()?;
        }
        Ok(())
    }
}

impl HistoryRequest {
    pub fn validate(&self) -> StoreResult<()> {
        validate_tenant_id(&self.tenant_id)?;
        if let Some(watch_id) = &self.watch_id {
            validate_storage_id(watch_id, "watch_id")?;
        }
        if self.limit == 0 || self.limit > 1000 {
            return Err(StoreError::Validation(
                "history limit must be between 1 and 1000".into(),
            ));
        }
        Ok(())
    }
}

impl WatchRunDelivery {
    pub fn validate(&self) -> StoreResult<()> {
        validate_storage_id(&self.delivery_id, "delivery_id")?;
        validate_json_object(&self.target, "target")?;
        Ok(())
    }
}

impl ClaimPendingDelivery {
    pub fn validate(&self) -> StoreResult<()> {
        validate_tenant_id(&self.tenant_id)?;
        validate_storage_id(&self.lease_owner, "lease_owner")?;
        validate_interval(self.lease_seconds)
    }
}

impl CompleteDelivery {
    pub fn validate(&self) -> StoreResult<()> {
        validate_tenant_id(&self.tenant_id)?;
        validate_storage_id(&self.delivery_id, "delivery_id")?;
        validate_storage_id(&self.lease_owner, "lease_owner")
    }
}

impl ListDeliveries {
    pub fn validate(&self) -> StoreResult<()> {
        validate_tenant_id(&self.tenant_id)?;
        if let Some(status) = &self.status {
            validate_delivery_status(status)?;
        }
        if self.limit == 0 || self.limit > 1000 {
            return Err(StoreError::Validation(
                "delivery limit must be between 1 and 1000".into(),
            ));
        }
        Ok(())
    }
}

fn validate_delivery_status(value: &str) -> StoreResult<()> {
    if matches!(value, "pending" | "delivering" | "succeeded" | "failed") {
        return Ok(());
    }
    Err(StoreError::Validation("delivery status is invalid".into()))
}

fn validate_interval(value: u64) -> StoreResult<()> {
    if (1..=31_536_000).contains(&value) {
        return Ok(());
    }
    Err(StoreError::Validation(
        "interval_seconds must be between 1 and 31536000".into(),
    ))
}

fn validate_bounded_token(value: &str, max_len: usize, name: &str) -> StoreResult<()> {
    let valid_chars = value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'));
    if !value.is_empty() && value.len() <= max_len && valid_chars {
        return Ok(());
    }
    Err(StoreError::Validation(format!(
        "{name} must be a bounded ASCII token"
    )))
}
