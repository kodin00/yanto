-- Scoped bearer tokens (02 4.1). They belong to a user and inherit that user's
-- per-Project permissions, capped by `scope` (D23).
--
-- DEVIATION from 02: the active-name uniqueness is per user, not global, so a
-- Member cannot probe other users' token names.
CREATE TABLE api_tokens (
    id           TEXT    NOT NULL PRIMARY KEY,
    name         TEXT    NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
    token_hash   TEXT    NOT NULL UNIQUE,
    token_hint   TEXT    NOT NULL,
    scope        TEXT    NOT NULL CHECK (scope IN ('read', 'write', 'admin')),
    user_id      TEXT    NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    expires_at   INTEGER,
    last_used_at INTEGER,
    last_used_ip TEXT,
    revoked_at   INTEGER,
    created_at   INTEGER NOT NULL
) STRICT;

CREATE UNIQUE INDEX api_tokens_active_name_idx ON api_tokens (user_id, name) WHERE revoked_at IS NULL;
CREATE INDEX api_tokens_user_idx ON api_tokens (user_id);
