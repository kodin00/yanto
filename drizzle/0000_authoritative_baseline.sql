CREATE TABLE "account_tokens" (
	"id" text PRIMARY KEY NOT NULL,
	"user_id" text NOT NULL,
	"purpose" text NOT NULL,
	"token_hash" text NOT NULL,
	"expires_at" timestamp with time zone NOT NULL,
	"used_at" timestamp with time zone,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "account_tokens_purpose_check" CHECK ("account_tokens"."purpose" IN ('invite', 'reset'))
);
--> statement-breakpoint
CREATE TABLE "agent_events" (
	"id" text PRIMARY KEY NOT NULL,
	"run_id" text NOT NULL,
	"sequence" integer NOT NULL,
	"kind" text NOT NULL,
	"payload" jsonb DEFAULT '{}'::jsonb NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE "agent_messages" (
	"id" text PRIMARY KEY NOT NULL,
	"task_id" text NOT NULL,
	"run_id" text,
	"role" text NOT NULL,
	"content" text NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "agent_messages_role_check" CHECK ("agent_messages"."role" IN ('user', 'assistant'))
);
--> statement-breakpoint
CREATE TABLE "agent_runs" (
	"id" text PRIMARY KEY NOT NULL,
	"task_id" text NOT NULL,
	"status" text NOT NULL,
	"provider_protocol" text NOT NULL,
	"model_name" text NOT NULL,
	"assistant_text" text DEFAULT '' NOT NULL,
	"error" text,
	"started_at" timestamp with time zone DEFAULT now() NOT NULL,
	"finished_at" timestamp with time zone,
	CONSTRAINT "agent_runs_status_check" CHECK ("agent_runs"."status" IN ('running', 'succeeded', 'failed', 'canceled'))
);
--> statement-breakpoint
CREATE TABLE "agent_tasks" (
	"id" text PRIMARY KEY NOT NULL,
	"project_id" text NOT NULL,
	"model_id" text NOT NULL,
	"title" text NOT NULL,
	"prompt" text NOT NULL,
	"status" text DEFAULT 'backlog' NOT NULL,
	"source_branch" text NOT NULL,
	"task_branch" text NOT NULL,
	"source_sha" text,
	"worktree_path" text,
	"codex_thread_id" text,
	"resume_existing_branch" boolean DEFAULT false NOT NULL,
	"auto_commit" boolean DEFAULT false NOT NULL,
	"auto_push" boolean DEFAULT false NOT NULL,
	"auto_cleanup" boolean DEFAULT false NOT NULL,
	"last_error" text,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL,
	"started_at" timestamp with time zone,
	"finished_at" timestamp with time zone,
	"pushed_at" timestamp with time zone,
	"archived_at" timestamp with time zone,
	CONSTRAINT "agent_tasks_status_check" CHECK ("agent_tasks"."status" IN ('backlog', 'running', 'review', 'done'))
);
--> statement-breakpoint
CREATE TABLE "ai_models" (
	"id" text PRIMARY KEY NOT NULL,
	"provider_id" text NOT NULL,
	"model_id" text NOT NULL,
	"display_name" text NOT NULL,
	"enabled" boolean DEFAULT true NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE "ai_providers" (
	"id" text PRIMARY KEY NOT NULL,
	"name" text NOT NULL,
	"protocol" text NOT NULL,
	"base_url" text NOT NULL,
	"api_key" text NOT NULL,
	"enabled" boolean DEFAULT true NOT NULL,
	"default_model_id" text,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "ai_providers_protocol_check" CHECK ("ai_providers"."protocol" IN ('openai_responses', 'openai_chat', 'anthropic_messages', 'codex_account'))
);
--> statement-breakpoint
CREATE TABLE "app_settings" (
	"key" text PRIMARY KEY NOT NULL,
	"value" text NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE "audit_logs" (
	"id" text PRIMARY KEY NOT NULL,
	"actor" text NOT NULL,
	"action" text NOT NULL,
	"entity_type" text NOT NULL,
	"entity_id" text,
	"project_id" text,
	"metadata" jsonb DEFAULT '{}'::jsonb NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE "backups" (
	"id" text PRIMARY KEY NOT NULL,
	"project_id" text,
	"kind" text NOT NULL,
	"status" text NOT NULL,
	"filename" text NOT NULL,
	"file_path" text NOT NULL,
	"file_size_bytes" bigint,
	"error" text,
	"note" text,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"finished_at" timestamp with time zone,
	"downloaded_at" timestamp with time zone,
	"download_count" integer DEFAULT 0 NOT NULL
);
--> statement-breakpoint
CREATE TABLE "cloudflare_clients" (
	"id" text PRIMARY KEY NOT NULL,
	"name" text NOT NULL,
	"account_id" text NOT NULL,
	"zone_id" text DEFAULT '' NOT NULL,
	"api_token" text NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE "cloudflare_routes" (
	"id" text PRIMARY KEY NOT NULL,
	"tunnel_id" text NOT NULL,
	"project_id" text,
	"assignment_id" text,
	"zone_id" text NOT NULL,
	"hostname" text NOT NULL,
	"service_target" text NOT NULL,
	"protocol" text DEFAULT 'http' NOT NULL,
	"port" integer DEFAULT 80 NOT NULL,
	"no_tls_verify" boolean DEFAULT false NOT NULL,
	"enabled" boolean DEFAULT true NOT NULL,
	"sync_status" text DEFAULT 'active' NOT NULL,
	"last_error" text,
	"cf_dns_record_id" text,
	"last_published_at" timestamp with time zone,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE "cloudflare_tunnel_assignments" (
	"id" text PRIMARY KEY NOT NULL,
	"tunnel_id" text NOT NULL,
	"target_type" text NOT NULL,
	"project_id" text,
	"compose_project" text,
	"compose_service" text,
	"container_name" text,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE "cloudflare_tunnels" (
	"id" text PRIMARY KEY NOT NULL,
	"client_id" text NOT NULL,
	"node_id" text NOT NULL,
	"cf_account_id" text NOT NULL,
	"cf_tunnel_id" text NOT NULL,
	"tunnel_name" text NOT NULL,
	"tunnel_token" text NOT NULL,
	"docker_network_name" text NOT NULL,
	"status" text DEFAULT 'active' NOT NULL,
	"last_health_check_at" timestamp with time zone,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE "deployment_nodes" (
	"id" text PRIMARY KEY NOT NULL,
	"name" text NOT NULL,
	"role" text NOT NULL,
	"status" text DEFAULT 'offline' NOT NULL,
	"last_seen_at" timestamp with time zone,
	"docker_version" text,
	"labels" jsonb DEFAULT '{}'::jsonb NOT NULL,
	"token_hash" text,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE "deployments" (
	"id" text PRIMARY KEY NOT NULL,
	"project_id" text NOT NULL,
	"node_id" text DEFAULT 'node_master_local' NOT NULL,
	"status" text NOT NULL,
	"trigger" text NOT NULL,
	"target_ref" text,
	"commit_sha" text,
	"commit_message" text,
	"rollback_from_deployment_id" text,
	"logs" text DEFAULT '' NOT NULL,
	"exit_code" integer,
	"started_at" timestamp with time zone DEFAULT now() NOT NULL,
	"finished_at" timestamp with time zone
);
--> statement-breakpoint
CREATE TABLE "frp_tunnels" (
	"id" text PRIMARY KEY NOT NULL,
	"node_id" text,
	"name" text NOT NULL,
	"protocol" text NOT NULL,
	"local_host" text NOT NULL,
	"local_port" integer NOT NULL,
	"remote_port" integer NOT NULL,
	"enabled" boolean DEFAULT true NOT NULL,
	"sync_status" text DEFAULT 'syncing' NOT NULL,
	"last_error" text,
	"last_synced_at" timestamp with time zone,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "frp_tunnels_protocol_check" CHECK ("frp_tunnels"."protocol" IN ('tcp', 'udp')),
	CONSTRAINT "frp_tunnels_local_port_check" CHECK ("frp_tunnels"."local_port" BETWEEN 1 AND 65535),
	CONSTRAINT "frp_tunnels_remote_port_check" CHECK ("frp_tunnels"."remote_port" BETWEEN 1 AND 65535)
);
--> statement-breakpoint
CREATE TABLE "mcp_access_tokens" (
	"id" text PRIMARY KEY NOT NULL,
	"name" text NOT NULL,
	"token_hash" text NOT NULL,
	"access_level" text NOT NULL,
	"last_used_at" timestamp with time zone,
	"revoked_at" timestamp with time zone,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE "projects" (
	"id" text PRIMARY KEY NOT NULL,
	"name" text NOT NULL,
	"git_url" text,
	"docker_image" text DEFAULT '' NOT NULL,
	"branch" text NOT NULL,
	"folder_name" text NOT NULL,
	"local_path" text NOT NULL,
	"compose_file" text NOT NULL,
	"compose_content" text,
	"env_file" text DEFAULT '.env' NOT NULL,
	"auto_start" boolean DEFAULT false NOT NULL,
	"manual_deploy_enabled" boolean DEFAULT true NOT NULL,
	"github_webhook_enabled" boolean DEFAULT true NOT NULL,
	"target_node_id" text DEFAULT 'node_master_local' NOT NULL,
	"deploy_token" text NOT NULL,
	"ssh_private_key_path" text,
	"ssh_public_key" text,
	"agent_image" text DEFAULT '' NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE "user_project_access" (
	"user_id" text NOT NULL,
	"project_id" text NOT NULL,
	"permissions" jsonb DEFAULT '[]'::jsonb NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "user_project_access_user_id_project_id_pk" PRIMARY KEY("user_id","project_id"),
	CONSTRAINT "user_project_access_permissions_array_check" CHECK (jsonb_typeof("user_project_access"."permissions") = 'array')
);
--> statement-breakpoint
CREATE TABLE "users" (
	"id" text PRIMARY KEY NOT NULL,
	"username" text NOT NULL,
	"role" text NOT NULL,
	"status" text NOT NULL,
	"password_hash" text,
	"session_version" integer DEFAULT 1 NOT NULL,
	"last_login_at" timestamp with time zone,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "users_role_check" CHECK ("users"."role" IN ('owner', 'member')),
	CONSTRAINT "users_status_check" CHECK ("users"."status" IN ('invited', 'active', 'disabled')),
	CONSTRAINT "users_username_nonempty_check" CHECK (length("users"."username") > 0)
);
--> statement-breakpoint
ALTER TABLE "account_tokens" ADD CONSTRAINT "account_tokens_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "agent_events" ADD CONSTRAINT "agent_events_run_id_agent_runs_id_fk" FOREIGN KEY ("run_id") REFERENCES "public"."agent_runs"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "agent_messages" ADD CONSTRAINT "agent_messages_task_id_agent_tasks_id_fk" FOREIGN KEY ("task_id") REFERENCES "public"."agent_tasks"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "agent_messages" ADD CONSTRAINT "agent_messages_run_id_agent_runs_id_fk" FOREIGN KEY ("run_id") REFERENCES "public"."agent_runs"("id") ON DELETE set null ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "agent_runs" ADD CONSTRAINT "agent_runs_task_id_agent_tasks_id_fk" FOREIGN KEY ("task_id") REFERENCES "public"."agent_tasks"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "agent_tasks" ADD CONSTRAINT "agent_tasks_project_id_projects_id_fk" FOREIGN KEY ("project_id") REFERENCES "public"."projects"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "agent_tasks" ADD CONSTRAINT "agent_tasks_model_id_ai_models_id_fk" FOREIGN KEY ("model_id") REFERENCES "public"."ai_models"("id") ON DELETE restrict ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "ai_models" ADD CONSTRAINT "ai_models_provider_id_ai_providers_id_fk" FOREIGN KEY ("provider_id") REFERENCES "public"."ai_providers"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "ai_providers" ADD CONSTRAINT "ai_providers_default_model_id_ai_models_id_fk" FOREIGN KEY ("default_model_id") REFERENCES "public"."ai_models"("id") ON DELETE set null ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "audit_logs" ADD CONSTRAINT "audit_logs_project_id_projects_id_fk" FOREIGN KEY ("project_id") REFERENCES "public"."projects"("id") ON DELETE set null ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "backups" ADD CONSTRAINT "backups_project_id_projects_id_fk" FOREIGN KEY ("project_id") REFERENCES "public"."projects"("id") ON DELETE set null ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "cloudflare_routes" ADD CONSTRAINT "cloudflare_routes_tunnel_id_cloudflare_tunnels_id_fk" FOREIGN KEY ("tunnel_id") REFERENCES "public"."cloudflare_tunnels"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "cloudflare_routes" ADD CONSTRAINT "cloudflare_routes_project_id_projects_id_fk" FOREIGN KEY ("project_id") REFERENCES "public"."projects"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "cloudflare_routes" ADD CONSTRAINT "cloudflare_routes_assignment_id_cloudflare_tunnel_assignments_id_fk" FOREIGN KEY ("assignment_id") REFERENCES "public"."cloudflare_tunnel_assignments"("id") ON DELETE restrict ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "cloudflare_tunnel_assignments" ADD CONSTRAINT "cloudflare_tunnel_assignments_tunnel_id_cloudflare_tunnels_id_fk" FOREIGN KEY ("tunnel_id") REFERENCES "public"."cloudflare_tunnels"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "cloudflare_tunnel_assignments" ADD CONSTRAINT "cloudflare_tunnel_assignments_project_id_projects_id_fk" FOREIGN KEY ("project_id") REFERENCES "public"."projects"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "cloudflare_tunnels" ADD CONSTRAINT "cloudflare_tunnels_client_id_cloudflare_clients_id_fk" FOREIGN KEY ("client_id") REFERENCES "public"."cloudflare_clients"("id") ON DELETE restrict ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "cloudflare_tunnels" ADD CONSTRAINT "cloudflare_tunnels_node_id_deployment_nodes_id_fk" FOREIGN KEY ("node_id") REFERENCES "public"."deployment_nodes"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "deployments" ADD CONSTRAINT "deployments_project_id_projects_id_fk" FOREIGN KEY ("project_id") REFERENCES "public"."projects"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "deployments" ADD CONSTRAINT "deployments_node_id_deployment_nodes_id_fk" FOREIGN KEY ("node_id") REFERENCES "public"."deployment_nodes"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "frp_tunnels" ADD CONSTRAINT "frp_tunnels_node_id_deployment_nodes_id_fk" FOREIGN KEY ("node_id") REFERENCES "public"."deployment_nodes"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "projects" ADD CONSTRAINT "projects_target_node_id_deployment_nodes_id_fk" FOREIGN KEY ("target_node_id") REFERENCES "public"."deployment_nodes"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "user_project_access" ADD CONSTRAINT "user_project_access_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "user_project_access" ADD CONSTRAINT "user_project_access_project_id_projects_id_fk" FOREIGN KEY ("project_id") REFERENCES "public"."projects"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
CREATE UNIQUE INDEX "account_tokens_hash_idx" ON "account_tokens" USING btree ("token_hash");--> statement-breakpoint
CREATE INDEX "account_tokens_user_active_idx" ON "account_tokens" USING btree ("user_id","used_at");--> statement-breakpoint
CREATE INDEX "account_tokens_expires_idx" ON "account_tokens" USING btree ("expires_at");--> statement-breakpoint
CREATE UNIQUE INDEX "agent_events_run_sequence_idx" ON "agent_events" USING btree ("run_id","sequence");--> statement-breakpoint
CREATE INDEX "agent_messages_task_created_idx" ON "agent_messages" USING btree ("task_id","created_at");--> statement-breakpoint
CREATE INDEX "agent_runs_task_started_idx" ON "agent_runs" USING btree ("task_id","started_at");--> statement-breakpoint
CREATE INDEX "agent_runs_status_idx" ON "agent_runs" USING btree ("status");--> statement-breakpoint
CREATE UNIQUE INDEX "agent_runs_one_running_per_task_idx" ON "agent_runs" USING btree ("task_id") WHERE "agent_runs"."status" = 'running';--> statement-breakpoint
CREATE INDEX "agent_tasks_status_created_idx" ON "agent_tasks" USING btree ("status","created_at");--> statement-breakpoint
CREATE INDEX "agent_tasks_archived_updated_idx" ON "agent_tasks" USING btree ("archived_at","updated_at");--> statement-breakpoint
CREATE INDEX "agent_tasks_project_idx" ON "agent_tasks" USING btree ("project_id");--> statement-breakpoint
CREATE UNIQUE INDEX "agent_tasks_project_branch_idx" ON "agent_tasks" USING btree ("project_id","task_branch");--> statement-breakpoint
CREATE UNIQUE INDEX "ai_models_provider_model_idx" ON "ai_models" USING btree ("provider_id","model_id");--> statement-breakpoint
CREATE INDEX "ai_models_provider_idx" ON "ai_models" USING btree ("provider_id");--> statement-breakpoint
CREATE INDEX "ai_providers_created_at_idx" ON "ai_providers" USING btree ("created_at");--> statement-breakpoint
CREATE INDEX "audit_logs_created_at_idx" ON "audit_logs" USING btree ("created_at");--> statement-breakpoint
CREATE INDEX "audit_logs_project_created_at_idx" ON "audit_logs" USING btree ("project_id","created_at");--> statement-breakpoint
CREATE INDEX "backups_created_at_idx" ON "backups" USING btree ("created_at");--> statement-breakpoint
CREATE INDEX "backups_status_idx" ON "backups" USING btree ("status");--> statement-breakpoint
CREATE UNIQUE INDEX "cloudflare_clients_account_id_idx" ON "cloudflare_clients" USING btree ("account_id");--> statement-breakpoint
CREATE INDEX "cloudflare_routes_tunnel_id_idx" ON "cloudflare_routes" USING btree ("tunnel_id");--> statement-breakpoint
CREATE INDEX "cloudflare_routes_project_id_idx" ON "cloudflare_routes" USING btree ("project_id");--> statement-breakpoint
CREATE UNIQUE INDEX "cloudflare_routes_hostname_idx" ON "cloudflare_routes" USING btree ("zone_id","hostname");--> statement-breakpoint
CREATE INDEX "cloudflare_assignments_tunnel_idx" ON "cloudflare_tunnel_assignments" USING btree ("tunnel_id");--> statement-breakpoint
CREATE UNIQUE INDEX "cloudflare_assignments_target_idx" ON "cloudflare_tunnel_assignments" USING btree ("tunnel_id","target_type","compose_project","compose_service","container_name");--> statement-breakpoint
CREATE INDEX "cloudflare_tunnels_node_id_idx" ON "cloudflare_tunnels" USING btree ("node_id");--> statement-breakpoint
CREATE UNIQUE INDEX "cloudflare_tunnels_cf_tunnel_id_idx" ON "cloudflare_tunnels" USING btree ("cf_tunnel_id");--> statement-breakpoint
CREATE UNIQUE INDEX "cloudflare_tunnels_network_idx" ON "cloudflare_tunnels" USING btree ("docker_network_name");--> statement-breakpoint
CREATE INDEX "deployment_nodes_role_idx" ON "deployment_nodes" USING btree ("role");--> statement-breakpoint
CREATE INDEX "deployment_nodes_status_idx" ON "deployment_nodes" USING btree ("status");--> statement-breakpoint
CREATE INDEX "deployment_nodes_token_hash_idx" ON "deployment_nodes" USING btree ("token_hash");--> statement-breakpoint
CREATE INDEX "deployments_project_started_at_idx" ON "deployments" USING btree ("project_id","started_at");--> statement-breakpoint
CREATE INDEX "deployments_status_idx" ON "deployments" USING btree ("status");--> statement-breakpoint
CREATE INDEX "deployments_started_at_idx" ON "deployments" USING btree ("started_at");--> statement-breakpoint
CREATE INDEX "frp_tunnels_node_id_idx" ON "frp_tunnels" USING btree ("node_id");--> statement-breakpoint
CREATE UNIQUE INDEX "frp_tunnels_protocol_remote_port_idx" ON "frp_tunnels" USING btree ("protocol","remote_port");--> statement-breakpoint
CREATE UNIQUE INDEX "mcp_access_tokens_token_hash_idx" ON "mcp_access_tokens" USING btree ("token_hash");--> statement-breakpoint
CREATE INDEX "mcp_access_tokens_revoked_idx" ON "mcp_access_tokens" USING btree ("revoked_at");--> statement-breakpoint
CREATE INDEX "mcp_access_tokens_created_at_idx" ON "mcp_access_tokens" USING btree ("created_at");--> statement-breakpoint
CREATE INDEX "projects_created_at_idx" ON "projects" USING btree ("created_at");--> statement-breakpoint
CREATE INDEX "projects_folder_name_idx" ON "projects" USING btree ("folder_name");--> statement-breakpoint
CREATE INDEX "user_project_access_project_idx" ON "user_project_access" USING btree ("project_id");--> statement-breakpoint
CREATE UNIQUE INDEX "users_username_lower_idx" ON "users" USING btree (lower("username"));--> statement-breakpoint
CREATE UNIQUE INDEX "users_single_owner_idx" ON "users" USING btree ("role") WHERE "users"."role" = 'owner';--> statement-breakpoint
CREATE INDEX "users_status_idx" ON "users" USING btree ("status");