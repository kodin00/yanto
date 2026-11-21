-- Cron Entries and their run history (02 4.4, D11, D26). Prefixes: `crn` (entry), `crr` (run).
--
-- `project_id` gets its `REFERENCES projects (id) ON DELETE SET NULL` in 0550 (`projects` is M2's).
CREATE TABLE cron_entries (
    id          TEXT    NOT NULL PRIMARY KEY,
    name        TEXT    NOT NULL UNIQUE,
    schedule    TEXT    NOT NULL,
    run_as_user TEXT    NOT NULL,
    command     TEXT    NOT NULL CHECK (length(command) > 0 AND length(CAST(command AS BLOB)) <= 4096),
    overlap     TEXT    NOT NULL DEFAULT 'allow' CHECK (overlap IN ('allow', 'skip')),
    timeout_s   INTEGER CHECK (timeout_s IS NULL OR timeout_s > 0),
    enabled     INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    project_id  TEXT,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
) STRICT;

CREATE TABLE cron_runs (
    id          TEXT    NOT NULL PRIMARY KEY,
    entry_id    TEXT    NOT NULL REFERENCES cron_entries (id) ON DELETE CASCADE,
    status      TEXT    NOT NULL CHECK (status IN ('running', 'success', 'failed', 'timeout', 'skipped')),
    exit_code   INTEGER,
    started_at  INTEGER NOT NULL,
    finished_at INTEGER,
    output_tail TEXT
) STRICT;

CREATE INDEX cron_runs_entry_idx ON cron_runs (entry_id, started_at DESC);
