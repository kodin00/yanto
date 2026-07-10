ALTER TABLE "projects" ADD COLUMN IF NOT EXISTS "agent_image" text DEFAULT '' NOT NULL;

CREATE TABLE IF NOT EXISTS "ai_providers" (
  "id" text PRIMARY KEY NOT NULL,
  "name" text NOT NULL,
  "protocol" text NOT NULL,
  "base_url" text NOT NULL,
  "api_key" text NOT NULL,
  "enabled" boolean DEFAULT true NOT NULL,
  "created_at" timestamp with time zone DEFAULT now() NOT NULL,
  "updated_at" timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE IF NOT EXISTS "ai_models" (
  "id" text PRIMARY KEY NOT NULL,
  "provider_id" text NOT NULL REFERENCES "ai_providers"("id") ON DELETE cascade,
  "model_id" text NOT NULL,
  "display_name" text NOT NULL,
  "enabled" boolean DEFAULT true NOT NULL,
  "created_at" timestamp with time zone DEFAULT now() NOT NULL,
  "updated_at" timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE IF NOT EXISTS "agent_tasks" (
  "id" text PRIMARY KEY NOT NULL,
  "project_id" text NOT NULL REFERENCES "projects"("id") ON DELETE cascade,
  "model_id" text NOT NULL REFERENCES "ai_models"("id") ON DELETE restrict,
  "title" text NOT NULL,
  "prompt" text NOT NULL,
  "status" text DEFAULT 'backlog' NOT NULL,
  "source_branch" text NOT NULL,
  "task_branch" text NOT NULL,
  "source_sha" text,
  "worktree_path" text,
  "resume_existing_branch" boolean DEFAULT false NOT NULL,
  "auto_commit" boolean DEFAULT false NOT NULL,
  "auto_push" boolean DEFAULT false NOT NULL,
  "auto_cleanup" boolean DEFAULT false NOT NULL,
  "last_error" text,
  "created_at" timestamp with time zone DEFAULT now() NOT NULL,
  "updated_at" timestamp with time zone DEFAULT now() NOT NULL,
  "started_at" timestamp with time zone,
  "finished_at" timestamp with time zone,
  "pushed_at" timestamp with time zone
);

CREATE TABLE IF NOT EXISTS "agent_runs" (
  "id" text PRIMARY KEY NOT NULL,
  "task_id" text NOT NULL REFERENCES "agent_tasks"("id") ON DELETE cascade,
  "status" text NOT NULL,
  "provider_protocol" text NOT NULL,
  "model_name" text NOT NULL,
  "assistant_text" text DEFAULT '' NOT NULL,
  "error" text,
  "started_at" timestamp with time zone DEFAULT now() NOT NULL,
  "finished_at" timestamp with time zone
);

CREATE TABLE IF NOT EXISTS "agent_messages" (
  "id" text PRIMARY KEY NOT NULL,
  "task_id" text NOT NULL REFERENCES "agent_tasks"("id") ON DELETE cascade,
  "run_id" text REFERENCES "agent_runs"("id") ON DELETE set null,
  "role" text NOT NULL,
  "content" text NOT NULL,
  "created_at" timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE IF NOT EXISTS "agent_events" (
  "id" text PRIMARY KEY NOT NULL,
  "run_id" text NOT NULL REFERENCES "agent_runs"("id") ON DELETE cascade,
  "sequence" integer NOT NULL,
  "kind" text NOT NULL,
  "payload" jsonb DEFAULT '{}'::jsonb NOT NULL,
  "created_at" timestamp with time zone DEFAULT now() NOT NULL
);

CREATE INDEX IF NOT EXISTS "ai_providers_created_at_idx" ON "ai_providers" ("created_at");
CREATE UNIQUE INDEX IF NOT EXISTS "ai_models_provider_model_idx" ON "ai_models" ("provider_id", "model_id");
CREATE INDEX IF NOT EXISTS "ai_models_provider_idx" ON "ai_models" ("provider_id");
CREATE INDEX IF NOT EXISTS "agent_tasks_status_created_idx" ON "agent_tasks" ("status", "created_at");
CREATE INDEX IF NOT EXISTS "agent_tasks_project_idx" ON "agent_tasks" ("project_id");
CREATE UNIQUE INDEX IF NOT EXISTS "agent_tasks_project_branch_idx" ON "agent_tasks" ("project_id", "task_branch");
CREATE INDEX IF NOT EXISTS "agent_runs_task_started_idx" ON "agent_runs" ("task_id", "started_at");
CREATE INDEX IF NOT EXISTS "agent_runs_status_idx" ON "agent_runs" ("status");
CREATE INDEX IF NOT EXISTS "agent_messages_task_created_idx" ON "agent_messages" ("task_id", "created_at");
CREATE UNIQUE INDEX IF NOT EXISTS "agent_events_run_sequence_idx" ON "agent_events" ("run_id", "sequence");
