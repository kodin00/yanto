CREATE UNIQUE INDEX IF NOT EXISTS "agent_runs_one_running_per_task_idx"
ON "agent_runs" ("task_id")
WHERE "status" = 'running';
