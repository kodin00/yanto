-- Single-use invite/reset tokens, 24h TTL (02 4.1). Only the SHA-256 hex is stored.
CREATE TABLE account_tokens (
    id         TEXT    NOT NULL PRIMARY KEY,
    user_id    TEXT    NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    purpose    TEXT    NOT NULL CHECK (purpose IN ('invite', 'reset')),
    token_hash TEXT    NOT NULL UNIQUE,
    expires_at INTEGER NOT NULL,
    used_at    INTEGER,
    created_at INTEGER NOT NULL
) STRICT;

CREATE INDEX account_tokens_user_active_idx ON account_tokens (user_id, used_at);
CREATE INDEX account_tokens_expires_idx ON account_tokens (expires_at);
