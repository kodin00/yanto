ALTER TABLE "agent_tasks" ADD COLUMN IF NOT EXISTS "archived_at" timestamp with time zone;

CREATE INDEX IF NOT EXISTS "agent_tasks_archived_updated_idx"
ON "agent_tasks" ("archived_at", "updated_at");
