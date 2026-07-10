import { bigint, boolean, index, integer, jsonb, pgTable, text, timestamp, uniqueIndex } from "drizzle-orm/pg-core";

export const deploymentNodes = pgTable(
  "deployment_nodes",
  {
    id: text("id").primaryKey(),
    name: text("name").notNull(),
    role: text("role").notNull(),
    status: text("status").notNull().default("offline"),
    lastSeenAt: timestamp("last_seen_at", { withTimezone: true }),
    dockerVersion: text("docker_version"),
    labels: jsonb("labels").$type<Record<string, string>>().notNull().default({}),
    tokenHash: text("token_hash"),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
    updatedAt: timestamp("updated_at", { withTimezone: true }).notNull().defaultNow()
  },
  (table) => [index("deployment_nodes_role_idx").on(table.role), index("deployment_nodes_status_idx").on(table.status), index("deployment_nodes_token_hash_idx").on(table.tokenHash)]
);

export const projects = pgTable(
  "projects",
  {
    id: text("id").primaryKey(),
    name: text("name").notNull(),
    gitUrl: text("git_url"),
    branch: text("branch").notNull(),
    folderName: text("folder_name").notNull(),
    localPath: text("local_path").notNull(),
    composeFile: text("compose_file").notNull(),
    composeContent: text("compose_content"),
    envFile: text("env_file").notNull().default(".env"),
    autoStart: boolean("auto_start").notNull().default(false),
    manualDeployEnabled: boolean("manual_deploy_enabled").notNull().default(true),
    githubWebhookEnabled: boolean("github_webhook_enabled").notNull().default(true),
    targetNodeId: text("target_node_id").notNull().default("node_master_local").references(() => deploymentNodes.id),
    deployToken: text("deploy_token").notNull(),
    sshPrivateKeyPath: text("ssh_private_key_path"),
    sshPublicKey: text("ssh_public_key"),
    agentImage: text("agent_image").notNull().default(""),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
    updatedAt: timestamp("updated_at", { withTimezone: true }).notNull().defaultNow()
  },
  (table) => [index("projects_created_at_idx").on(table.createdAt), index("projects_folder_name_idx").on(table.folderName)]
);

export const aiProviders = pgTable(
  "ai_providers",
  {
    id: text("id").primaryKey(),
    name: text("name").notNull(),
    protocol: text("protocol").notNull(),
    baseUrl: text("base_url").notNull(),
    apiKey: text("api_key").notNull(),
    enabled: boolean("enabled").notNull().default(true),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
    updatedAt: timestamp("updated_at", { withTimezone: true }).notNull().defaultNow()
  },
  (table) => [index("ai_providers_created_at_idx").on(table.createdAt)]
);

export const aiModels = pgTable(
  "ai_models",
  {
    id: text("id").primaryKey(),
    providerId: text("provider_id").notNull().references(() => aiProviders.id, { onDelete: "cascade" }),
    modelId: text("model_id").notNull(),
    displayName: text("display_name").notNull(),
    enabled: boolean("enabled").notNull().default(true),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
    updatedAt: timestamp("updated_at", { withTimezone: true }).notNull().defaultNow()
  },
  (table) => [
    uniqueIndex("ai_models_provider_model_idx").on(table.providerId, table.modelId),
    index("ai_models_provider_idx").on(table.providerId)
  ]
);

export const agentTasks = pgTable(
  "agent_tasks",
  {
    id: text("id").primaryKey(),
    projectId: text("project_id").notNull().references(() => projects.id, { onDelete: "cascade" }),
    modelId: text("model_id").notNull().references(() => aiModels.id, { onDelete: "restrict" }),
    title: text("title").notNull(),
    prompt: text("prompt").notNull(),
    status: text("status").notNull().default("backlog"),
    sourceBranch: text("source_branch").notNull(),
    taskBranch: text("task_branch").notNull(),
    sourceSha: text("source_sha"),
    worktreePath: text("worktree_path"),
    resumeExistingBranch: boolean("resume_existing_branch").notNull().default(false),
    autoCommit: boolean("auto_commit").notNull().default(false),
    autoPush: boolean("auto_push").notNull().default(false),
    autoCleanup: boolean("auto_cleanup").notNull().default(false),
    lastError: text("last_error"),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
    updatedAt: timestamp("updated_at", { withTimezone: true }).notNull().defaultNow(),
    startedAt: timestamp("started_at", { withTimezone: true }),
    finishedAt: timestamp("finished_at", { withTimezone: true }),
    pushedAt: timestamp("pushed_at", { withTimezone: true })
  },
  (table) => [
    index("agent_tasks_status_created_idx").on(table.status, table.createdAt),
    index("agent_tasks_project_idx").on(table.projectId),
    uniqueIndex("agent_tasks_project_branch_idx").on(table.projectId, table.taskBranch)
  ]
);

export const agentRuns = pgTable(
  "agent_runs",
  {
    id: text("id").primaryKey(),
    taskId: text("task_id").notNull().references(() => agentTasks.id, { onDelete: "cascade" }),
    status: text("status").notNull(),
    providerProtocol: text("provider_protocol").notNull(),
    modelName: text("model_name").notNull(),
    assistantText: text("assistant_text").notNull().default(""),
    error: text("error"),
    startedAt: timestamp("started_at", { withTimezone: true }).notNull().defaultNow(),
    finishedAt: timestamp("finished_at", { withTimezone: true })
  },
  (table) => [index("agent_runs_task_started_idx").on(table.taskId, table.startedAt), index("agent_runs_status_idx").on(table.status)]
);

export const agentMessages = pgTable(
  "agent_messages",
  {
    id: text("id").primaryKey(),
    taskId: text("task_id").notNull().references(() => agentTasks.id, { onDelete: "cascade" }),
    runId: text("run_id").references(() => agentRuns.id, { onDelete: "set null" }),
    role: text("role").notNull(),
    content: text("content").notNull(),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow()
  },
  (table) => [index("agent_messages_task_created_idx").on(table.taskId, table.createdAt)]
);

export const agentEvents = pgTable(
  "agent_events",
  {
    id: text("id").primaryKey(),
    runId: text("run_id").notNull().references(() => agentRuns.id, { onDelete: "cascade" }),
    sequence: integer("sequence").notNull(),
    kind: text("kind").notNull(),
    payload: jsonb("payload").$type<Record<string, unknown>>().notNull().default({}),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow()
  },
  (table) => [uniqueIndex("agent_events_run_sequence_idx").on(table.runId, table.sequence)]
);

export const deployments = pgTable(
  "deployments",
  {
    id: text("id").primaryKey(),
    projectId: text("project_id").notNull().references(() => projects.id, { onDelete: "cascade" }),
    nodeId: text("node_id").notNull().default("node_master_local").references(() => deploymentNodes.id),
    status: text("status").notNull(),
    trigger: text("trigger").notNull(),
    targetRef: text("target_ref"),
    commitSha: text("commit_sha"),
    commitMessage: text("commit_message"),
    rollbackFromDeploymentId: text("rollback_from_deployment_id"),
    logs: text("logs").notNull().default(""),
    exitCode: integer("exit_code"),
    startedAt: timestamp("started_at", { withTimezone: true }).notNull().defaultNow(),
    finishedAt: timestamp("finished_at", { withTimezone: true })
  },
  (table) => [
    index("deployments_project_started_at_idx").on(table.projectId, table.startedAt),
    index("deployments_status_idx").on(table.status),
    index("deployments_started_at_idx").on(table.startedAt)
  ]
);

export const backups = pgTable(
  "backups",
  {
    id: text("id").primaryKey(),
    projectId: text("project_id").references(() => projects.id, { onDelete: "set null" }),
    kind: text("kind").notNull(),
    status: text("status").notNull(),
    filename: text("filename").notNull(),
    filePath: text("file_path").notNull(),
    fileSizeBytes: bigint("file_size_bytes", { mode: "number" }),
    error: text("error"),
    note: text("note"),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
    finishedAt: timestamp("finished_at", { withTimezone: true }),
    downloadedAt: timestamp("downloaded_at", { withTimezone: true }),
    downloadCount: integer("download_count").notNull().default(0)
  },
  (table) => [index("backups_created_at_idx").on(table.createdAt), index("backups_status_idx").on(table.status)]
);

export const auditLogs = pgTable(
  "audit_logs",
  {
    id: text("id").primaryKey(),
    actor: text("actor").notNull(),
    action: text("action").notNull(),
    entityType: text("entity_type").notNull(),
    entityId: text("entity_id"),
    projectId: text("project_id").references(() => projects.id, { onDelete: "set null" }),
    metadata: jsonb("metadata").$type<Record<string, unknown>>().notNull().default({}),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow()
  },
  (table) => [index("audit_logs_created_at_idx").on(table.createdAt), index("audit_logs_project_created_at_idx").on(table.projectId, table.createdAt)]
);

export const appSettings = pgTable("app_settings", {
  key: text("key").primaryKey(),
  value: text("value").notNull(),
  updatedAt: timestamp("updated_at", { withTimezone: true }).notNull().defaultNow()
});

export const mcpAccessTokens = pgTable(
  "mcp_access_tokens",
  {
    id: text("id").primaryKey(),
    name: text("name").notNull(),
    tokenHash: text("token_hash").notNull(),
    accessLevel: text("access_level").notNull(),
    lastUsedAt: timestamp("last_used_at", { withTimezone: true }),
    revokedAt: timestamp("revoked_at", { withTimezone: true }),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
    updatedAt: timestamp("updated_at", { withTimezone: true }).notNull().defaultNow()
  },
  (table) => [
    uniqueIndex("mcp_access_tokens_token_hash_idx").on(table.tokenHash),
    index("mcp_access_tokens_revoked_idx").on(table.revokedAt),
    index("mcp_access_tokens_created_at_idx").on(table.createdAt)
  ]
);

export const cloudflareClients = pgTable(
  "cloudflare_clients",
  {
    id: text("id").primaryKey(),
    name: text("name").notNull(),
    accountId: text("account_id").notNull(),
    zoneId: text("zone_id").notNull().default(""),
    apiToken: text("api_token").notNull(),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
    updatedAt: timestamp("updated_at", { withTimezone: true }).notNull().defaultNow()
  },
  (table) => [uniqueIndex("cloudflare_clients_account_id_idx").on(table.accountId)]
);

export const cloudflareTunnels = pgTable(
  "cloudflare_tunnels",
  {
    id: text("id").primaryKey(),
    clientId: text("client_id").notNull().references(() => cloudflareClients.id, { onDelete: "restrict" }),
    nodeId: text("node_id").notNull().references(() => deploymentNodes.id),
    cfAccountId: text("cf_account_id").notNull(),
    cfTunnelId: text("cf_tunnel_id").notNull(),
    tunnelName: text("tunnel_name").notNull(),
    tunnelToken: text("tunnel_token").notNull(),
    dockerNetworkName: text("docker_network_name").notNull(),
    status: text("status").notNull().default("active"),
    lastHealthCheckAt: timestamp("last_health_check_at", { withTimezone: true }),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
    updatedAt: timestamp("updated_at", { withTimezone: true }).notNull().defaultNow()
  },
  (table) => [index("cloudflare_tunnels_node_id_idx").on(table.nodeId), uniqueIndex("cloudflare_tunnels_cf_tunnel_id_idx").on(table.cfTunnelId), uniqueIndex("cloudflare_tunnels_network_idx").on(table.dockerNetworkName)]
);

export const cloudflareTunnelAssignments = pgTable(
  "cloudflare_tunnel_assignments",
  {
    id: text("id").primaryKey(),
    tunnelId: text("tunnel_id").notNull().references(() => cloudflareTunnels.id, { onDelete: "cascade" }),
    targetType: text("target_type").notNull(),
    projectId: text("project_id").references(() => projects.id, { onDelete: "cascade" }),
    composeProject: text("compose_project"),
    composeService: text("compose_service"),
    containerName: text("container_name"),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow()
  },
  (table) => [index("cloudflare_assignments_tunnel_idx").on(table.tunnelId), uniqueIndex("cloudflare_assignments_target_idx").on(table.tunnelId, table.targetType, table.composeProject, table.composeService, table.containerName)]
);

export const cloudflareRoutes = pgTable(
  "cloudflare_routes",
  {
    id: text("id").primaryKey(),
    tunnelId: text("tunnel_id").notNull().references(() => cloudflareTunnels.id, { onDelete: "cascade" }),
    projectId: text("project_id").references(() => projects.id, { onDelete: "cascade" }),
    assignmentId: text("assignment_id").references(() => cloudflareTunnelAssignments.id, { onDelete: "restrict" }),
    zoneId: text("zone_id").notNull(),
    hostname: text("hostname").notNull(),
    serviceTarget: text("service_target").notNull(),
    protocol: text("protocol").notNull().default("http"),
    port: integer("port").notNull().default(80),
    noTlsVerify: boolean("no_tls_verify").notNull().default(false),
    enabled: boolean("enabled").notNull().default(true),
    syncStatus: text("sync_status").notNull().default("active"),
    lastError: text("last_error"),
    cfDnsRecordId: text("cf_dns_record_id"),
    lastPublishedAt: timestamp("last_published_at", { withTimezone: true }),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
    updatedAt: timestamp("updated_at", { withTimezone: true }).notNull().defaultNow()
  },
  (table) => [
    index("cloudflare_routes_tunnel_id_idx").on(table.tunnelId),
    index("cloudflare_routes_project_id_idx").on(table.projectId),
    uniqueIndex("cloudflare_routes_hostname_idx").on(table.zoneId, table.hostname)
  ]
);

export const frpTunnels = pgTable(
  "frp_tunnels",
  {
    id: text("id").primaryKey(),
    nodeId: text("node_id").references(() => deploymentNodes.id, { onDelete: "cascade" }),
    name: text("name").notNull(),
    protocol: text("protocol").notNull(),
    localHost: text("local_host").notNull(),
    localPort: integer("local_port").notNull(),
    remotePort: integer("remote_port").notNull(),
    enabled: boolean("enabled").notNull().default(true),
    syncStatus: text("sync_status").notNull().default("syncing"),
    lastError: text("last_error"),
    lastSyncedAt: timestamp("last_synced_at", { withTimezone: true }),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
    updatedAt: timestamp("updated_at", { withTimezone: true }).notNull().defaultNow()
  },
  (table) => [
    index("frp_tunnels_node_id_idx").on(table.nodeId),
    uniqueIndex("frp_tunnels_protocol_remote_port_idx").on(table.protocol, table.remotePort)
  ]
);

export type ProjectRow = typeof projects.$inferSelect;
export type NewProjectRow = typeof projects.$inferInsert;
export type AiProviderRow = typeof aiProviders.$inferSelect;
export type AiModelRow = typeof aiModels.$inferSelect;
export type AgentTaskRow = typeof agentTasks.$inferSelect;
export type AgentRunRow = typeof agentRuns.$inferSelect;
export type AgentMessageRow = typeof agentMessages.$inferSelect;
export type DeploymentNodeRow = typeof deploymentNodes.$inferSelect;
export type NewDeploymentNodeRow = typeof deploymentNodes.$inferInsert;
export type DeploymentRow = typeof deployments.$inferSelect;
export type NewDeploymentRow = typeof deployments.$inferInsert;
export type BackupRow = typeof backups.$inferSelect;
export type NewBackupRow = typeof backups.$inferInsert;
export type AuditLogRow = typeof auditLogs.$inferSelect;
export type NewAuditLogRow = typeof auditLogs.$inferInsert;
export type AppSettingRow = typeof appSettings.$inferSelect;
export type McpAccessTokenRow = typeof mcpAccessTokens.$inferSelect;
export type NewMcpAccessTokenRow = typeof mcpAccessTokens.$inferInsert;
export type CloudflareClientRow = typeof cloudflareClients.$inferSelect;
export type CloudflareTunnelRow = typeof cloudflareTunnels.$inferSelect;
export type CloudflareTunnelAssignmentRow = typeof cloudflareTunnelAssignments.$inferSelect;
export type NewCloudflareTunnelRow = typeof cloudflareTunnels.$inferInsert;
export type CloudflareRouteRow = typeof cloudflareRoutes.$inferSelect;
export type NewCloudflareRouteRow = typeof cloudflareRoutes.$inferInsert;
export type FrpTunnelRow = typeof frpTunnels.$inferSelect;
export type NewFrpTunnelRow = typeof frpTunnels.$inferInsert;
