-- Local, downloadable Backups (02 4.4, D14). Prefix: `bak`.
--
-- `project_id` and `database_id` have no FK yet: `projects` / `project_databases` belong to M2.
-- TODO(M2-merge): rebuild with `ON DELETE SET NULL` FKs in a follow-up 05xx migration.
CREATE TABLE backups (
    id             TEXT    NOT NULL PRIMARY KEY,
    project_id     TEXT,
    kind           TEXT    NOT NULL
        CHECK (kind IN ('project_volumes', 'project_postgres', 'project_mysql', 'yanto_sqlite')),
    database_id    TEXT,
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

CREATE INDEX backups_created_idx ON backups (created_at DESC);
CREATE INDEX backups_project_idx ON backups (project_id, created_at DESC);
CREATE INDEX backups_status_idx ON backups (status);
