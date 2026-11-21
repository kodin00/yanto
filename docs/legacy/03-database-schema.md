# Database Schema

PostgreSQL. All primary keys are `text` IDs with prefixes (e.g. `usr_...`, `dep_...`, `bak_...`). Timestamps are `timestamptz`, default `now()`. Secrets (`ai_providers.api_key`, `cloudflare_clients.api_token`, `cloudflare_tunnels.tunnel_token`) are **encrypted at the application layer**, not by Postgres.

## `users`

Dashboard accounts. Enforces a single owner.

| Column | Type | Notes |
|---|---|---|
| `id` | text PK | |
| `username` | text NOT NULL | case-insensitive unique via `users_username_lower_idx` on `lower(username)`; non-empty check |
| `role` | text NOT NULL | check `IN ('owner','member')` |
| `status` | text NOT NULL | check `IN ('invited','active','disabled')` |
| `password_hash` | text nullable | |
| `session_version` | integer NOT NULL default 1 | revocation counter |
| `last_login_at` | timestamptz | |
| `created_at`, `updated_at` | timestamptz NOT NULL default now() | |

Indexes: `users_username_lower_idx` (unique), `users_single_owner_idx` (unique partial where role='owner'), `users_status_idx`.

## `deployment_nodes`

Master/worker nodes that can run deployments.

| Column | Type | Notes |
|---|---|---|
| `id` | text PK | |
| `name` | text NOT NULL | |
| `role` | text NOT NULL | `master`/`worker` |
| `status` | text NOT NULL default `offline` | |
| `last_seen_at` | timestamptz | |
| `docker_version` | text | |
| `labels` | jsonb NOT NULL default `{}` | string→string map |
| `token_hash` | text | hash of worker bearer token |
| `created_at`, `updated_at` | | |

Indexes: `deployment_nodes_role_idx`, `deployment_nodes_status_idx`, `deployment_nodes_token_hash_idx`.

Seed row `node_master_local` ('Master', role master, status online) created by migrations / `ensureLocalMasterNode`.

## `projects`

Deployable projects (git repo + docker compose, or a Docker image).

| Column | Type | Notes |
|---|---|---|
| `id` | text PK | |
| `name` | text NOT NULL | |
| `git_url` | text nullable | |
| `docker_image` | text NOT NULL default `''` | |
| `branch` | text NOT NULL | |
| `folder_name` | text NOT NULL | maps to compose project name; uniqueness matters for container↔project mapping |
| `local_path` | text NOT NULL | |
| `compose_file` | text NOT NULL | |
| `compose_content` | text nullable | inline compose override |
| `env_file` | text NOT NULL default `.env` | |
| `auto_start` | boolean NOT NULL default false | |
| `manual_deploy_enabled` | boolean NOT NULL default true | |
| `github_webhook_enabled` | boolean NOT NULL default true | |
| `target_node_id` | text NOT NULL default `node_master_local` | FK `deployment_nodes.id` |
| `deploy_token` | text NOT NULL | GitHub webhook secret / manual deploy bearer |
| `ssh_private_key_path` | text nullable | |
| `ssh_public_key` | text nullable | |
| `agent_image` | text NOT NULL default `''` | |
| `created_at`, `updated_at` | | |

Indexes: `projects_created_at_idx`, `projects_folder_name_idx`.

## `user_project_access`

Per-user project grants. Composite PK `(user_id, project_id)`.

| Column | Type | Notes |
|---|---|---|
| `user_id` | text NOT NULL | FK users.id ON DELETE cascade |
| `project_id` | text NOT NULL | FK projects.id ON DELETE cascade |
| `permissions` | jsonb NOT NULL default `[]` | array of `ProjectPermission`; check `jsonb_typeof = 'array'` |
| `created_at`, `updated_at` | | |

Indexes: PK, `user_project_access_project_idx`.

## `account_tokens`

Single-use hashed tokens for invite/reset.

| Column | Type | Notes |
|---|---|---|
| `id` | text PK | |
| `user_id` | text NOT NULL | FK users.id cascade |
| `purpose` | text NOT NULL | check `IN ('invite','reset')` |
| `token_hash` | text NOT NULL | unique |
| `expires_at` | timestamptz NOT NULL | |
| `used_at` | timestamptz | |
| `created_at` | | |

Indexes: `account_tokens_hash_idx` (unique), `account_tokens_user_active_idx` (user_id, used_at), `account_tokens_expires_idx`.

## `ai_providers`

AI provider configs.

| Column | Type | Notes |
|---|---|---|
| `id` | text PK | |
| `name` | text NOT NULL | |
| `protocol` | text NOT NULL | check `IN ('openai_responses','openai_chat','anthropic_messages','codex_account')` |
| `base_url` | text NOT NULL | |
| `api_key` | text NOT NULL | stored encrypted at app layer |
| `enabled` | boolean NOT NULL default true | |
| `default_model_id` | text | FK ai_models.id ON DELETE set null (self-referential) |
| `created_at`, `updated_at` | | |

Index: `ai_providers_created_at_idx`.

## `ai_models`

Models under a provider.

| Column | Type | Notes |
|---|---|---|
| `id` | text PK | |
| `provider_id` | text NOT NULL | FK ai_providers.id cascade |
| `model_id` | text NOT NULL | upstream model id |
| `display_name` | text NOT NULL | |
| `enabled` | boolean NOT NULL default true | |
| `created_at`, `updated_at` | | |

Indexes: `ai_models_provider_model_idx` unique (provider_id, model_id), `ai_models_provider_idx`.

## `agent_tasks`

AI coding agent tasks against a project.

| Column | Type | Notes |
|---|---|---|
| `id` | text PK | |
| `project_id` | text NOT NULL | FK projects.id cascade |
| `model_id` | text NOT NULL | FK ai_models.id ON DELETE restrict |
| `title`, `prompt` | text NOT NULL | |
| `status` | text NOT NULL default `backlog` | check `IN ('backlog','running','review','done')` |
| `source_branch`, `task_branch` | text NOT NULL | |
| `source_sha` | text | |
| `worktree_path` | text | |
| `codex_thread_id` | text | |
| `resume_existing_branch`, `auto_commit`, `auto_push`, `auto_cleanup` | boolean NOT NULL default false | |
| `last_error` | text | |
| `created_at`, `updated_at`, `started_at`, `finished_at`, `pushed_at`, `archived_at` | timestamptz | |

Indexes: `agent_tasks_status_created_idx` (status, created_at), `agent_tasks_archived_updated_idx` (archived_at, updated_at), `agent_tasks_project_idx`, `agent_tasks_project_branch_idx` unique (project_id, task_branch).

## `agent_runs`

Individual execution runs of a task.

| Column | Type | Notes |
|---|---|---|
| `id` | text PK | |
| `task_id` | text NOT NULL | FK agent_tasks.id cascade |
| `status` | text NOT NULL | check `IN ('running','succeeded','failed','canceled')` |
| `provider_protocol` | text NOT NULL | |
| `model_name` | text NOT NULL | |
| `assistant_text` | text NOT NULL default `''` | |
| `error` | text | |
| `started_at` | timestamptz NOT NULL default now() | |
| `finished_at` | timestamptz | |

Indexes: `agent_runs_task_started_idx` (task_id, started_at), `agent_runs_status_idx`, `agent_runs_one_running_per_task_idx` unique partial on task_id WHERE status='running' (**enforces one running run per task**).

## `agent_messages`

Conversation messages for a task.

| Column | Type | Notes |
|---|---|---|
| `id` | text PK | |
| `task_id` | text NOT NULL | FK agent_tasks.id cascade |
| `run_id` | text | FK agent_runs.id ON DELETE set null |
| `role` | text NOT NULL | check `IN ('user','assistant')` |
| `content` | text NOT NULL | |
| `created_at` | | |

Index: `agent_messages_task_created_idx` (task_id, created_at).

## `agent_events`

Structured events emitted during a run (ordered).

| Column | Type | Notes |
|---|---|---|
| `id` | text PK | |
| `run_id` | text NOT NULL | FK agent_runs.id cascade |
| `sequence` | integer NOT NULL | |
| `kind` | text NOT NULL | |
| `payload` | jsonb NOT NULL default `{}` | |
| `created_at` | | |

Index: `agent_events_run_sequence_idx` unique (run_id, sequence).

## `deployments`

Deployment records.

| Column | Type | Notes |
|---|---|---|
| `id` | text PK | |
| `project_id` | text NOT NULL | FK projects.id cascade |
| `node_id` | text NOT NULL default `node_master_local` | FK deployment_nodes.id |
| `status` | text NOT NULL | `running`/`success`/`failed` |
| `trigger` | text NOT NULL | `manual`/`webhook`/`github`/`rollback` |
| `target_ref` | text | git ref deployed |
| `commit_sha`, `commit_message` | text | |
| `rollback_from_deployment_id` | text | |
| `logs` | text NOT NULL default `''` | truncated to `deploymentLogMaxChars` |
| `exit_code` | integer | |
| `started_at` | timestamptz NOT NULL default now() | |
| `finished_at` | timestamptz | |

Indexes: `deployments_project_started_at_idx`, `deployments_status_idx`, `deployments_started_at_idx`.

## `backups`

Backup records (postgres dumps etc.).

| Column | Type | Notes |
|---|---|---|
| `id` | text PK | |
| `project_id` | text | FK projects.id ON DELETE set null |
| `kind` | text NOT NULL | `postgres` (app DB) or `postgres-container` |
| `status` | text NOT NULL | `running`/`success`/`failed` |
| `filename` | text NOT NULL | |
| `file_path` | text NOT NULL | |
| `file_size_bytes` | bigint | |
| `error`, `note` | text | |
| `created_at`, `finished_at`, `downloaded_at` | timestamptz | |
| `download_count` | integer NOT NULL default 0 | |

Indexes: `backups_created_at_idx`, `backups_status_idx`.

## `audit_logs`

Append-only audit trail.

| Column | Type | Notes |
|---|---|---|
| `id` | text PK | |
| `actor` | text NOT NULL | username, or `"github"`/`"worker"`/`"admin"`/`"mcp:<tokenName>"` |
| `action` | text NOT NULL | |
| `entity_type` | text NOT NULL | |
| `entity_id` | text | |
| `project_id` | text | FK projects.id ON DELETE set null |
| `metadata` | jsonb NOT NULL default `{}` | |
| `created_at` | | |

Indexes: `audit_logs_created_at_idx`, `audit_logs_project_created_at_idx` (project_id, created_at).

## `app_settings`

Key/value runtime settings store.

| Column | Type | Notes |
|---|---|---|
| `key` | text PK | |
| `value` | text NOT NULL | JSON-encoded |
| `updated_at` | timestamptz NOT NULL default now() | |

Known keys: `runtime.setting`, `cloudflare.r2`, `cloudflare.tunnel`, `setup.wizard`, `feature.multi_node`, `worker.join_token`, `frp.public_host`.

## `mcp_access_tokens`

Bearer tokens for MCP access.

| Column | Type | Notes |
|---|---|---|
| `id` | text PK | |
| `name` | text NOT NULL | |
| `token_hash` | text NOT NULL | unique |
| `access_level` | text NOT NULL | `read`/`write`/`admin` |
| `last_used_at`, `revoked_at` | timestamptz | |
| `created_at`, `updated_at` | | |

Indexes: `mcp_access_tokens_token_hash_idx` unique, `mcp_access_tokens_revoked_idx`, `mcp_access_tokens_created_at_idx`.

## `cloudflare_clients`

Cloudflare API credentials.

| Column | Type | Notes |
|---|---|---|
| `id` | text PK | |
| `name` | text NOT NULL | |
| `account_id` | text NOT NULL | unique |
| `zone_id` | text NOT NULL default `''` | |
| `api_token` | text NOT NULL | encrypted at app layer |
| `created_at`, `updated_at` | | |

Index: `cloudflare_clients_account_id_idx` unique.

## `cloudflare_tunnels`

Cloudflare Tunnels running on a node.

| Column | Type | Notes |
|---|---|---|
| `id` | text PK | |
| `client_id` | text NOT NULL | FK cloudflare_clients.id ON DELETE restrict |
| `node_id` | text NOT NULL | FK deployment_nodes.id |
| `cf_account_id`, `cf_tunnel_id`, `tunnel_name`, `tunnel_token`, `docker_network_name` | text NOT NULL | tunnel_token encrypted |
| `status` | text NOT NULL default `active` | |
| `last_health_check_at` | timestamptz | |
| `created_at`, `updated_at` | | |

Indexes: `cloudflare_tunnels_node_id_idx`, `cloudflare_tunnels_cf_tunnel_id_idx` unique, `cloudflare_tunnels_network_idx` unique (docker network name).

## `cloudflare_tunnel_assignments`

Binds a tunnel target (compose service or container) to a project.

| Column | Type | Notes |
|---|---|---|
| `id` | text PK | |
| `tunnel_id` | text NOT NULL | FK cloudflare_tunnels.id cascade |
| `target_type` | text NOT NULL | `compose_service`/`container` |
| `project_id` | text | FK projects.id cascade |
| `compose_project`, `compose_service`, `container_name` | text | |
| `created_at` | | |

Indexes: `cloudflare_assignments_tunnel_idx`, `cloudflare_assignments_target_idx` unique (tunnel_id, target_type, compose_project, compose_service, container_name).

## `cloudflare_routes`

Public hostname → tunnel service mappings.

| Column | Type | Notes |
|---|---|---|
| `id` | text PK | |
| `tunnel_id` | text NOT NULL | FK cloudflare_tunnels.id cascade |
| `project_id` | text | FK projects.id cascade |
| `assignment_id` | text | FK cloudflare_tunnel_assignments.id ON DELETE restrict |
| `zone_id`, `hostname`, `service_target` | text NOT NULL | |
| `protocol` | text NOT NULL default `http` | |
| `port` | integer NOT NULL default 80 | |
| `no_tls_verify` | boolean NOT NULL default false | |
| `enabled` | boolean NOT NULL default true | |
| `sync_status` | text NOT NULL default `active` | |
| `last_error` | text | |
| `cf_dns_record_id` | text | |
| `last_published_at` | timestamptz | |
| `created_at`, `updated_at` | | |

Indexes: `cloudflare_routes_tunnel_id_idx`, `cloudflare_routes_project_id_idx`, `cloudflare_routes_hostname_idx` unique (zone_id, hostname).

## `frp_tunnels`

FRP (fast reverse proxy) tunnel definitions.

| Column | Type | Notes |
|---|---|---|
| `id` | text PK | |
| `node_id` | text | FK deployment_nodes.id ON DELETE cascade (nullable) |
| `name` | text NOT NULL | |
| `protocol` | text NOT NULL | check `IN ('tcp','udp')` |
| `local_host` | text NOT NULL | |
| `local_port` | integer NOT NULL | check 1–65535 |
| `remote_port` | integer NOT NULL | check 1–65535 |
| `enabled` | boolean NOT NULL default true | |
| `sync_status` | text NOT NULL default `syncing` | |
| `last_error` | text | |
| `last_synced_at` | timestamptz | |
| `created_at`, `updated_at` | | |

Indexes: `frp_tunnels_node_id_idx`, `frp_tunnels_protocol_remote_port_idx` unique (protocol, remote_port).

## Migration System

- Migrations run under a Postgres advisory lock (`pg_advisory_lock(hashtext('yanto.schema.migrations'))`).
- **Legacy compatibility bridge:** if the `projects` table exists but no migration history is present, a synthetic `legacy-handwritten-schema` row is inserted at a fixed baseline timestamp so the baseline is treated as already applied. This supports databases that pre-existed the migration tooling.
- **Data backfills:** migrate legacy `app_settings.cloudflare.tunnel` JSON into a `cloudflare_clients` row (`cfc_legacy`); and if `ADMIN_USERNAME`/`ADMIN_PASSWORD` are set and the users table is empty, insert an owner user (password hashed if not already bcrypt).
- A second migration applies idempotent `ADD COLUMN IF NOT EXISTS`-style compatibility changes.

## Notes for Reimplementation

- IDs are text with prefixes; a `createId(prefix)` helper generates `<prefix>_<24 hex chars>` (12 random bytes).
- The single-owner invariant is enforced at the DB layer (partial unique index), not just app logic.
- Session revocation is via `session_version` bumping.
- Container↔project mapping relies on Docker Compose `com.docker.compose.project` labels matching `projects.folder_name`, and requires folder-name uniqueness for unambiguous authorization (returns 409 otherwise).
- `app_settings` is a generic key/value table backing both runtime-config (overrides env) and feature state.
