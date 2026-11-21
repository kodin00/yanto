-- Hosted Registry (D10, 02 4.3): singleton settings and the users that become the htpasswd file.
-- User prefix: `rus`. The htpasswd file is derived from `registry_users`.
CREATE TABLE hosted_registry_settings (
    id                    INTEGER NOT NULL PRIMARY KEY CHECK (id = 1),
    enabled               INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    listen_port           INTEGER NOT NULL DEFAULT 5000 CHECK (listen_port BETWEEN 1 AND 65535),
    container_name        TEXT    NOT NULL DEFAULT 'yanto-registry',
    image_digest          TEXT,
    gc_in_progress        INTEGER NOT NULL DEFAULT 0 CHECK (gc_in_progress IN (0, 1)),
    internal_password_enc TEXT,
    public_host           TEXT,
    updated_at            INTEGER NOT NULL
) STRICT;

INSERT INTO hosted_registry_settings (id, updated_at) VALUES (1, 0);

CREATE TABLE registry_users (
    id            TEXT    NOT NULL PRIMARY KEY,
    username      TEXT    NOT NULL COLLATE NOCASE UNIQUE
        CHECK (username GLOB '[A-Za-z0-9._-]*' AND length(username) <= 64 AND instr(username, ':') = 0),
    password_hash TEXT    NOT NULL,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
) STRICT;
