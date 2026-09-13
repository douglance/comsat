pub const WATCH_SELECT_SQL: &str = "
    SELECT tenant_id, watch_id, query_json, source_ids_json, interval_seconds,
        cursor_json, enabled, created_at_epoch_seconds, next_due_epoch_seconds,
        lease_owner, lease_expires_epoch_seconds
    FROM watches
    WHERE tenant_id = ?1
    ORDER BY created_at_epoch_seconds, watch_id";

pub const DUE_WATCH_SQL: &str = "
    SELECT tenant_id, watch_id, query_json, source_ids_json, interval_seconds,
        cursor_json, enabled, created_at_epoch_seconds, next_due_epoch_seconds,
        lease_owner, lease_expires_epoch_seconds
    FROM watches
    WHERE tenant_id = ?1 AND enabled = 1 AND next_due_epoch_seconds <= ?2
      AND (lease_expires_epoch_seconds IS NULL OR lease_expires_epoch_seconds <= ?2)
    ORDER BY next_due_epoch_seconds, created_at_epoch_seconds, watch_id
    LIMIT 1";

pub const CLAIM_WATCH_SQL: &str = "
    UPDATE watches
    SET lease_owner = ?1, lease_expires_epoch_seconds = ?2
    WHERE tenant_id = ?3 AND watch_id = ?4 AND enabled = 1
      AND next_due_epoch_seconds <= ?5
      AND (lease_expires_epoch_seconds IS NULL OR lease_expires_epoch_seconds <= ?5)";

pub const START_RUN_SQL: &str = "
    INSERT INTO watch_runs (
        tenant_id, watch_id, run_id, idempotency_key, lease_owner, status, started_at_epoch_seconds
    )
    SELECT ?1, ?2, ?3, ?4, ?5, 'running', ?6
      WHERE EXISTS (
          SELECT 1 FROM watches
          WHERE tenant_id = ?1 AND watch_id = ?2
            AND lease_owner = ?5 AND lease_expires_epoch_seconds > ?6
      )
    ON CONFLICT (tenant_id, watch_id, idempotency_key) DO NOTHING";

pub const CLAIM_DELIVERY_SQL: &str = "
    UPDATE deliveries
    SET status = 'delivering',
        attempts = attempts + 1,
        lease_owner = ?1,
        lease_expires_epoch_seconds = ?2,
        error = NULL
    WHERE tenant_id = ?3
      AND delivery_id = (
          SELECT delivery_id FROM deliveries
          WHERE tenant_id = ?3
            AND ((status = 'pending' AND next_attempt_epoch_seconds <= ?4)
                OR (status = 'delivering' AND lease_expires_epoch_seconds <= ?4))
          ORDER BY next_attempt_epoch_seconds, created_at_epoch_seconds, delivery_id
          LIMIT 1
      )
      AND ((status = 'pending' AND next_attempt_epoch_seconds <= ?4)
          OR (status = 'delivering' AND lease_expires_epoch_seconds <= ?4))";
