import { integer, pgTable, text, timestamp } from "drizzle-orm/pg-core";

export const projects = pgTable("projects", {
  id: text("id").primaryKey(),
  name: text("name").notNull(),
  gitUrl: text("git_url"),
  branch: text("branch").notNull(),
  folderName: text("folder_name").notNull(),
  localPath: text("local_path").notNull(),
  composeFile: text("compose_file").notNull(),
  composeContent: text("compose_content"),
  deployToken: text("deploy_token").notNull(),
  sshPrivateKeyPath: text("ssh_private_key_path"),
  sshPublicKey: text("ssh_public_key"),
  createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
  updatedAt: timestamp("updated_at", { withTimezone: true }).notNull().defaultNow()
});

export const deployments = pgTable("deployments", {
  id: text("id").primaryKey(),
  projectId: text("project_id").notNull().references(() => projects.id, { onDelete: "cascade" }),
  status: text("status").notNull(),
  trigger: text("trigger").notNull(),
  logs: text("logs").notNull().default(""),
  exitCode: integer("exit_code"),
  startedAt: timestamp("started_at", { withTimezone: true }).notNull().defaultNow(),
  finishedAt: timestamp("finished_at", { withTimezone: true })
});

export type ProjectRow = typeof projects.$inferSelect;
export type NewProjectRow = typeof projects.$inferInsert;
export type DeploymentRow = typeof deployments.$inferSelect;
export type NewDeploymentRow = typeof deployments.$inferInsert;
