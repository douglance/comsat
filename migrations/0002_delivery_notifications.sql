ALTER TABLE deliveries ADD COLUMN payload_json TEXT NOT NULL DEFAULT '{}';
ALTER TABLE deliveries ADD COLUMN next_attempt_epoch_seconds INTEGER NOT NULL DEFAULT 0;
ALTER TABLE deliveries ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0;
ALTER TABLE deliveries ADD COLUMN lease_owner TEXT;
ALTER TABLE deliveries ADD COLUMN lease_expires_epoch_seconds INTEGER;

CREATE INDEX IF NOT EXISTS deliveries_pending_idx
ON deliveries (tenant_id, status, next_attempt_epoch_seconds, created_at_epoch_seconds);
