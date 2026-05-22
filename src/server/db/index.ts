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
      deploy_token text NOT NULL,
      ssh_private_key_path text,
      ssh_public_key text,
      created_at timestamptz NOT NULL DEFAULT now(),
      updated_at timestamptz NOT NULL DEFAULT now()
    );
  `);

  await pool.query(`ALTER TABLE projects ALTER COLUMN git_url DROP NOT NULL;`);
  await pool.query(`ALTER TABLE projects ADD COLUMN IF NOT EXISTS compose_content text;`);

  await pool.query(`
    CREATE TABLE IF NOT EXISTS deployments (
      id text PRIMARY KEY,
      project_id text NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
      status text NOT NULL,
      trigger text NOT NULL,
      logs text NOT NULL DEFAULT '',
      exit_code integer,
      started_at timestamptz NOT NULL DEFAULT now(),
      finished_at timestamptz
    );
  `);
}
