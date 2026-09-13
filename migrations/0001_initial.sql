PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS sources (
    tenant_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    enabled INTEGER NOT NULL,
    observed_at_epoch_seconds INTEGER NOT NULL,
    PRIMARY KEY (tenant_id, source_id)
);

CREATE TABLE IF NOT EXISTS watches (
    tenant_id TEXT NOT NULL,
    watch_id TEXT NOT NULL,
    query_json TEXT NOT NULL,
    source_ids_json TEXT NOT NULL,
    interval_seconds INTEGER NOT NULL,
    cursor_json TEXT NOT NULL,
    enabled INTEGER NOT NULL,
    created_at_epoch_seconds INTEGER NOT NULL,
    next_due_epoch_seconds INTEGER NOT NULL,
    lease_owner TEXT,
    lease_expires_epoch_seconds INTEGER,
    PRIMARY KEY (tenant_id, watch_id)
);

CREATE INDEX IF NOT EXISTS watches_due_idx
ON watches (tenant_id, enabled, next_due_epoch_seconds, lease_expires_epoch_seconds);

CREATE TABLE IF NOT EXISTS watch_runs (
    tenant_id TEXT NOT NULL,
    watch_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    lease_owner TEXT NOT NULL,
    status TEXT NOT NULL,
    started_at_epoch_seconds INTEGER NOT NULL,
    finished_at_epoch_seconds INTEGER,
    error TEXT,
    PRIMARY KEY (tenant_id, watch_id, run_id),
    UNIQUE (tenant_id, watch_id, idempotency_key),
    FOREIGN KEY (tenant_id, watch_id)
        REFERENCES watches (tenant_id, watch_id)
        ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS records (
    tenant_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    record_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    url TEXT NOT NULL,
    title TEXT,
    text TEXT,
    author TEXT,
    created_at TEXT,
    updated_at TEXT,
    metadata_json TEXT NOT NULL,
    first_observed_epoch_seconds INTEGER NOT NULL,
    last_observed_epoch_seconds INTEGER NOT NULL,
    PRIMARY KEY (tenant_id, source_id, record_id)
);

CREATE INDEX IF NOT EXISTS records_history_idx
ON records (tenant_id, last_observed_epoch_seconds);

CREATE TABLE IF NOT EXISTS watch_records (
    tenant_id TEXT NOT NULL,
    watch_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    record_id TEXT NOT NULL,
    first_observed_epoch_seconds INTEGER NOT NULL,
    last_observed_epoch_seconds INTEGER NOT NULL,
    PRIMARY KEY (tenant_id, watch_id, source_id, record_id),
    FOREIGN KEY (tenant_id, watch_id)
        REFERENCES watches (tenant_id, watch_id)
        ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, source_id, record_id)
        REFERENCES records (tenant_id, source_id, record_id)
        ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS watch_records_history_idx
ON watch_records (tenant_id, watch_id, last_observed_epoch_seconds);

CREATE TABLE IF NOT EXISTS deliveries (
    tenant_id TEXT NOT NULL,
    delivery_id TEXT NOT NULL,
    watch_id TEXT NOT NULL,
    run_id TEXT,
    status TEXT NOT NULL,
    target_json TEXT NOT NULL,
    created_at_epoch_seconds INTEGER NOT NULL,
    delivered_at_epoch_seconds INTEGER,
    error TEXT,
    PRIMARY KEY (tenant_id, delivery_id),
    FOREIGN KEY (tenant_id, watch_id)
        REFERENCES watches (tenant_id, watch_id)
        ON DELETE CASCADE
);
