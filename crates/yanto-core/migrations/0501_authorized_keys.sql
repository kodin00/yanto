-- Authorized Keys rendered into the marked block of the configured Unix user's
-- ~/.ssh/authorized_keys (02 4.4, D12). Prefix: `akey`.
CREATE TABLE authorized_keys (
    id          TEXT    NOT NULL PRIMARY KEY,
    name        TEXT    NOT NULL UNIQUE,
    key_type    TEXT    NOT NULL,
    public_key  TEXT    NOT NULL CHECK (instr(public_key, char(10)) = 0 AND instr(public_key, char(13)) = 0),
    fingerprint TEXT    NOT NULL UNIQUE,
    enabled     INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    created_at  INTEGER NOT NULL
) STRICT;
