use crate::{
    codec::{collect_rows, delivery_from_row, json_text, map_sql},
    sql::CLAIM_DELIVERY_SQL,
};
use comsat_store::{
    ClaimPendingDelivery, CompleteDelivery, CompleteWatchRun, DELIVERY_RETRY_SECONDS, Delivery,
    ListDeliveries, MAX_DELIVERY_ATTEMPTS, StoreError, StoreResult,
};
use comsat_types::Record;
use rusqlite::{Connection, OptionalExtension, Transaction, params};

pub fn claim_pending_delivery(
    transaction: &Transaction<'_>,
    request: &ClaimPendingDelivery,
) -> StoreResult<Option<Delivery>> {
    let lease_seconds = i64::try_from(request.lease_seconds)
        .map_err(|_| StoreError::Validation("lease_seconds is too large".into()))?;
    let lease_expires = request.now_epoch_seconds + lease_seconds;
    let changed = transaction
        .execute(
            CLAIM_DELIVERY_SQL,
            params![
                request.lease_owner,
                lease_expires,
                request.tenant_id,
                request.now_epoch_seconds,
            ],
        )
        .map_err(map_sql)?;
    if changed == 0 {
        return Ok(None);
    }
    fetch_claimed_delivery(transaction, request)
}

pub fn complete_delivery(
    connection: &Connection,
    request: &CompleteDelivery,
) -> StoreResult<Delivery> {
    let status = delivery_status(request.success);
    let next_attempt = next_attempt_epoch(request);
    let delivered_at = request.success.then_some(request.finished_at_epoch_seconds);
    let changed = connection
        .execute(
            COMPLETE_DELIVERY_SQL,
            params![
                status,
                next_attempt,
                delivered_at,
                request.error,
                request.tenant_id,
                request.delivery_id,
                request.lease_owner,
                i64::from(MAX_DELIVERY_ATTEMPTS),
                request.finished_at_epoch_seconds,
            ],
        )
        .map_err(map_sql)?;
    if changed != 1 {
        return Err(StoreError::Conflict(
            "delivery is not leased by this worker".into(),
        ));
    }
    fetch_delivery(connection, &request.tenant_id, &request.delivery_id)
}

pub fn list_deliveries(
    connection: &Connection,
    request: &ListDeliveries,
) -> StoreResult<Vec<Delivery>> {
    if let Some(status) = &request.status {
        let mut statement = connection
            .prepare(LIST_DELIVERIES_STATUS_SQL)
            .map_err(map_sql)?;
        return collect_rows(
            statement
                .query_map(
                    params![request.tenant_id, status, request.limit],
                    delivery_from_row,
                )
                .map_err(map_sql)?,
        );
    }
    let mut statement = connection.prepare(LIST_DELIVERIES_SQL).map_err(map_sql)?;
    collect_rows(
        statement
            .query_map(params![request.tenant_id, request.limit], delivery_from_row)
            .map_err(map_sql)?,
    )
}

pub fn insert_run_delivery(
    transaction: &Transaction<'_>,
    request: &CompleteWatchRun,
    records: &[Record],
) -> StoreResult<()> {
    let Some(delivery) = &request.delivery else {
        return Ok(());
    };
    if records.is_empty() {
        return Ok(());
    }
    let payload = serde_json::json!({
        "type": "comsat.watch.records_observed",
        "tenant_id": request.tenant_id,
        "watch_id": request.watch_id,
        "run_id": request.run_id,
        "observed_at_epoch_seconds": request.finished_at_epoch_seconds,
        "records": records,
    });
    transaction
        .execute(
            INSERT_RUN_DELIVERY_SQL,
            params![
                request.tenant_id,
                delivery.delivery_id,
                request.watch_id,
                request.run_id,
                json_text(&delivery.target)?,
                json_text(&payload)?,
                request.finished_at_epoch_seconds,
            ],
        )
        .map_err(map_sql)?;
    Ok(())
}

fn fetch_claimed_delivery(
    transaction: &Transaction<'_>,
    request: &ClaimPendingDelivery,
) -> StoreResult<Option<Delivery>> {
    transaction
        .query_row(
            CLAIMED_DELIVERY_SELECT_SQL,
            params![request.tenant_id, request.lease_owner],
            delivery_from_row,
        )
        .optional()
        .map_err(map_sql)
}

fn fetch_delivery(
    connection: &Connection,
    tenant_id: &str,
    delivery_id: &str,
) -> StoreResult<Delivery> {
    connection
        .query_row(
            DELIVERY_BY_ID_SQL,
            params![tenant_id, delivery_id],
            delivery_from_row,
        )
        .optional()
        .map_err(map_sql)?
        .ok_or_else(|| StoreError::Conflict("delivery was not found".into()))
}

const fn delivery_status(success: bool) -> &'static str {
    if success { "succeeded" } else { "pending" }
}

const fn next_attempt_epoch(request: &CompleteDelivery) -> i64 {
    if request.success {
        return request.finished_at_epoch_seconds;
    }
    request.finished_at_epoch_seconds + DELIVERY_RETRY_SECONDS
}

const INSERT_RUN_DELIVERY_SQL: &str = "INSERT INTO deliveries (
    tenant_id, delivery_id, watch_id, run_id, status, target_json, payload_json,
    created_at_epoch_seconds, next_attempt_epoch_seconds
) VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?6, ?7, ?7)
ON CONFLICT (tenant_id, delivery_id) DO NOTHING";

const CLAIMED_DELIVERY_SELECT_SQL: &str = "SELECT tenant_id, delivery_id, watch_id, run_id,
    status, target_json, payload_json, created_at_epoch_seconds, next_attempt_epoch_seconds,
    attempts, lease_owner, lease_expires_epoch_seconds, delivered_at_epoch_seconds, error
FROM deliveries
WHERE tenant_id = ?1 AND lease_owner = ?2 AND status = 'delivering'
ORDER BY lease_expires_epoch_seconds DESC, delivery_id
LIMIT 1";

const DELIVERY_BY_ID_SQL: &str = "SELECT tenant_id, delivery_id, watch_id, run_id,
    status, target_json, payload_json, created_at_epoch_seconds, next_attempt_epoch_seconds,
    attempts, lease_owner, lease_expires_epoch_seconds, delivered_at_epoch_seconds, error
FROM deliveries
WHERE tenant_id = ?1 AND delivery_id = ?2";

const COMPLETE_DELIVERY_SQL: &str = "UPDATE deliveries
SET status = CASE
        WHEN ?1 = 'succeeded' THEN 'succeeded'
        WHEN attempts >= ?8 THEN 'failed'
        ELSE 'pending'
    END,
    next_attempt_epoch_seconds = ?2,
    delivered_at_epoch_seconds = ?3,
    error = ?4,
    lease_owner = NULL,
    lease_expires_epoch_seconds = NULL
WHERE tenant_id = ?5 AND delivery_id = ?6 AND lease_owner = ?7
  AND status = 'delivering' AND lease_expires_epoch_seconds > ?9";

const LIST_DELIVERIES_SQL: &str = "SELECT tenant_id, delivery_id, watch_id, run_id,
    status, target_json, payload_json, created_at_epoch_seconds, next_attempt_epoch_seconds,
    attempts, lease_owner, lease_expires_epoch_seconds, delivered_at_epoch_seconds, error
FROM deliveries
WHERE tenant_id = ?1
ORDER BY created_at_epoch_seconds, delivery_id
LIMIT ?2";

const LIST_DELIVERIES_STATUS_SQL: &str = "SELECT tenant_id, delivery_id, watch_id, run_id,
    status, target_json, payload_json, created_at_epoch_seconds, next_attempt_epoch_seconds,
    attempts, lease_owner, lease_expires_epoch_seconds, delivered_at_epoch_seconds, error
FROM deliveries
WHERE tenant_id = ?1 AND status = ?2
ORDER BY created_at_epoch_seconds, delivery_id
LIMIT ?3";
