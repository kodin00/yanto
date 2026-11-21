-- Adds the Project foreign keys that M5 could not declare before `projects` (0200) and
-- `project_databases` (0250) existed (08-conventions section 1 recipe). SQLite cannot ADD a
-- foreign key, so `cron_entries` and `backups` are rebuilt; every index is recreated.
--
-- `cron_runs` references `cron_entries ON DELETE CASCADE`, and DROP TABLE on a parent performs
-- an implicit DELETE that would cascade, so the run history is parked in a plain table first.
-- Values that point at rows which do not exist are set to NULL during the copy.
PRAGMA defer_foreign_keys = ON;

CREATE TABLE cron_runs_save AS SELECT * FROM cron_runs;
DROP TABLE cron_runs;

CREATE TABLE cron_entries_new (
    id          TEXT    NOT NULL PRIMARY KEY,
    name        TEXT    NOT NULL UNIQUE,
    schedule    TEXT    NOT NULL,
    run_as_user TEXT    NOT NULL,
    command     TEXT    NOT NULL CHECK (length(command) > 0 AND length(CAST(command AS BLOB)) <= 4096),
    overlap     TEXT    NOT NULL DEFAULT 'allow' CHECK (overlap IN ('allow', 'skip')),
    timeout_s   INTEGER CHECK (timeout_s IS NULL OR timeout_s > 0),
    enabled     INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    project_id  TEXT    REFERENCES projects (id) ON DELETE SET NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
) STRICT;
INSERT INTO cron_entries_new
    SELECT id, name, schedule, run_as_user, command, overlap, timeout_s, enabled,
           CASE WHEN project_id IN (SELECT id FROM projects) THEN project_id END,
           created_at, updated_at
    FROM cron_entries;
DROP TABLE cron_entries;
ALTER TABLE cron_entries_new RENAME TO cron_entries;
CREATE INDEX cron_entries_project_idx ON cron_entries (project_id);

CREATE TABLE cron_runs (
    id          TEXT    NOT NULL PRIMARY KEY,
    entry_id    TEXT    NOT NULL REFERENCES cron_entries (id) ON DELETE CASCADE,
    status      TEXT    NOT NULL CHECK (status IN ('running', 'success', 'failed', 'timeout', 'skipped')),
    exit_code   INTEGER,
    started_at  INTEGER NOT NULL,
    finished_at INTEGER,
    output_tail TEXT
) STRICT;
INSERT INTO cron_runs
    SELECT id, entry_id, status, exit_code, started_at, finished_at, output_tail FROM cron_runs_save;
DROP TABLE cron_runs_save;
CREATE INDEX cron_runs_entry_idx ON cron_runs (entry_id, started_at DESC);

CREATE TABLE backups_new (
    id             TEXT    NOT NULL PRIMARY KEY,
    project_id     TEXT    REFERENCES projects (id) ON DELETE SET NULL,
    kind           TEXT    NOT NULL
        CHECK (kind IN ('project_volumes', 'project_postgres', 'project_mysql', 'yanto_sqlite')),
    database_id    TEXT    REFERENCES project_databases (id) ON DELETE SET NULL,
    status         TEXT    NOT NULL CHECK (status IN ('running', 'success', 'failed')),
    trigger        TEXT    NOT NULL CHECK (trigger IN ('manual', 'api', 'cron', 'pre_restore')),
    filename       TEXT    NOT NULL,
    file_path      TEXT    NOT NULL,
    size_bytes     INTEGER,
    sha256         TEXT,
    error          TEXT,
    note           TEXT,
    created_at     INTEGER NOT NULL,
    finished_at    INTEGER,
    downloaded_at  INTEGER,
    download_count INTEGER NOT NULL DEFAULT 0
) STRICT;
INSERT INTO backups_new
    SELECT id,
           CASE WHEN project_id IN (SELECT id FROM projects) THEN project_id END,
           kind,
           CASE WHEN database_id IN (SELECT id FROM project_databases) THEN database_id END,
           status, trigger, filename, file_path, size_bytes, sha256, error, note,
           created_at, finished_at, downloaded_at, download_count
    FROM backups;
DROP TABLE backups;
ALTER TABLE backups_new RENAME TO backups;
CREATE INDEX backups_created_idx ON backups (created_at DESC);
CREATE INDEX backups_project_idx ON backups (project_id, created_at DESC);
CREATE INDEX backups_status_idx ON backups (status);
