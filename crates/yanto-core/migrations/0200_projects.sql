-- Projects, Deploy Keys, Registry Credentials, declared ports, Deployments (02 4.2).
-- Order matters: `projects` references the two credential tables.

CREATE TABLE deploy_keys (
    id              TEXT    NOT NULL PRIMARY KEY,
    name            TEXT    NOT NULL UNIQUE CHECK (length(name) BETWEEN 1 AND 100),
    key_type        TEXT    NOT NULL CHECK (key_type IN ('ed25519', 'rsa', 'ecdsa')),
    public_key      TEXT    NOT NULL,
    fingerprint     TEXT    NOT NULL UNIQUE,
    private_key_enc TEXT    NOT NULL,
    created_at      INTEGER NOT NULL
) STRICT;

CREATE TABLE registry_credentials (
    id            TEXT    NOT NULL PRIMARY KEY,
    name          TEXT    NOT NULL UNIQUE CHECK (length(name) BETWEEN 1 AND 100),
    registry_host TEXT    NOT NULL,
    username      TEXT    NOT NULL,
    secret_enc    TEXT    NOT NULL,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
) STRICT;

CREATE TABLE projects (
    id                     TEXT    NOT NULL PRIMARY KEY,
    name                   TEXT    NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
    slug                   TEXT    NOT NULL UNIQUE
        CHECK (slug GLOB '[a-z0-9]*' AND slug NOT GLOB '*[^a-z0-9_-]*' AND length(slug) <= 63),
    source_kind            TEXT    NOT NULL CHECK (source_kind IN ('git', 'image')),
    git_url                TEXT,
    branch                 TEXT,
    image_ref              TEXT,
    compose_path           TEXT    NOT NULL DEFAULT 'docker-compose.yml'
        CHECK (compose_path NOT LIKE '/%' AND compose_path NOT LIKE '%..%'),
    compose_content        TEXT,
    env_file               TEXT    NOT NULL DEFAULT '.env'
        CHECK (env_file NOT LIKE '/%' AND env_file NOT LIKE '%..%'),
    auto_start             INTEGER NOT NULL DEFAULT 1 CHECK (auto_start IN (0, 1)),
    manual_deploy_enabled  INTEGER NOT NULL DEFAULT 1 CHECK (manual_deploy_enabled IN (0, 1)),
    github_webhook_enabled INTEGER NOT NULL DEFAULT 1 CHECK (github_webhook_enabled IN (0, 1)),
    deploy_token_hash      TEXT    NOT NULL,
    deploy_token_enc       TEXT    NOT NULL,
    deploy_key_id          TEXT    REFERENCES deploy_keys (id) ON DELETE SET NULL,
    registry_credential_id TEXT    REFERENCES registry_credentials (id) ON DELETE SET NULL,
    created_at             INTEGER NOT NULL,
    updated_at             INTEGER NOT NULL,
    CHECK (
        (source_kind = 'git' AND git_url IS NOT NULL AND branch IS NOT NULL AND image_ref IS NULL)
        OR (source_kind = 'image' AND image_ref IS NOT NULL AND git_url IS NULL)
    )
) STRICT;

CREATE INDEX projects_created_idx ON projects (created_at);
CREATE INDEX projects_deploy_key_idx ON projects (deploy_key_id);
CREATE INDEX projects_registry_credential_idx ON projects (registry_credential_id);

CREATE TABLE project_ports (
    id              TEXT    NOT NULL PRIMARY KEY,
    project_id      TEXT    NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    label           TEXT    NOT NULL CHECK (length(label) BETWEEN 1 AND 64),
    compose_service TEXT,
    host_port       INTEGER NOT NULL CHECK (host_port BETWEEN 1 AND 65535),
    container_port  INTEGER NOT NULL CHECK (container_port BETWEEN 1 AND 65535),
    bind_address    TEXT    NOT NULL DEFAULT '127.0.0.1' CHECK (bind_address IN ('127.0.0.1', '::1')),
    protocol        TEXT    NOT NULL DEFAULT 'tcp' CHECK (protocol IN ('tcp', 'udp')),
    created_at      INTEGER NOT NULL
) STRICT;

CREATE UNIQUE INDEX project_ports_host_port_idx ON project_ports (host_port, protocol);
CREATE UNIQUE INDEX project_ports_label_idx ON project_ports (project_id, label);

CREATE TABLE deployments (
    id                          TEXT    NOT NULL PRIMARY KEY,
    project_id                  TEXT    NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    status                      TEXT    NOT NULL
        CHECK (status IN ('queued', 'running', 'success', 'failed', 'cancelled', 'interrupted')),
    "trigger"                   TEXT    NOT NULL
        CHECK ("trigger" IN ('manual', 'api', 'deploy_token', 'github', 'rollback', 'cron')),
    triggered_by                TEXT,
    target_ref                  TEXT,
    commit_sha                  TEXT,
    commit_message              TEXT,
    rollback_from_deployment_id TEXT    REFERENCES deployments (id) ON DELETE SET NULL,
    image_digest                TEXT,
    compose_snapshot            TEXT,
    env_snapshot_enc            TEXT,
    pending_env_enc             TEXT,
    exit_code                   INTEGER,
    error                       TEXT,
    log_bytes                   INTEGER NOT NULL DEFAULT 0,
    log_truncated               INTEGER NOT NULL DEFAULT 0 CHECK (log_truncated IN (0, 1)),
    queued_at                   INTEGER NOT NULL,
    started_at                  INTEGER,
    finished_at                 INTEGER
) STRICT;

CREATE UNIQUE INDEX deployments_one_running ON deployments (project_id) WHERE status = 'running';
CREATE UNIQUE INDEX deployments_one_queued ON deployments (project_id) WHERE status = 'queued';
CREATE INDEX deployments_project_queued_idx ON deployments (project_id, queued_at DESC);
CREATE INDEX deployments_status_queued_idx ON deployments (status, queued_at);
CREATE INDEX deployments_queued_at_idx ON deployments (queued_at DESC);
CREATE INDEX deployments_rollback_idx ON deployments (rollback_from_deployment_id);

CREATE TABLE deployment_logs (
    deployment_id TEXT    NOT NULL REFERENCES deployments (id) ON DELETE CASCADE,
    seq           INTEGER NOT NULL,
    ts            INTEGER NOT NULL,
    stream        TEXT    NOT NULL CHECK (stream IN ('stdout', 'stderr', 'system')),
    data          TEXT    NOT NULL,
    PRIMARY KEY (deployment_id, seq)
) STRICT, WITHOUT ROWID;
