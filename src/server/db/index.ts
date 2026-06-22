import { drizzle } from "drizzle-orm/node-postgres";
import pg from "pg";
import { config } from "../config.js";
import { encrypt, isEncrypted } from "../services/crypto.js";
import * as schema from "./schema.js";

export const pool = new pg.Pool({
  connectionString: config.databaseUrl,
  max: 20,
  idleTimeoutMillis: 30_000,
  connectionTimeoutMillis: 5_000
});

export const db = drizzle(pool, { schema });

export async function migrate() {
  await pool.query(`
    CREATE TABLE IF NOT EXISTS deployment_nodes (
      id text PRIMARY KEY,
      name text NOT NULL,
      role text NOT NULL,
      status text NOT NULL DEFAULT 'offline',
      last_seen_at timestamptz,
      docker_version text,
      labels jsonb NOT NULL DEFAULT '{}'::jsonb,
      token_hash text,
      created_at timestamptz NOT NULL DEFAULT now(),
      updated_at timestamptz NOT NULL DEFAULT now()
    );
  `);

  await pool.query(`
    INSERT INTO deployment_nodes (id, name, role, status, last_seen_at, labels, created_at, updated_at)
    VALUES ('node_master_local', 'Master', 'master', 'online', now(), '{}'::jsonb, now(), now())
    ON CONFLICT (id) DO UPDATE SET
      name = EXCLUDED.name,
      role = EXCLUDED.role,
      status = EXCLUDED.status,
      last_seen_at = EXCLUDED.last_seen_at,
      updated_at = EXCLUDED.updated_at;
  `);

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
      manual_deploy_enabled boolean NOT NULL DEFAULT true,
      github_webhook_enabled boolean NOT NULL DEFAULT true,
      target_node_id text NOT NULL DEFAULT 'node_master_local',
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
  await pool.query(`ALTER TABLE projects ADD COLUMN IF NOT EXISTS manual_deploy_enabled boolean NOT NULL DEFAULT true;`);
  await pool.query(`ALTER TABLE projects ADD COLUMN IF NOT EXISTS github_webhook_enabled boolean NOT NULL DEFAULT true;`);
  await pool.query(`ALTER TABLE projects ADD COLUMN IF NOT EXISTS target_node_id text NOT NULL DEFAULT 'node_master_local';`);
  await pool.query(`UPDATE projects SET target_node_id = 'node_master_local' WHERE target_node_id IS NULL OR target_node_id = '';`);

  await pool.query(`
    CREATE TABLE IF NOT EXISTS deployments (
      id text PRIMARY KEY,
      project_id text NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
      node_id text NOT NULL DEFAULT 'node_master_local',
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
  await pool.query(`ALTER TABLE deployments ADD COLUMN IF NOT EXISTS node_id text NOT NULL DEFAULT 'node_master_local';`);
  await pool.query(`UPDATE deployments SET node_id = 'node_master_local' WHERE node_id IS NULL OR node_id = '';`);
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

  await pool.query(`
    CREATE TABLE IF NOT EXISTS app_settings (
      key text PRIMARY KEY,
      value text NOT NULL,
      updated_at timestamptz NOT NULL DEFAULT now()
    );
  `);

  await pool.query(`CREATE INDEX IF NOT EXISTS projects_created_at_idx ON projects(created_at);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS projects_folder_name_idx ON projects(folder_name);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS projects_target_node_idx ON projects(target_node_id);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS deployment_nodes_role_idx ON deployment_nodes(role);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS deployment_nodes_status_idx ON deployment_nodes(status);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS deployment_nodes_token_hash_idx ON deployment_nodes(token_hash);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS deployments_project_started_at_idx ON deployments(project_id, started_at DESC);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS deployments_node_status_idx ON deployments(node_id, status, started_at);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS deployments_status_idx ON deployments(status);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS deployments_started_at_idx ON deployments(started_at DESC);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS backups_created_at_idx ON backups(created_at DESC);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS backups_status_idx ON backups(status);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS audit_logs_created_at_idx ON audit_logs(created_at DESC);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS audit_logs_project_created_at_idx ON audit_logs(project_id, created_at DESC);`);

  await pool.query(`
    CREATE TABLE IF NOT EXISTS cloudflare_clients (
      id text PRIMARY KEY,
      name text NOT NULL,
      account_id text NOT NULL,
      api_token text NOT NULL,
      created_at timestamptz NOT NULL DEFAULT now(),
      updated_at timestamptz NOT NULL DEFAULT now()
    );
  `);
  await pool.query(`CREATE UNIQUE INDEX IF NOT EXISTS cloudflare_clients_account_id_idx ON cloudflare_clients(account_id);`);

  const legacySetting = await pool.query<{ value: string }>(`SELECT value FROM app_settings WHERE key = 'cloudflare.tunnel' LIMIT 1`);
  if (legacySetting.rows[0]) {
    try {
      const legacy = JSON.parse(legacySetting.rows[0].value) as { accountId?: string; apiToken?: string };
      if (legacy.accountId && legacy.apiToken) {
        const token = isEncrypted(legacy.apiToken) ? legacy.apiToken : encrypt(legacy.apiToken);
        await pool.query(
          `INSERT INTO cloudflare_clients (id, name, account_id, api_token) VALUES ('cfc_legacy', 'Default Cloudflare', $1, $2) ON CONFLICT (account_id) DO NOTHING`,
          [legacy.accountId, token]
        );
      }
    } catch {
      // Ignore malformed legacy settings; they remain available for manual recovery.
    }
  }

  await pool.query(`
    CREATE TABLE IF NOT EXISTS cloudflare_tunnels (
      id text PRIMARY KEY,
      client_id text NOT NULL REFERENCES cloudflare_clients(id) ON DELETE RESTRICT,
      node_id text NOT NULL REFERENCES deployment_nodes(id),
      cf_account_id text NOT NULL,
      cf_tunnel_id text NOT NULL,
      tunnel_name text NOT NULL,
      tunnel_token text NOT NULL,
      docker_network_name text NOT NULL,
      status text NOT NULL DEFAULT 'active',
      last_health_check_at timestamptz,
      created_at timestamptz NOT NULL DEFAULT now(),
      updated_at timestamptz NOT NULL DEFAULT now()
    );
  `);
  await pool.query(`ALTER TABLE cloudflare_tunnels ADD COLUMN IF NOT EXISTS client_id text REFERENCES cloudflare_clients(id) ON DELETE RESTRICT;`);
  await pool.query(`ALTER TABLE cloudflare_tunnels ADD COLUMN IF NOT EXISTS docker_network_name text;`);
  await pool.query(`UPDATE cloudflare_tunnels SET client_id = COALESCE(client_id, (SELECT id FROM cloudflare_clients WHERE account_id = cloudflare_tunnels.cf_account_id LIMIT 1));`);
  await pool.query(`UPDATE cloudflare_tunnels SET docker_network_name = COALESCE(docker_network_name, 'yanto-cf-' || regexp_replace(lower(id), '[^a-z0-9_.-]', '-', 'g'));`);
  await pool.query(`DROP INDEX IF EXISTS cloudflare_tunnels_node_id_idx;`);
  await pool.query(`CREATE INDEX IF NOT EXISTS cloudflare_tunnels_node_id_idx ON cloudflare_tunnels(node_id);`);
  await pool.query(`DROP INDEX IF EXISTS cloudflare_tunnels_cf_tunnel_id_idx;`);
  await pool.query(`CREATE UNIQUE INDEX IF NOT EXISTS cloudflare_tunnels_cf_tunnel_id_idx ON cloudflare_tunnels(cf_tunnel_id);`);
  await pool.query(`CREATE UNIQUE INDEX IF NOT EXISTS cloudflare_tunnels_network_idx ON cloudflare_tunnels(docker_network_name);`);

  await pool.query(`
    CREATE TABLE IF NOT EXISTS cloudflare_tunnel_assignments (
      id text PRIMARY KEY,
      tunnel_id text NOT NULL REFERENCES cloudflare_tunnels(id) ON DELETE CASCADE,
      target_type text NOT NULL,
      project_id text REFERENCES projects(id) ON DELETE CASCADE,
      compose_project text,
      compose_service text,
      container_name text,
      created_at timestamptz NOT NULL DEFAULT now()
    );
  `);
  await pool.query(`CREATE INDEX IF NOT EXISTS cloudflare_assignments_tunnel_idx ON cloudflare_tunnel_assignments(tunnel_id);`);
  await pool.query(`CREATE UNIQUE INDEX IF NOT EXISTS cloudflare_assignments_target_idx ON cloudflare_tunnel_assignments(tunnel_id, target_type, compose_project, compose_service, container_name);`);

  await pool.query(`
    CREATE TABLE IF NOT EXISTS cloudflare_routes (
      id text PRIMARY KEY,
      tunnel_id text NOT NULL REFERENCES cloudflare_tunnels(id) ON DELETE CASCADE,
      project_id text REFERENCES projects(id) ON DELETE CASCADE,
      assignment_id text REFERENCES cloudflare_tunnel_assignments(id) ON DELETE RESTRICT,
      zone_id text NOT NULL,
      hostname text NOT NULL,
      service_target text NOT NULL,
      protocol text NOT NULL DEFAULT 'http',
      port integer NOT NULL DEFAULT 80,
      no_tls_verify boolean NOT NULL DEFAULT false,
      enabled boolean NOT NULL DEFAULT true,
      sync_status text NOT NULL DEFAULT 'active',
      last_error text,
      cf_dns_record_id text,
      last_published_at timestamptz,
      created_at timestamptz NOT NULL DEFAULT now(),
      updated_at timestamptz NOT NULL DEFAULT now()
    );
  `);

  await pool.query(`ALTER TABLE cloudflare_routes ALTER COLUMN project_id DROP NOT NULL;`);
  await pool.query(`ALTER TABLE cloudflare_routes ADD COLUMN IF NOT EXISTS assignment_id text REFERENCES cloudflare_tunnel_assignments(id) ON DELETE RESTRICT;`);
  await pool.query(`ALTER TABLE cloudflare_routes ADD COLUMN IF NOT EXISTS zone_id text NOT NULL DEFAULT '';`);
  await pool.query(`ALTER TABLE cloudflare_routes ADD COLUMN IF NOT EXISTS protocol text NOT NULL DEFAULT 'http';`);
  await pool.query(`ALTER TABLE cloudflare_routes ADD COLUMN IF NOT EXISTS port integer NOT NULL DEFAULT 80;`);
  await pool.query(`UPDATE cloudflare_routes SET zone_id = COALESCE(NULLIF(zone_id, ''), (SELECT (value::jsonb ->> 'zoneId') FROM app_settings WHERE key = 'cloudflare.tunnel' LIMIT 1), 'legacy');`);
  await pool.query(`ALTER TABLE cloudflare_routes ADD COLUMN IF NOT EXISTS no_tls_verify boolean NOT NULL DEFAULT false;`);
  await pool.query(`ALTER TABLE cloudflare_routes ADD COLUMN IF NOT EXISTS sync_status text NOT NULL DEFAULT 'active';`);
  await pool.query(`ALTER TABLE cloudflare_routes ADD COLUMN IF NOT EXISTS last_error text;`);
  await pool.query(`
    DELETE FROM cloudflare_routes stale
    USING cloudflare_routes keep
    WHERE stale.project_id = keep.project_id
      AND stale.created_at < keep.created_at;
  `);
  await pool.query(`CREATE INDEX IF NOT EXISTS cloudflare_routes_tunnel_id_idx ON cloudflare_routes(tunnel_id);`);
  await pool.query(`DROP INDEX IF EXISTS cloudflare_routes_project_id_idx;`);
  await pool.query(`DROP INDEX IF EXISTS cloudflare_routes_project_id_unique_idx;`);
  await pool.query(`CREATE INDEX IF NOT EXISTS cloudflare_routes_project_id_idx ON cloudflare_routes(project_id);`);
  await pool.query(`DROP INDEX IF EXISTS cloudflare_routes_hostname_idx;`);
  await pool.query(`CREATE UNIQUE INDEX IF NOT EXISTS cloudflare_routes_hostname_idx ON cloudflare_routes(zone_id, hostname);`);
}
