pub(super) const UPSERT_SOURCE_SQL: &str = "INSERT INTO sources (
    tenant_id, source_id, enabled, observed_at_epoch_seconds
)
VALUES (?1, ?2, ?3, ?4)
ON CONFLICT (tenant_id, source_id) DO UPDATE SET
    enabled = excluded.enabled,
    observed_at_epoch_seconds = excluded.observed_at_epoch_seconds";

pub(super) const INSERT_WATCH_SQL: &str = "INSERT INTO watches (
    tenant_id, watch_id, query_json, source_ids_json, interval_seconds,
    cursor_json, enabled, created_at_epoch_seconds, next_due_epoch_seconds
) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)";

pub(super) const LIST_WATCHES_SQL: &str = "SELECT tenant_id, watch_id, query_json,
    source_ids_json, interval_seconds, cursor_json, enabled, created_at_epoch_seconds,
    next_due_epoch_seconds, lease_owner, lease_expires_epoch_seconds
FROM watches
WHERE tenant_id = ?1
ORDER BY created_at_epoch_seconds, watch_id";

pub(super) const DELETE_WATCH_SQL: &str =
    "DELETE FROM watches WHERE tenant_id = ?1 AND watch_id = ?2";

pub(super) const CLAIM_DUE_WATCH_SQL: &str = "UPDATE watches
SET lease_owner = ?1, lease_expires_epoch_seconds = ?2
WHERE tenant_id = ?3
  AND enabled = 1
  AND next_due_epoch_seconds <= ?4
  AND (lease_expires_epoch_seconds IS NULL OR lease_expires_epoch_seconds <= ?4)
  AND watch_id = (
      SELECT watch_id FROM watches
      WHERE tenant_id = ?3
        AND enabled = 1
        AND next_due_epoch_seconds <= ?4
        AND (lease_expires_epoch_seconds IS NULL OR lease_expires_epoch_seconds <= ?4)
      ORDER BY next_due_epoch_seconds, created_at_epoch_seconds, watch_id
      LIMIT 1
  )
RETURNING tenant_id, watch_id, query_json, source_ids_json, interval_seconds,
    cursor_json, enabled, created_at_epoch_seconds, next_due_epoch_seconds,
    lease_owner, lease_expires_epoch_seconds";

pub(super) const START_RUN_SQL: &str = "INSERT INTO watch_runs (
    tenant_id, watch_id, run_id, idempotency_key, lease_owner, status,
    started_at_epoch_seconds
)
SELECT ?1, ?2, ?3, ?4, ?5, 'running', ?6
WHERE EXISTS (
    SELECT 1 FROM watches
    WHERE tenant_id = ?1 AND watch_id = ?2
      AND lease_owner = ?5 AND lease_expires_epoch_seconds > ?6
)
ON CONFLICT (tenant_id, watch_id, idempotency_key) DO NOTHING";

pub(super) const LOAD_RUN_SQL: &str = "SELECT tenant_id, watch_id, run_id,
    idempotency_key, lease_owner, status, started_at_epoch_seconds,
    finished_at_epoch_seconds, error
FROM watch_runs
WHERE tenant_id = ?1 AND watch_id = ?2 AND idempotency_key = ?3";

pub(super) const HISTORY_TENANT_SQL: &str = "SELECT source_id, record_id, kind,
    url, title, text, author, created_at, updated_at, metadata_json,
    NULL AS watch_id, first_observed_epoch_seconds, last_observed_epoch_seconds
FROM records
WHERE tenant_id = ?1 AND last_observed_epoch_seconds >= ?2
ORDER BY last_observed_epoch_seconds DESC, source_id, record_id
LIMIT ?3";

pub(super) const HISTORY_WATCH_SQL: &str = "SELECT r.source_id, r.record_id,
    r.kind, r.url, r.title, r.text, r.author, r.created_at, r.updated_at,
    r.metadata_json, wr.watch_id, wr.first_observed_epoch_seconds,
    wr.last_observed_epoch_seconds
FROM watch_records wr
JOIN records r ON r.tenant_id = wr.tenant_id
    AND r.source_id = wr.source_id AND r.record_id = wr.record_id
WHERE wr.tenant_id = ?1 AND wr.watch_id = ?2
    AND wr.last_observed_epoch_seconds >= ?3
ORDER BY wr.last_observed_epoch_seconds DESC, r.source_id, r.record_id
LIMIT ?4";

pub(super) const INSERT_RUN_DELIVERY_SQL: &str = "WITH incoming AS (
    SELECT json_extract(value, '$.source_id') AS source_id,
        json_extract(value, '$.record_id') AS record_id,
        json_extract(value, '$.kind') AS kind,
        json_extract(value, '$.url') AS url,
        json_extract(value, '$.title') AS title,
        json_extract(value, '$.text') AS text,
        json_extract(value, '$.author') AS author,
        json_extract(value, '$.created_at') AS created_at,
        json_extract(value, '$.updated_at') AS updated_at,
        json_extract(value, '$.metadata_json') AS metadata_json
    FROM json_each(?1)
    GROUP BY json_extract(value, '$.source_id'), json_extract(value, '$.record_id')
), new_records AS (
    SELECT * FROM incoming
    WHERE NOT EXISTS (
        SELECT 1 FROM watch_records
        WHERE tenant_id = ?2 AND watch_id = ?3
          AND source_id = incoming.source_id AND record_id = incoming.record_id
    )
)
INSERT OR IGNORE INTO deliveries (
    tenant_id, delivery_id, watch_id, run_id, status, target_json, payload_json,
    created_at_epoch_seconds, next_attempt_epoch_seconds
)
SELECT ?2, ?7, ?3, ?4, 'pending', ?8,
    json_object(
        'type', 'comsat.watch.records_observed',
        'tenant_id', ?2,
        'watch_id', ?3,
        'run_id', ?4,
        'observed_at_epoch_seconds', ?6,
        'records', json_group_array(json_object(
            'id', record_id,
            'source', source_id,
            'kind', kind,
            'url', url,
            'title', title,
            'text', text,
            'author', author,
            'created_at', created_at,
            'updated_at', updated_at,
            'metadata', json(metadata_json)
        ))
    ),
    ?6, ?6
FROM new_records
WHERE EXISTS (
    SELECT 1 FROM watches w JOIN watch_runs r
    ON r.tenant_id = w.tenant_id AND r.watch_id = w.watch_id
    WHERE w.tenant_id = ?2 AND w.watch_id = ?3 AND r.run_id = ?4
      AND w.lease_owner = ?5 AND r.lease_owner = ?5
      AND w.lease_expires_epoch_seconds > ?6 AND r.status = 'running'
)
HAVING COUNT(*) > 0";

pub(super) const CLAIM_DELIVERY_SQL: &str = "UPDATE deliveries
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
      OR (status = 'delivering' AND lease_expires_epoch_seconds <= ?4))
RETURNING tenant_id, delivery_id, watch_id, run_id, status, target_json,
    payload_json, created_at_epoch_seconds, next_attempt_epoch_seconds,
    attempts, lease_owner, lease_expires_epoch_seconds, delivered_at_epoch_seconds, error";

pub(super) const COMPLETE_DELIVERY_SQL: &str = "UPDATE deliveries
SET status = CASE
        WHEN ?1 = 1 THEN 'succeeded'
        WHEN attempts >= ?2 THEN 'failed'
        ELSE 'pending'
    END,
    next_attempt_epoch_seconds = ?3,
    delivered_at_epoch_seconds = ?4,
    error = ?5,
    lease_owner = NULL,
    lease_expires_epoch_seconds = NULL
WHERE tenant_id = ?6 AND delivery_id = ?7 AND lease_owner = ?8
  AND status = 'delivering' AND lease_expires_epoch_seconds > ?9
RETURNING tenant_id, delivery_id, watch_id, run_id, status, target_json,
    payload_json, created_at_epoch_seconds, next_attempt_epoch_seconds,
    attempts, lease_owner, lease_expires_epoch_seconds, delivered_at_epoch_seconds, error";

pub(super) const LIST_DELIVERIES_SQL: &str = "SELECT tenant_id, delivery_id,
    watch_id, run_id, status, target_json, payload_json, created_at_epoch_seconds,
    next_attempt_epoch_seconds, attempts, lease_owner, lease_expires_epoch_seconds,
    delivered_at_epoch_seconds, error
FROM deliveries
WHERE tenant_id = ?1
ORDER BY created_at_epoch_seconds, delivery_id
LIMIT ?2";

pub(super) const LIST_DELIVERIES_STATUS_SQL: &str = "SELECT tenant_id,
    delivery_id, watch_id, run_id, status, target_json, payload_json,
    created_at_epoch_seconds, next_attempt_epoch_seconds, attempts, lease_owner,
    lease_expires_epoch_seconds, delivered_at_epoch_seconds, error
FROM deliveries
WHERE tenant_id = ?1 AND status = ?2
ORDER BY created_at_epoch_seconds, delivery_id
LIMIT ?3";

pub(super) const COMPLETE_GATE_SQL: &str = "SELECT w.interval_seconds AS interval_seconds
FROM watches w
JOIN watch_runs r ON r.tenant_id = w.tenant_id AND r.watch_id = w.watch_id
WHERE w.tenant_id = ?1 AND w.watch_id = ?2 AND r.run_id = ?3
  AND w.lease_owner = ?4 AND w.lease_expires_epoch_seconds > ?5
  AND r.lease_owner = ?4 AND r.status = 'running'";

pub(super) const INSERT_RECORDS_SQL: &str = "WITH incoming AS (
    SELECT json_extract(value, '$.source_id') AS source_id,
        json_extract(value, '$.record_id') AS record_id,
        json_extract(value, '$.kind') AS kind,
        json_extract(value, '$.url') AS url,
        json_extract(value, '$.title') AS title,
        json_extract(value, '$.text') AS text,
        json_extract(value, '$.author') AS author,
        json_extract(value, '$.created_at') AS created_at,
        json_extract(value, '$.updated_at') AS updated_at,
        json_extract(value, '$.metadata_json') AS metadata_json
    FROM json_each(?1)
)
INSERT INTO records (
    tenant_id, source_id, record_id, kind, url, title, text, author, created_at,
    updated_at, metadata_json, first_observed_epoch_seconds, last_observed_epoch_seconds
)
SELECT ?2, source_id, record_id, kind, url, title, text, author, created_at,
    updated_at, metadata_json, ?6, ?6
FROM incoming
WHERE EXISTS (
    SELECT 1 FROM watches w JOIN watch_runs r
    ON r.tenant_id = w.tenant_id AND r.watch_id = w.watch_id
    WHERE w.tenant_id = ?2 AND w.watch_id = ?3 AND r.run_id = ?4
      AND w.lease_owner = ?5 AND r.lease_owner = ?5
      AND w.lease_expires_epoch_seconds > ?6 AND r.status = 'running'
)
ON CONFLICT (tenant_id, source_id, record_id) DO NOTHING";

pub(super) const UPDATE_RECORDS_SQL: &str = "WITH incoming AS (
    SELECT json_extract(value, '$.source_id') AS source_id,
        json_extract(value, '$.record_id') AS record_id,
        json_extract(value, '$.kind') AS kind,
        json_extract(value, '$.url') AS url,
        json_extract(value, '$.title') AS title,
        json_extract(value, '$.text') AS text,
        json_extract(value, '$.author') AS author,
        json_extract(value, '$.created_at') AS created_at,
        json_extract(value, '$.updated_at') AS updated_at,
        json_extract(value, '$.metadata_json') AS metadata_json
    FROM json_each(?1)
)
UPDATE records
SET kind = (SELECT kind FROM incoming WHERE source_id = records.source_id AND record_id = records.record_id LIMIT 1),
    url = (SELECT url FROM incoming WHERE source_id = records.source_id AND record_id = records.record_id LIMIT 1),
    title = (SELECT title FROM incoming WHERE source_id = records.source_id AND record_id = records.record_id LIMIT 1),
    text = (SELECT text FROM incoming WHERE source_id = records.source_id AND record_id = records.record_id LIMIT 1),
    author = (SELECT author FROM incoming WHERE source_id = records.source_id AND record_id = records.record_id LIMIT 1),
    created_at = (SELECT created_at FROM incoming WHERE source_id = records.source_id AND record_id = records.record_id LIMIT 1),
    updated_at = (SELECT updated_at FROM incoming WHERE source_id = records.source_id AND record_id = records.record_id LIMIT 1),
    metadata_json = (SELECT metadata_json FROM incoming WHERE source_id = records.source_id AND record_id = records.record_id LIMIT 1),
    last_observed_epoch_seconds = ?6
WHERE tenant_id = ?2
  AND EXISTS (SELECT 1 FROM incoming WHERE source_id = records.source_id AND record_id = records.record_id)
  AND EXISTS (
    SELECT 1 FROM watches w JOIN watch_runs r
    ON r.tenant_id = w.tenant_id AND r.watch_id = w.watch_id
    WHERE w.tenant_id = ?2 AND w.watch_id = ?3 AND r.run_id = ?4
      AND w.lease_owner = ?5 AND r.lease_owner = ?5
      AND w.lease_expires_epoch_seconds > ?6 AND r.status = 'running'
  )";

pub(super) const INSERT_WATCH_RECORDS_SQL: &str = "WITH incoming AS (
    SELECT json_extract(value, '$.source_id') AS source_id,
        json_extract(value, '$.record_id') AS record_id
    FROM json_each(?1)
)
INSERT INTO watch_records (
    tenant_id, watch_id, source_id, record_id, first_observed_epoch_seconds,
    last_observed_epoch_seconds
)
SELECT ?2, ?3, source_id, record_id, ?6, ?6
FROM incoming
WHERE EXISTS (
    SELECT 1 FROM watches w JOIN watch_runs r
    ON r.tenant_id = w.tenant_id AND r.watch_id = w.watch_id
    WHERE w.tenant_id = ?2 AND w.watch_id = ?3 AND r.run_id = ?4
      AND w.lease_owner = ?5 AND r.lease_owner = ?5
      AND w.lease_expires_epoch_seconds > ?6 AND r.status = 'running'
)
ON CONFLICT (tenant_id, watch_id, source_id, record_id) DO NOTHING";

pub(super) const UPDATE_WATCH_RECORDS_SQL: &str = "WITH incoming AS (
    SELECT json_extract(value, '$.source_id') AS source_id,
        json_extract(value, '$.record_id') AS record_id
    FROM json_each(?1)
)
UPDATE watch_records
SET last_observed_epoch_seconds = ?6
WHERE tenant_id = ?2 AND watch_id = ?3
  AND EXISTS (SELECT 1 FROM incoming WHERE source_id = watch_records.source_id AND record_id = watch_records.record_id)
  AND EXISTS (
    SELECT 1 FROM watches w JOIN watch_runs r
    ON r.tenant_id = w.tenant_id AND r.watch_id = w.watch_id
    WHERE w.tenant_id = ?2 AND w.watch_id = ?3 AND r.run_id = ?4
      AND w.lease_owner = ?5 AND r.lease_owner = ?5
      AND w.lease_expires_epoch_seconds > ?6 AND r.status = 'running'
  )";

pub(super) const COMPLETE_RUN_SQL: &str = "UPDATE watch_runs
SET status = ?1, finished_at_epoch_seconds = ?2, error = ?3
WHERE tenant_id = ?4 AND watch_id = ?5 AND run_id = ?6
  AND lease_owner = ?7 AND status = 'running'
  AND EXISTS (
    SELECT 1 FROM watches
    WHERE tenant_id = ?4 AND watch_id = ?5
      AND lease_owner = ?7 AND lease_expires_epoch_seconds > ?2
  )";

pub(super) const COMPLETE_WATCH_SQL: &str = "UPDATE watches
SET cursor_json = ?1,
    next_due_epoch_seconds = ?2 + interval_seconds,
    lease_owner = NULL,
    lease_expires_epoch_seconds = NULL
WHERE tenant_id = ?3 AND watch_id = ?4
  AND lease_owner = ?5 AND lease_expires_epoch_seconds > ?6
  AND EXISTS (
    SELECT 1 FROM watch_runs
    WHERE tenant_id = ?3 AND watch_id = ?4 AND run_id = ?7
      AND lease_owner = ?5 AND status IN ('succeeded', 'failed')
      AND finished_at_epoch_seconds = ?2
  )";
