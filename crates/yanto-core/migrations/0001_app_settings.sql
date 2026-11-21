-- Non-secret key/value settings (docs/rewrite/02-data-model.md, app_settings).
-- Secrets never go here; they have dedicated *_enc columns elsewhere.
-- Timestamps are unix milliseconds, UTC.
CREATE TABLE app_settings (
    key        TEXT    NOT NULL PRIMARY KEY,
    value      TEXT    NOT NULL CHECK (json_valid(value)),
    updated_at INTEGER NOT NULL
) STRICT;
