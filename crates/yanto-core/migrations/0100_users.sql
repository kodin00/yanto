-- M1 identity: dashboard accounts (docs/rewrite/02-data-model.md 4.1).
-- Exactly one Owner, enforced by the database.
CREATE TABLE users (
    id            TEXT    NOT NULL PRIMARY KEY,
    username      TEXT    NOT NULL COLLATE NOCASE UNIQUE
                          CHECK (length(username) BETWEEN 1 AND 200),
    role          TEXT    NOT NULL CHECK (role IN ('owner', 'member')),
    status        TEXT    NOT NULL CHECK (status IN ('invited', 'active', 'disabled')),
    password_hash TEXT,
    last_login_at INTEGER,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL,
    CHECK (role <> 'owner' OR status <> 'disabled'),
    CHECK (status <> 'active' OR password_hash IS NOT NULL)
) STRICT;

CREATE UNIQUE INDEX users_single_owner ON users (role) WHERE role = 'owner';
CREATE INDEX users_status_idx ON users (status);

CREATE TRIGGER users_owner_no_delete
BEFORE DELETE ON users
WHEN OLD.role = 'owner'
BEGIN
    SELECT RAISE(ABORT, 'the Owner cannot be deleted');
END;

CREATE TRIGGER users_owner_role_immutable
BEFORE UPDATE OF role ON users
WHEN OLD.role <> NEW.role
BEGIN
    SELECT RAISE(ABORT, 'user role cannot change');
END;
