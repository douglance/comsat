-- Durable Code Mode lifecycle: executions, their oversized artifacts, and
-- saved snippets survive the process that produced them.
CREATE TABLE IF NOT EXISTS codemode_executions (
    id TEXT PRIMARY KEY,
    updated_at INTEGER NOT NULL,
    state TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS codemode_executions_updated_at
    ON codemode_executions(updated_at DESC);

CREATE TABLE IF NOT EXISTS codemode_artifacts (
    execution_id TEXT NOT NULL,
    artifact_id TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (execution_id, artifact_id)
);

CREATE TABLE IF NOT EXISTS codemode_snippets (
    name TEXT PRIMARY KEY,
    saved_at INTEGER NOT NULL,
    snippet TEXT NOT NULL
);
