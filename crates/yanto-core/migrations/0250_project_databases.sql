-- Declared Postgres/MySQL services of a Project, so Backups can dump them (02 4.2, D14).
-- Credentials are never stored here: `user_env` / `password_env` name environment variables that
-- exist inside the service's container and are read there (04 7.2). Prefix: `pdb`.
CREATE TABLE project_databases (
    id              TEXT    NOT NULL PRIMARY KEY,
    project_id      TEXT    NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    engine          TEXT    NOT NULL CHECK (engine IN ('postgres', 'mysql')),
    compose_service TEXT    NOT NULL CHECK (length(compose_service) BETWEEN 1 AND 100),
    "database"      TEXT    CHECK ("database" IS NULL OR length("database") BETWEEN 1 AND 100),
    user_env        TEXT    CHECK (user_env IS NULL OR length(user_env) BETWEEN 1 AND 100),
    password_env    TEXT    CHECK (password_env IS NULL OR length(password_env) BETWEEN 1 AND 100),
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
) STRICT;

CREATE UNIQUE INDEX project_databases_service_idx ON project_databases (project_id, compose_service);
