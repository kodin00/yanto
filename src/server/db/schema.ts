import { bigint, boolean, index, integer, jsonb, pgTable, text, timestamp } from "drizzle-orm/pg-core";

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
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
    updatedAt: timestamp("updated_at", { withTimezone: true }).notNull().defaultNow()
  },
  (table) => [index("projects_created_at_idx").on(table.createdAt), index("projects_folder_name_idx").on(table.folderName)]
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

export const cloudflareTunnels = pgTable(
  "cloudflare_tunnels",
  {
    id: text("id").primaryKey(),
    nodeId: text("node_id").notNull().references(() => deploymentNodes.id),
    cfAccountId: text("cf_account_id").notNull(),
    cfTunnelId: text("cf_tunnel_id").notNull(),
    tunnelName: text("tunnel_name").notNull(),
    tunnelToken: text("tunnel_token").notNull(),
    status: text("status").notNull().default("active"),
    lastHealthCheckAt: timestamp("last_health_check_at", { withTimezone: true }),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
    updatedAt: timestamp("updated_at", { withTimezone: true }).notNull().defaultNow()
  },
  (table) => [index("cloudflare_tunnels_node_id_idx").on(table.nodeId), index("cloudflare_tunnels_cf_tunnel_id_idx").on(table.cfTunnelId)]
);

export const cloudflareRoutes = pgTable(
  "cloudflare_routes",
  {
    id: text("id").primaryKey(),
    tunnelId: text("tunnel_id").notNull().references(() => cloudflareTunnels.id, { onDelete: "cascade" }),
    projectId: text("project_id").notNull().references(() => projects.id, { onDelete: "cascade" }),
    hostname: text("hostname").notNull(),
    serviceTarget: text("service_target").notNull(),
    noTlsVerify: boolean("no_tls_verify").notNull().default(false),
    enabled: boolean("enabled").notNull().default(true),
    cfDnsRecordId: text("cf_dns_record_id"),
    lastPublishedAt: timestamp("last_published_at", { withTimezone: true }),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
    updatedAt: timestamp("updated_at", { withTimezone: true }).notNull().defaultNow()
  },
  (table) => [
    index("cloudflare_routes_tunnel_id_idx").on(table.tunnelId),
    index("cloudflare_routes_project_id_idx").on(table.projectId),
    index("cloudflare_routes_hostname_idx").on(table.hostname)
  ]
);

export type ProjectRow = typeof projects.$inferSelect;
export type NewProjectRow = typeof projects.$inferInsert;
export type DeploymentNodeRow = typeof deploymentNodes.$inferSelect;
export type NewDeploymentNodeRow = typeof deploymentNodes.$inferInsert;
export type DeploymentRow = typeof deployments.$inferSelect;
export type NewDeploymentRow = typeof deployments.$inferInsert;
export type BackupRow = typeof backups.$inferSelect;
export type NewBackupRow = typeof backups.$inferInsert;
export type AuditLogRow = typeof auditLogs.$inferSelect;
export type NewAuditLogRow = typeof auditLogs.$inferInsert;
export type AppSettingRow = typeof appSettings.$inferSelect;
export type CloudflareTunnelRow = typeof cloudflareTunnels.$inferSelect;
export type NewCloudflareTunnelRow = typeof cloudflareTunnels.$inferInsert;
export type CloudflareRouteRow = typeof cloudflareRoutes.$inferSelect;
export type NewCloudflareRouteRow = typeof cloudflareRoutes.$inferInsert;
