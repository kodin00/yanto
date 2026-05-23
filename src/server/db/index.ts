import { drizzle } from "drizzle-orm/node-postgres";
import pg from "pg";
import { config } from "../config.js";
import * as schema from "./schema.js";

export const pool = new pg.Pool({
  connectionString: config.databaseUrl
});

export const db = drizzle(pool, { schema });

export async function migrate() {
  await pool.query(`
    CREATE TABLE IF NOT EXISTS projects (
      id text PRIMARY KEY,
      name text NOT NULL,
      git_url text,
      branch text NOT NULL,
      folder_name text NOT NULL,
      local_path text NOT NULL,
      compose_file text NOT NULL,
      compose_content text,
      env_file text NOT NULL DEFAULT '.env',
      auto_start boolean NOT NULL DEFAULT false,
      deploy_token text NOT NULL,
      ssh_private_key_path text,
      ssh_public_key text,
      created_at timestamptz NOT NULL DEFAULT now(),
      updated_at timestamptz NOT NULL DEFAULT now()
    );
  `);

  await pool.query(`ALTER TABLE projects ALTER COLUMN git_url DROP NOT NULL;`);
  await pool.query(`ALTER TABLE projects ADD COLUMN IF NOT EXISTS compose_content text;`);
  await pool.query(`ALTER TABLE projects ADD COLUMN IF NOT EXISTS env_file text NOT NULL DEFAULT '.env';`);
  await pool.query(`ALTER TABLE projects ADD COLUMN IF NOT EXISTS auto_start boolean NOT NULL DEFAULT false;`);

  await pool.query(`
    CREATE TABLE IF NOT EXISTS deployments (
      id text PRIMARY KEY,
      project_id text NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
      status text NOT NULL,
      trigger text NOT NULL,
      target_ref text,
      commit_sha text,
      commit_message text,
      rollback_from_deployment_id text,
      logs text NOT NULL DEFAULT '',
      exit_code integer,
      started_at timestamptz NOT NULL DEFAULT now(),
      finished_at timestamptz
    );
  `);

  await pool.query(`ALTER TABLE deployments ADD COLUMN IF NOT EXISTS target_ref text;`);
  await pool.query(`ALTER TABLE deployments ADD COLUMN IF NOT EXISTS commit_sha text;`);
  await pool.query(`ALTER TABLE deployments ADD COLUMN IF NOT EXISTS commit_message text;`);
  await pool.query(`ALTER TABLE deployments ADD COLUMN IF NOT EXISTS rollback_from_deployment_id text;`);

  await pool.query(`
    CREATE TABLE IF NOT EXISTS backups (
      id text PRIMARY KEY,
      project_id text REFERENCES projects(id) ON DELETE SET NULL,
      kind text NOT NULL,
      status text NOT NULL,
      filename text NOT NULL DEFAULT '',
      file_path text NOT NULL,
      file_size_bytes bigint,
      error text,
      note text,
      created_at timestamptz NOT NULL DEFAULT now(),
      finished_at timestamptz,
      downloaded_at timestamptz,
      download_count integer NOT NULL DEFAULT 0
    );
  `);
  await pool.query(`ALTER TABLE backups ALTER COLUMN project_id DROP NOT NULL;`);
  await pool.query(`ALTER TABLE backups ADD COLUMN IF NOT EXISTS filename text NOT NULL DEFAULT '';`);
  await pool.query(`ALTER TABLE backups ADD COLUMN IF NOT EXISTS error text;`);
  await pool.query(`ALTER TABLE backups ADD COLUMN IF NOT EXISTS finished_at timestamptz;`);
  await pool.query(`ALTER TABLE backups ADD COLUMN IF NOT EXISTS downloaded_at timestamptz;`);
  await pool.query(`ALTER TABLE backups ADD COLUMN IF NOT EXISTS download_count integer NOT NULL DEFAULT 0;`);

  await pool.query(`
    CREATE TABLE IF NOT EXISTS audit_logs (
      id text PRIMARY KEY,
      actor text NOT NULL,
      action text NOT NULL,
      entity_type text NOT NULL,
      entity_id text,
      project_id text REFERENCES projects(id) ON DELETE SET NULL,
      metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
      created_at timestamptz NOT NULL DEFAULT now()
    );
  `);

  await pool.query(`CREATE INDEX IF NOT EXISTS projects_created_at_idx ON projects(created_at);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS projects_folder_name_idx ON projects(folder_name);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS deployments_project_started_at_idx ON deployments(project_id, started_at DESC);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS deployments_status_idx ON deployments(status);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS deployments_started_at_idx ON deployments(started_at DESC);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS backups_created_at_idx ON backups(created_at DESC);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS backups_status_idx ON backups(status);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS audit_logs_created_at_idx ON audit_logs(created_at DESC);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS audit_logs_project_created_at_idx ON audit_logs(project_id, created_at DESC);`);
}
