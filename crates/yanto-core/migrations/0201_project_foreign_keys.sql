-- Add the `projects` foreign keys the M1 tables could not have yet (08-conventions 1).
-- SQLite cannot ALTER TABLE ... ADD FOREIGN KEY, so the two grant tables are rebuilt:
-- create-new / copy / drop / rename. Grants that point at Projects that never existed
-- are dropped (M1 could not validate them).
PRAGMA defer_foreign_keys = ON;

CREATE TABLE user_project_access_new (
    user_id    TEXT    NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    project_id TEXT    NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, project_id)
) STRICT;

INSERT INTO user_project_access_new (user_id, project_id, created_at, updated_at)
SELECT a.user_id, a.project_id, a.created_at, a.updated_at
FROM user_project_access a
WHERE a.project_id IN (SELECT id FROM projects);

CREATE TABLE user_project_permissions_new (
    user_id    TEXT NOT NULL,
    project_id TEXT NOT NULL,
    permission TEXT NOT NULL
        CHECK (permission IN ('deploy', 'runtime', 'config', 'secrets', 'backups', 'hostnames')),
    PRIMARY KEY (user_id, project_id, permission),
    FOREIGN KEY (user_id, project_id)
        REFERENCES user_project_access_new (user_id, project_id) ON DELETE CASCADE
) STRICT;

INSERT INTO user_project_permissions_new (user_id, project_id, permission)
SELECT p.user_id, p.project_id, p.permission
FROM user_project_permissions p
WHERE EXISTS (
    SELECT 1 FROM user_project_access_new a
    WHERE a.user_id = p.user_id AND a.project_id = p.project_id
);

DROP TABLE user_project_permissions;
DROP TABLE user_project_access;
ALTER TABLE user_project_access_new RENAME TO user_project_access;
ALTER TABLE user_project_permissions_new RENAME TO user_project_permissions;

CREATE INDEX user_project_access_project_idx ON user_project_access (project_id);

CREATE TRIGGER user_project_access_no_owner
BEFORE INSERT ON user_project_access
WHEN (SELECT role FROM users WHERE id = NEW.user_id) = 'owner'
BEGIN
    SELECT RAISE(ABORT, 'the Owner has implicit access to every Project');
END;

-- audit_logs keeps its history when a Project is deleted: detach instead of a FK rebuild.
-- The `audit_no_update` trigger (0105) allows exactly this mutation.
CREATE TRIGGER audit_project_detach AFTER DELETE ON projects
BEGIN
    UPDATE audit_logs SET project_id = NULL WHERE project_id = OLD.id;
END;
