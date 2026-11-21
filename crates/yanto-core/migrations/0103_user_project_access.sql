-- Per-Project access for Members (02 4.1).
--
-- DEVIATION (M1): `project_id` has NO foreign key yet because the `projects`
-- table arrives in M2. SQLite cannot add a FOREIGN KEY with ALTER TABLE, so M2's
-- first migration rebuilds both tables with the FK (create-new / copy / drop /
-- rename inside one transaction, see docs/rewrite/08-conventions.md "Adding a
-- foreign key to an M1 table"). Column names and types already match the final
-- schema, so the rebuild is a pure copy.
CREATE TABLE user_project_access (
    user_id    TEXT    NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    project_id TEXT    NOT NULL CHECK (length(project_id) BETWEEN 1 AND 64),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, project_id)
) STRICT;

CREATE INDEX user_project_access_project_idx ON user_project_access (project_id);

CREATE TRIGGER user_project_access_no_owner
BEFORE INSERT ON user_project_access
WHEN (SELECT role FROM users WHERE id = NEW.user_id) = 'owner'
BEGIN
    SELECT RAISE(ABORT, 'the Owner has implicit access to every Project');
END;

CREATE TABLE user_project_permissions (
    user_id    TEXT NOT NULL,
    project_id TEXT NOT NULL,
    permission TEXT NOT NULL
        CHECK (permission IN ('deploy', 'runtime', 'config', 'secrets', 'backups', 'hostnames')),
    PRIMARY KEY (user_id, project_id, permission),
    FOREIGN KEY (user_id, project_id)
        REFERENCES user_project_access (user_id, project_id) ON DELETE CASCADE
) STRICT;
