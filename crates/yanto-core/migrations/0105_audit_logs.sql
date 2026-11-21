-- Append-only audit trail (02 4.5).
--
-- `project_id` has no FK yet (see 0103). The append-only trigger allows the one
-- permitted mutation, setting `project_id` to NULL, so M2 can either rebuild the
-- table with `ON DELETE SET NULL` or add an AFTER DELETE trigger on `projects`.
CREATE TABLE audit_logs (
    id          TEXT    NOT NULL PRIMARY KEY,
    actor_type  TEXT    NOT NULL
        CHECK (actor_type IN ('user', 'api_token', 'deploy_token', 'github', 'cli', 'system')),
    actor_id    TEXT,
    actor_label TEXT    NOT NULL,
    action      TEXT    NOT NULL,
    entity_type TEXT    NOT NULL,
    entity_id   TEXT,
    project_id  TEXT,
    ip          TEXT,
    channel     TEXT    NOT NULL
        CHECK (channel IN ('dashboard', 'rest', 'mcp', 'deploy-token', 'webhook', 'cli', 'cron', 'system')),
    outcome     TEXT    NOT NULL DEFAULT 'ok' CHECK (outcome IN ('ok', 'denied', 'error')),
    request_id  TEXT,
    metadata    TEXT    NOT NULL DEFAULT '{}' CHECK (json_valid(metadata)),
    created_at  INTEGER NOT NULL
) STRICT;

CREATE INDEX audit_created_idx ON audit_logs (created_at DESC, id DESC);
CREATE INDEX audit_project_created_idx ON audit_logs (project_id, created_at DESC);
CREATE INDEX audit_actor_idx ON audit_logs (actor_type, actor_id, created_at DESC);

CREATE TRIGGER audit_no_update
BEFORE UPDATE ON audit_logs
WHEN NOT (
    NEW.project_id IS NULL
    AND NEW.id = OLD.id
    AND NEW.actor_type = OLD.actor_type
    AND NEW.actor_id IS OLD.actor_id
    AND NEW.actor_label = OLD.actor_label
    AND NEW.action = OLD.action
    AND NEW.entity_type = OLD.entity_type
    AND NEW.entity_id IS OLD.entity_id
    AND NEW.ip IS OLD.ip
    AND NEW.channel = OLD.channel
    AND NEW.outcome = OLD.outcome
    AND NEW.request_id IS OLD.request_id
    AND NEW.metadata = OLD.metadata
    AND NEW.created_at = OLD.created_at
)
BEGIN
    SELECT RAISE(ABORT, 'audit_logs is append-only');
END;
