import { drizzle } from "drizzle-orm/node-postgres";
import pg from "pg";
import { config } from "../config.js";
import { encrypt, isEncrypted } from "../services/crypto.js";
import { hashPassword } from "../services/passwords.js";
import { createId } from "../services/tokens.js";
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
  await pool.query(`ALTER TABLE projects ADD COLUMN IF NOT EXISTS agent_image text NOT NULL DEFAULT '';`);
  await pool.query(`UPDATE projects SET target_node_id = 'node_master_local' WHERE target_node_id IS NULL OR target_node_id = '';`);

  await pool.query(`
    CREATE TABLE IF NOT EXISTS ai_providers (
      id text PRIMARY KEY,
      name text NOT NULL,
      protocol text NOT NULL,
      base_url text NOT NULL,
      api_key text NOT NULL,
      enabled boolean NOT NULL DEFAULT true,
      created_at timestamptz NOT NULL DEFAULT now(),
      updated_at timestamptz NOT NULL DEFAULT now(),
      CONSTRAINT ai_providers_protocol_check CHECK (protocol IN ('openai_responses', 'openai_chat', 'anthropic_messages', 'codex_account'))
    );
  `);
  await pool.query(`ALTER TABLE ai_providers DROP CONSTRAINT IF EXISTS ai_providers_protocol_check;`);
  await pool.query(`ALTER TABLE ai_providers ADD CONSTRAINT ai_providers_protocol_check CHECK (protocol IN ('openai_responses', 'openai_chat', 'anthropic_messages', 'codex_account'));`);
  await pool.query(`CREATE INDEX IF NOT EXISTS ai_providers_created_at_idx ON ai_providers(created_at DESC);`);

  await pool.query(`
    CREATE TABLE IF NOT EXISTS ai_models (
      id text PRIMARY KEY,
      provider_id text NOT NULL REFERENCES ai_providers(id) ON DELETE CASCADE,
      model_id text NOT NULL,
      display_name text NOT NULL,
      enabled boolean NOT NULL DEFAULT true,
      created_at timestamptz NOT NULL DEFAULT now(),
      updated_at timestamptz NOT NULL DEFAULT now()
    );
  `);
  await pool.query(`CREATE UNIQUE INDEX IF NOT EXISTS ai_models_provider_model_idx ON ai_models(provider_id, model_id);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS ai_models_provider_idx ON ai_models(provider_id);`);

  await pool.query(`ALTER TABLE ai_providers ADD COLUMN IF NOT EXISTS default_model_id text REFERENCES ai_models(id) ON DELETE SET NULL;`);

  await pool.query(`
    CREATE TABLE IF NOT EXISTS agent_tasks (
      id text PRIMARY KEY,
      project_id text NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
      model_id text NOT NULL REFERENCES ai_models(id) ON DELETE RESTRICT,
      title text NOT NULL,
      prompt text NOT NULL,
      status text NOT NULL DEFAULT 'backlog',
      source_branch text NOT NULL,
      task_branch text NOT NULL,
      source_sha text,
      worktree_path text,
      codex_thread_id text,
      resume_existing_branch boolean NOT NULL DEFAULT false,
      auto_commit boolean NOT NULL DEFAULT false,
      auto_push boolean NOT NULL DEFAULT false,
      auto_cleanup boolean NOT NULL DEFAULT false,
      last_error text,
      created_at timestamptz NOT NULL DEFAULT now(),
      updated_at timestamptz NOT NULL DEFAULT now(),
      started_at timestamptz,
      finished_at timestamptz,
      pushed_at timestamptz,
      archived_at timestamptz,
      CONSTRAINT agent_tasks_status_check CHECK (status IN ('backlog', 'running', 'review', 'done'))
    );
  `);
  await pool.query(`ALTER TABLE agent_tasks ADD COLUMN IF NOT EXISTS codex_thread_id text;`);
  await pool.query(`ALTER TABLE agent_tasks ADD COLUMN IF NOT EXISTS archived_at timestamptz;`);
  await pool.query(`CREATE INDEX IF NOT EXISTS agent_tasks_status_created_idx ON agent_tasks(status, created_at DESC);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS agent_tasks_archived_updated_idx ON agent_tasks(archived_at, updated_at DESC);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS agent_tasks_project_idx ON agent_tasks(project_id);`);
  await pool.query(`CREATE UNIQUE INDEX IF NOT EXISTS agent_tasks_project_branch_idx ON agent_tasks(project_id, task_branch);`);

  await pool.query(`
    CREATE TABLE IF NOT EXISTS agent_runs (
      id text PRIMARY KEY,
      task_id text NOT NULL REFERENCES agent_tasks(id) ON DELETE CASCADE,
      status text NOT NULL,
      provider_protocol text NOT NULL,
      model_name text NOT NULL,
      assistant_text text NOT NULL DEFAULT '',
      error text,
      started_at timestamptz NOT NULL DEFAULT now(),
      finished_at timestamptz,
      CONSTRAINT agent_runs_status_check CHECK (status IN ('running', 'succeeded', 'failed', 'canceled'))
    );
  `);
  await pool.query(`CREATE INDEX IF NOT EXISTS agent_runs_task_started_idx ON agent_runs(task_id, started_at DESC);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS agent_runs_status_idx ON agent_runs(status);`);
  await pool.query(`
    CREATE UNIQUE INDEX IF NOT EXISTS agent_runs_one_running_per_task_idx
    ON agent_runs (task_id)
    WHERE status = 'running';
  `);

  await pool.query(`
    CREATE TABLE IF NOT EXISTS agent_messages (
      id text PRIMARY KEY,
      task_id text NOT NULL REFERENCES agent_tasks(id) ON DELETE CASCADE,
      run_id text REFERENCES agent_runs(id) ON DELETE SET NULL,
      role text NOT NULL,
      content text NOT NULL,
      created_at timestamptz NOT NULL DEFAULT now(),
      CONSTRAINT agent_messages_role_check CHECK (role IN ('user', 'assistant'))
    );
  `);
  await pool.query(`CREATE INDEX IF NOT EXISTS agent_messages_task_created_idx ON agent_messages(task_id, created_at);`);

  await pool.query(`
    CREATE TABLE IF NOT EXISTS agent_events (
      id text PRIMARY KEY,
      run_id text NOT NULL REFERENCES agent_runs(id) ON DELETE CASCADE,
      sequence integer NOT NULL,
      kind text NOT NULL,
      payload jsonb NOT NULL DEFAULT '{}'::jsonb,
      created_at timestamptz NOT NULL DEFAULT now()
    );
  `);
  await pool.query(`CREATE UNIQUE INDEX IF NOT EXISTS agent_events_run_sequence_idx ON agent_events(run_id, sequence);`);

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
  await pool.query(`ALTER TABLE backups ADD COLUMN IF NOT EXISTS source_node_id text REFERENCES deployment_nodes(id) ON DELETE SET NULL DEFAULT 'node_master_local';`);
  await pool.query(`ALTER TABLE backups ADD COLUMN IF NOT EXISTS checksum text;`);

  await pool.query(`
    CREATE TABLE IF NOT EXISTS backup_policies (
      id text PRIMARY KEY,
      name text NOT NULL,
      source_node_id text NOT NULL REFERENCES deployment_nodes(id) ON DELETE CASCADE,
      target_container_id text,
      enabled boolean NOT NULL DEFAULT true,
      hourly_at_minute integer NOT NULL DEFAULT 0 CHECK (hourly_at_minute BETWEEN 0 AND 59),
      hourly_retention integer NOT NULL DEFAULT 24 CHECK (hourly_retention >= 1),
      daily_retention integer NOT NULL DEFAULT 30 CHECK (daily_retention >= 1),
      last_run_at timestamptz,
      next_run_at timestamptz,
      created_at timestamptz NOT NULL DEFAULT now(),
      updated_at timestamptz NOT NULL DEFAULT now()
    );
  `);
  await pool.query(`CREATE INDEX IF NOT EXISTS backup_policies_source_node_idx ON backup_policies(source_node_id);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS backup_policies_next_run_idx ON backup_policies(enabled, next_run_at);`);
  await pool.query(`ALTER TABLE backups ADD COLUMN IF NOT EXISTS policy_id text REFERENCES backup_policies(id) ON DELETE SET NULL;`);
  await pool.query(`
    CREATE TABLE IF NOT EXISTS backup_policy_destinations (
      policy_id text NOT NULL REFERENCES backup_policies(id) ON DELETE CASCADE,
      destination_node_id text NOT NULL REFERENCES deployment_nodes(id) ON DELETE CASCADE,
      created_at timestamptz NOT NULL DEFAULT now(),
      PRIMARY KEY (policy_id, destination_node_id)
    );
  `);
  await pool.query(`CREATE INDEX IF NOT EXISTS backup_policy_destinations_node_idx ON backup_policy_destinations(destination_node_id);`);
  await pool.query(`
    CREATE TABLE IF NOT EXISTS backup_replicas (
      id text PRIMARY KEY,
      backup_id text NOT NULL REFERENCES backups(id) ON DELETE CASCADE,
      destination_node_id text NOT NULL REFERENCES deployment_nodes(id) ON DELETE CASCADE,
      status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'copying', 'success', 'failed')),
      file_path text,
      checksum text,
      error text,
      attempts integer NOT NULL DEFAULT 0,
      created_at timestamptz NOT NULL DEFAULT now(),
      updated_at timestamptz NOT NULL DEFAULT now(),
      finished_at timestamptz,
      UNIQUE (backup_id, destination_node_id)
    );
  `);
  await pool.query(`CREATE INDEX IF NOT EXISTS backup_replicas_status_idx ON backup_replicas(status);`);

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

  await pool.query(`
    CREATE TABLE IF NOT EXISTS mcp_access_tokens (
      id text PRIMARY KEY,
      name text NOT NULL,
      token_hash text NOT NULL,
      access_level text NOT NULL,
      last_used_at timestamptz,
      revoked_at timestamptz,
      created_at timestamptz NOT NULL DEFAULT now(),
      updated_at timestamptz NOT NULL DEFAULT now()
    );
  `);
  await pool.query(`ALTER TABLE mcp_access_tokens ADD COLUMN IF NOT EXISTS last_used_at timestamptz;`);
  await pool.query(`ALTER TABLE mcp_access_tokens ADD COLUMN IF NOT EXISTS revoked_at timestamptz;`);
  await pool.query(`CREATE UNIQUE INDEX IF NOT EXISTS mcp_access_tokens_token_hash_idx ON mcp_access_tokens(token_hash);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS mcp_access_tokens_revoked_idx ON mcp_access_tokens(revoked_at);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS mcp_access_tokens_created_at_idx ON mcp_access_tokens(created_at DESC);`);

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
      zone_id text NOT NULL DEFAULT '',
      api_token text NOT NULL,
      created_at timestamptz NOT NULL DEFAULT now(),
      updated_at timestamptz NOT NULL DEFAULT now()
    );
  `);
  await pool.query(`ALTER TABLE cloudflare_clients ADD COLUMN IF NOT EXISTS zone_id text NOT NULL DEFAULT '';`);
  await pool.query(`CREATE UNIQUE INDEX IF NOT EXISTS cloudflare_clients_account_id_idx ON cloudflare_clients(account_id);`);

  const legacySetting = await pool.query<{ value: string }>(`SELECT value FROM app_settings WHERE key = 'cloudflare.tunnel' LIMIT 1`);
  if (legacySetting.rows[0]) {
    try {
      const legacy = JSON.parse(legacySetting.rows[0].value) as { accountId?: string; zoneId?: string; apiToken?: string };
      if (legacy.accountId && legacy.apiToken) {
        const token = isEncrypted(legacy.apiToken) ? legacy.apiToken : encrypt(legacy.apiToken);
        await pool.query(
          `INSERT INTO cloudflare_clients (id, name, account_id, zone_id, api_token) VALUES ('cfc_legacy', 'Default Cloudflare', $1, $2, $3) ON CONFLICT (account_id) DO UPDATE SET zone_id = COALESCE(NULLIF(cloudflare_clients.zone_id, ''), EXCLUDED.zone_id)`,
          [legacy.accountId, legacy.zoneId ?? "", token]
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

  await pool.query(`
    CREATE TABLE IF NOT EXISTS frp_tunnels (
      id text PRIMARY KEY,
      node_id text REFERENCES deployment_nodes(id) ON DELETE CASCADE,
      name text NOT NULL,
      protocol text NOT NULL,
      local_host text NOT NULL,
      local_port integer NOT NULL,
      remote_port integer NOT NULL,
      enabled boolean NOT NULL DEFAULT true,
      sync_status text NOT NULL DEFAULT 'syncing',
      last_error text,
      last_synced_at timestamptz,
      created_at timestamptz NOT NULL DEFAULT now(),
      updated_at timestamptz NOT NULL DEFAULT now(),
      CONSTRAINT frp_tunnels_protocol_check CHECK (protocol IN ('tcp', 'udp')),
      CONSTRAINT frp_tunnels_local_port_check CHECK (local_port BETWEEN 1 AND 65535),
      CONSTRAINT frp_tunnels_remote_port_check CHECK (remote_port BETWEEN 1 AND 65535)
    );
  `);
  await pool.query(`ALTER TABLE frp_tunnels ALTER COLUMN node_id DROP NOT NULL;`);
  await pool.query(`CREATE INDEX IF NOT EXISTS frp_tunnels_node_id_idx ON frp_tunnels(node_id);`);
  await pool.query(`
    CREATE TABLE IF NOT EXISTS frp_servers (
      id text PRIMARY KEY,
      node_id text NOT NULL REFERENCES deployment_nodes(id) ON DELETE CASCADE,
      name text NOT NULL,
      public_host text NOT NULL,
      bind_port integer NOT NULL DEFAULT 7000 CHECK (bind_port BETWEEN 1 AND 65535),
      port_start integer NOT NULL DEFAULT 25560 CHECK (port_start BETWEEN 1 AND 65535),
      port_end integer NOT NULL DEFAULT 25600 CHECK (port_end BETWEEN 1 AND 65535),
      auth_token text NOT NULL,
      status text NOT NULL DEFAULT 'offline',
      last_error text,
      created_at timestamptz NOT NULL DEFAULT now(),
      updated_at timestamptz NOT NULL DEFAULT now(),
      CHECK (port_start <= port_end),
      UNIQUE (node_id)
    );
  `);
  await pool.query(`
    CREATE TABLE IF NOT EXISTS frp_node_assignments (
      node_id text PRIMARY KEY REFERENCES deployment_nodes(id) ON DELETE CASCADE,
      role text NOT NULL DEFAULT 'disabled' CHECK (role IN ('disabled', 'client', 'server', 'both')),
      server_id text REFERENCES frp_servers(id) ON DELETE SET NULL,
      desired_revision integer NOT NULL DEFAULT 1,
      applied_revision integer NOT NULL DEFAULT 0,
      status text NOT NULL DEFAULT 'pending',
      last_error text,
      updated_at timestamptz NOT NULL DEFAULT now()
    );
  `);
  await pool.query(`CREATE INDEX IF NOT EXISTS frp_node_assignments_server_idx ON frp_node_assignments(server_id);`);
  await pool.query(`ALTER TABLE frp_tunnels ADD COLUMN IF NOT EXISTS client_node_id text REFERENCES deployment_nodes(id) ON DELETE CASCADE;`);
  await pool.query(`ALTER TABLE frp_tunnels ADD COLUMN IF NOT EXISTS server_id text REFERENCES frp_servers(id) ON DELETE CASCADE;`);
  await pool.query(`UPDATE frp_tunnels SET client_node_id = node_id WHERE client_node_id IS NULL AND node_id IS NOT NULL;`);
  await pool.query(`DROP INDEX IF EXISTS frp_tunnels_protocol_remote_port_idx;`);
  await pool.query(`CREATE INDEX IF NOT EXISTS frp_tunnels_client_node_idx ON frp_tunnels(client_node_id);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS frp_tunnels_server_idx ON frp_tunnels(server_id);`);
  await pool.query(`CREATE UNIQUE INDEX IF NOT EXISTS frp_tunnels_legacy_protocol_remote_port_idx ON frp_tunnels(protocol, remote_port) WHERE server_id IS NULL;`);
  await pool.query(`CREATE UNIQUE INDEX IF NOT EXISTS frp_tunnels_server_protocol_remote_port_idx ON frp_tunnels(server_id, protocol, remote_port) WHERE server_id IS NOT NULL;`);
  await pool.query(`DROP TABLE IF EXISTS frp_worker_states;`);

  await pool.query(`
    CREATE TABLE IF NOT EXISTS users (
      id text PRIMARY KEY,
      username text NOT NULL,
      role text NOT NULL,
      status text NOT NULL,
      password_hash text,
      session_version integer NOT NULL DEFAULT 1,
      last_login_at timestamptz,
      created_at timestamptz NOT NULL DEFAULT now(),
      updated_at timestamptz NOT NULL DEFAULT now(),
      CONSTRAINT users_role_check CHECK (role IN ('owner', 'member')),
      CONSTRAINT users_status_check CHECK (status IN ('invited', 'active', 'disabled')),
      CONSTRAINT users_username_nonempty_check CHECK (length(username) > 0)
    );
  `);
  await pool.query(`CREATE UNIQUE INDEX IF NOT EXISTS users_username_lower_idx ON users(lower(username));`);
  await pool.query(`CREATE UNIQUE INDEX IF NOT EXISTS users_single_owner_idx ON users(role) WHERE role = 'owner';`);
  await pool.query(`CREATE INDEX IF NOT EXISTS users_status_idx ON users(status);`);

  await pool.query(`
    CREATE TABLE IF NOT EXISTS user_project_access (
      user_id text NOT NULL REFERENCES users(id) ON DELETE CASCADE,
      project_id text NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
      permissions jsonb NOT NULL DEFAULT '[]'::jsonb,
      created_at timestamptz NOT NULL DEFAULT now(),
      updated_at timestamptz NOT NULL DEFAULT now(),
      PRIMARY KEY (user_id, project_id),
      CONSTRAINT user_project_access_permissions_array_check CHECK (jsonb_typeof(permissions) = 'array')
    );
  `);
  await pool.query(`CREATE INDEX IF NOT EXISTS user_project_access_project_idx ON user_project_access(project_id);`);

  await pool.query(`
    CREATE TABLE IF NOT EXISTS account_tokens (
      id text PRIMARY KEY,
      user_id text NOT NULL REFERENCES users(id) ON DELETE CASCADE,
      purpose text NOT NULL,
      token_hash text NOT NULL,
      expires_at timestamptz NOT NULL,
      used_at timestamptz,
      created_at timestamptz NOT NULL DEFAULT now(),
      CONSTRAINT account_tokens_purpose_check CHECK (purpose IN ('invite', 'reset'))
    );
  `);
  await pool.query(`CREATE UNIQUE INDEX IF NOT EXISTS account_tokens_hash_idx ON account_tokens(token_hash);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS account_tokens_user_active_idx ON account_tokens(user_id, used_at);`);
  await pool.query(`CREATE INDEX IF NOT EXISTS account_tokens_expires_idx ON account_tokens(expires_at);`);

  if (config.legacyAdminUsername && config.legacyAdminPassword) {
    const existing = await pool.query<{ count: string }>(`SELECT count(*)::text AS count FROM users`);
    if (existing.rows[0]?.count === "0") {
      const passwordHash = config.legacyAdminPassword.startsWith("$2")
        ? config.legacyAdminPassword
        : await hashPassword(config.legacyAdminPassword);
      await pool.query(
        `INSERT INTO users (id, username, role, status, password_hash) VALUES ($1, $2, 'owner', 'active', $3) ON CONFLICT DO NOTHING`,
        [createId("usr"), config.legacyAdminUsername, passwordHash]
      );
    }
  }
}
