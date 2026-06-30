import { McpServer, ResourceTemplate } from "@modelcontextprotocol/sdk/server/mcp.js";
import type { CallToolResult, ToolAnnotations } from "@modelcontextprotocol/sdk/types.js";
import { z } from "zod";
import { config } from "../config.js";
import { db } from "../db/index.js";
import { projects } from "../db/schema.js";
import { HttpError } from "../http-utils.js";
import {
  backupInput,
  cloudflareAssignmentInput,
  cloudflareClientInput,
  cloudflareDnsRecordInput,
  cloudflareHostnameInput,
  cloudflareRouteInput,
  cloudflareSettingsInput,
  cloudflareTunnelInput,
  deploymentInput,
  envInput,
  envVariablesInput,
  frpSettingsInput,
  frpTunnelInput,
  frpTunnelUpdateInput,
  multiNodeSettingsInput,
  projectInput,
  r2SettingsInput,
  rollbackInput,
  setupWizardInput
} from "../route-schemas.js";
import { recordAuditLog } from "../services/audit.js";
import { listAuditLogs } from "../services/audit.js";
import { createPostgresBackup, deleteBackup, listBackups, listPostgresBackupTargets, uploadBackupToR2 } from "../services/backups.js";
import { readProjectCompose } from "../services/compose.js";
import {
  createClientDnsRecord,
  createCloudflareClient,
  createDnsRecord,
  createManagedHostname,
  createManagedTunnel,
  createTunnelAssignment,
  deleteClientDnsRecord,
  deleteCloudflareClient,
  deleteDnsRecord,
  deleteManagedHostname,
  deleteManagedTunnel,
  deleteProjectRoute,
  deleteTunnelAssignment,
  disableProjectRoute,
  enableProjectRoute,
  getCloudflaredStatus,
  getTunnelForNode,
  getTunnelHealth,
  listClientDnsRecords,
  listCloudflareClients,
  listCloudflareZones,
  listDnsRecords,
  listManagedHostnames,
  listPublicTunnels,
  listRouteDiagnostics,
  listRoutesForProject,
  listTunnelAssignments,
  publishProjectRoute,
  publicCloudflareSettings,
  publicTunnel,
  restartCloudflared,
  retryManagedHostname,
  saveCloudflareSettings,
  startCloudflared,
  stopCloudflared,
  updateClientDnsRecord,
  updateCloudflareClient,
  updateDnsRecord,
  validateCloudflareClient,
  validateCloudflareSettings
} from "../services/cloudflare.js";
import { cleanupDocker, containerLogs, listContainers, previewDockerCleanup, restartContainer, startContainer, stopContainer } from "../services/docker.js";
import { findDeployment, latestDeployments, previewRollbackForProject, rollbackTargetForProject, startDeployment } from "../services/deployments.js";
import type { PendingDeploymentEnv } from "../services/deployment-runner.js";
import { controlFrpServer, createFrpTunnel, deleteFrpTunnel, frpOverview, saveFrpSettings, updateFrpTunnel } from "../services/frp.js";
import { hasMcpAccess } from "../services/mcp-tokens.js";
import { listNodes } from "../services/nodes.js";
import { previewEnvContent, readProjectEnv, readProjectEnvVariables, writeProjectEnv, writeProjectEnvVariables } from "../services/project-env.js";
import { restartProjectCompose, stopProjectCompose } from "../services/project-runtime.js";
import { createProject, deleteProject, getProject, getProjectDeployToken, listProjectsWithContainerCounts, publicProject, updateProject } from "../services/projects.js";
import {
  ensureWorkerJoinToken,
  publicMultiNodeSettings,
  publicR2Settings,
  publicSetupWizardSettings,
  saveMultiNodeSettings,
  saveR2Settings,
  saveSetupWizardSettings
} from "../services/settings.js";
import { generateManagedSshPrivateKey, managedSshKeyStatus, saveManagedSshPrivateKey } from "../services/ssh.js";
import { healthStatus, systemUsage } from "../services/system.js";
import { logger } from "../logger.js";
import type { YantoMcpContext } from "./context.js";
import { asRecord, limitText, requireConfirm, safeTool, toolResult } from "./tool-utils.js";

const confirmInput = z.object({ confirm: z.boolean().optional() });
const idInput = z.object({ id: z.string().min(1) });
const limitInput = z.object({ limit: z.number().int().min(1).max(500).optional() });
const logsInput = z.object({ id: z.string().min(1), tailChars: z.number().int().min(1).max(200_000).optional() });
const projectIdInput = z.object({ projectId: z.string().min(1) });
const projectResourceInput = z.object({ projectId: z.string().min(1), envFile: z.string().min(1).optional() });
const auditInput = z.object({ limit: z.number().int().min(1).max(500).optional(), projectId: z.string().min(1).optional() });
const cloudflareClientIdInput = z.object({ clientId: z.string().min(1) });
const cloudflareClientDnsInput = cloudflareClientIdInput.extend(cloudflareDnsRecordInput.shape);
const cloudflareClientDnsUpdateInput = cloudflareClientDnsInput.extend({ id: z.string().min(1) });
const cloudflareClientDnsDeleteInput = z.object({ clientId: z.string().min(1), id: z.string().min(1), confirm: z.boolean().optional() });
const cloudflareNodeInput = z.object({ nodeId: z.string().min(1) });
const cloudflareProjectRouteInput = z.object({ projectId: z.string().min(1) }).extend(cloudflareRouteInput.shape);
const cloudflareDeleteTunnelInput = z.object({ id: z.string().min(1), force: z.boolean().optional(), confirm: z.boolean().optional() });
const frpServerInput = z.object({ confirm: z.boolean().optional() });
const sshKeyInput = z.object({ privateKey: z.string().min(1), confirm: z.boolean().optional() });
const settingsConfirmInput = z.object({ confirm: z.boolean().optional() });
const r2SettingsMcpInput = r2SettingsInput.extend({ confirm: z.boolean().optional() });
const cloudflareSettingsMcpInput = cloudflareSettingsInput.extend({ confirm: z.boolean().optional() });
const cloudflareClientMcpInput = cloudflareClientInput.extend({ confirm: z.boolean().optional() });
const cloudflareClientUpdateInput = cloudflareClientInput.partial().extend({ id: z.string().min(1), confirm: z.boolean().optional() });
const deleteInput = idInput.extend({ confirm: z.boolean().optional() });
const backupDeleteInput = deleteInput;
const rollbackExecuteInput = rollbackInput.extend({ projectId: z.string().min(1), confirm: z.boolean().optional() });
const rollbackPreviewInput = z.object({ projectId: z.string().min(1), targetRef: z.string().min(1) });
const deploymentMcpInput = deploymentInput.extend({ projectId: z.string().min(1) });
const projectUpdateInput = projectInput.partial().extend({ id: z.string().min(1) });
const envReadInput = projectResourceInput;
const envWriteTextInput = envInput.extend({ projectId: z.string().min(1) });
const envWriteVariablesInput = envVariablesInput.extend({ projectId: z.string().min(1) });
const dnsUpdateInput = cloudflareDnsRecordInput.extend({ id: z.string().min(1) });
const routeActionInput = idInput;
const routeDeleteInput = deleteInput;
const hostnameRetryInput = idInput;
const hostnameDeleteInput = deleteInput;
const assignmentListInput = z.object({ tunnelId: z.string().min(1).optional() });
const assignmentDeleteInput = deleteInput;
const tunnelRuntimeActionInput = z.object({ nodeId: z.string().min(1), confirm: z.boolean().optional() });
const frpTunnelUpdateMcpInput = frpTunnelUpdateInput.extend({ id: z.string().min(1) });

type ToolLevel = "read" | "write" | "admin";
type ToolHandler<T extends z.ZodTypeAny> = (input: z.infer<T>) => Promise<CallToolResult> | CallToolResult;

const readOnly: ToolAnnotations = { readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false };
const externalRead: ToolAnnotations = { ...readOnly, openWorldHint: true };
const write: ToolAnnotations = { readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false };
const externalWrite: ToolAnnotations = { ...write, openWorldHint: true };
const destructive: ToolAnnotations = { readOnlyHint: false, destructiveHint: true, idempotentHint: false, openWorldHint: false };
const externalDestructive: ToolAnnotations = { ...destructive, openWorldHint: true };

function summarize(name: string, value: unknown, key = "result") {
  const count = Array.isArray(value) ? ` (${value.length})` : "";
  return toolResult(`${name}${count}`, asRecord(value, key));
}

function registerTool<T extends z.ZodTypeAny>(
  server: McpServer,
  ctx: YantoMcpContext,
  level: ToolLevel,
  name: string,
  description: string,
  inputSchema: T,
  annotations: ToolAnnotations,
  handler: ToolHandler<T>
) {
  if (!hasMcpAccess(ctx.accessLevel, level)) {
    return;
  }
  const register = server.registerTool.bind(server) as any;
  register(
    name,
    {
      title: name,
      description,
      inputSchema,
      annotations
    },
    (args: any) => safeTool(() => handler(inputSchema.parse(args)))
  );
}

async function settingsSnapshot() {
  const [count, sshKey, r2, cf, setupWizard, multiNode] = await Promise.all([
    db.select().from(projects),
    managedSshKeyStatus(),
    publicR2Settings(),
    publicCloudflareSettings(),
    publicSetupWizardSettings(),
    publicMultiNodeSettings()
  ]);
  return {
    projectsRoot: config.projectsRoot,
    hostProjectsRoot: config.hostProjectsRoot,
    sshKeysDir: config.sshKeysDir,
    appBaseUrl: config.appBaseUrl,
    projectCount: count.length,
    sshKey,
    r2,
    cf,
    setupWizard,
    multiNode
  };
}

async function requireProject(projectId: string) {
  const project = await getProject(projectId);
  if (!project) {
    throw new HttpError(404, "Project not found.");
  }
  return project;
}

function pendingEnvFromDeploymentInput(input: z.infer<typeof deploymentInput>): PendingDeploymentEnv | undefined {
  if (input.envContent !== undefined) {
    return { mode: "text", content: input.envContent, envFile: input.envFile };
  }
  if (input.envVariables) {
    return { mode: "variables", variables: input.envVariables, envFile: input.envFile };
  }
  return undefined;
}

export function createYantoMcpServer(ctx: YantoMcpContext) {
  const server = new McpServer(
    {
      name: "yanto",
      version: "0.1.0"
    },
    {
      capabilities: {
        resources: {},
        tools: {}
      }
    }
  );

  registerTool(server, ctx, "read", "yanto_health", "Read Yanto health.", z.object({}), readOnly, async () => summarize("Health", await healthStatus(), "health"));
  registerTool(server, ctx, "read", "yanto_system_usage", "Read system usage.", z.object({}), readOnly, async () => summarize("System usage", await systemUsage(), "usage"));
  registerTool(server, ctx, "read", "yanto_system_logs", "Read recent system logs.", z.object({ tailChars: z.number().int().min(1).max(200_000).optional() }), readOnly, (input) => {
    const logs = limitText(logger.history() || "No system log entries recorded yet.", input.tailChars ?? 80_000);
    return toolResult("System logs", { logs });
  });
  registerTool(server, ctx, "read", "yanto_audit_list", "List audit entries.", auditInput, readOnly, async (input) =>
    summarize("Audit logs", await listAuditLogs(input.limit ?? 100, input.projectId), "auditLogs")
  );
  registerTool(server, ctx, "read", "yanto_cleanup_preview", "Preview Docker cleanup.", z.object({}), readOnly, async () =>
    summarize("Cleanup preview", { logs: await previewDockerCleanup() })
  );
  registerTool(server, ctx, "admin", "yanto_cleanup", "Run protected Docker cleanup. Requires confirm: true.", confirmInput, destructive, async (input) => {
    requireConfirm(input.confirm, "Docker cleanup");
    const logs = await cleanupDocker();
    await recordAuditLog({ actor: ctx.actor, action: "system.cleanup", entityType: "system", metadata: { protected: true } });
    return toolResult("Cleanup completed.", { logs });
  });

  registerTool(server, ctx, "read", "yanto_nodes_list", "List deployment nodes.", z.object({}), readOnly, async () => summarize("Nodes", await listNodes(), "nodes"));
  registerTool(server, ctx, "admin", "yanto_worker_join_command_reveal", "Reveal worker join token and install command. Requires confirm: true.", confirmInput, destructive, async (input) => {
    requireConfirm(input.confirm, "Worker join token reveal");
    const token = await ensureWorkerJoinToken();
    const command = `curl -fsSL https://raw.githubusercontent.com/kodin00/yanto/master/scripts/install.sh | sudo bash -s -- worker --master ${config.appBaseUrl} --join-token ${token}`;
    await recordAuditLog({ actor: ctx.actor, action: "node.join_token.view", entityType: "deployment_node" });
    return toolResult("Worker join command revealed.", { token, command });
  });

  registerTool(server, ctx, "read", "yanto_projects_list", "List projects.", z.object({}), readOnly, async () =>
    summarize("Projects", (await listProjectsWithContainerCounts()).map(publicProject), "projects")
  );
  registerTool(server, ctx, "read", "yanto_project_get", "Get one project.", idInput, readOnly, async (input) =>
    summarize("Project", publicProject(await requireProject(input.id)), "project")
  );
  registerTool(server, ctx, "write", "yanto_project_create", "Create a project.", projectInput, write, async (input) => {
    const project = await createProject(input);
    await recordAuditLog({ actor: ctx.actor, action: "project.create", entityType: "project", entityId: project.id, projectId: project.id });
    return summarize("Project created", publicProject(project), "project");
  });
  registerTool(server, ctx, "write", "yanto_project_update", "Update a project.", projectUpdateInput, write, async (input) => {
    const { id, ...patch } = input;
    const project = await updateProject(id, patch);
    if (!project) throw new HttpError(404, "Project not found.");
    await recordAuditLog({ actor: ctx.actor, action: "project.update", entityType: "project", entityId: project.id, projectId: project.id });
    return summarize("Project updated", publicProject(project), "project");
  });
  registerTool(server, ctx, "admin", "yanto_project_delete", "Delete a project and its directory. Requires confirm: true.", deleteInput, destructive, async (input) => {
    requireConfirm(input.confirm, "Project deletion");
    await deleteProject(input.id);
    await recordAuditLog({ actor: ctx.actor, action: "project.delete", entityType: "project", entityId: input.id });
    return toolResult("Project deleted.", { ok: true, id: input.id });
  });
  registerTool(server, ctx, "read", "yanto_project_compose_read", "Read project compose content.", projectIdInput, readOnly, async (input) => {
    const project = await requireProject(input.projectId);
    return summarize("Compose content", await readProjectCompose(project));
  });
  registerTool(server, ctx, "read", "yanto_project_env_read", "Read project environment variables with values masked.", envReadInput, readOnly, async (input) => {
    const project = await requireProject(input.projectId);
    return summarize("Masked environment", await readProjectEnvVariables(project, input.envFile), "variables");
  });
  registerTool(server, ctx, "read", "yanto_project_env_content_read", "Read project environment file content with values masked.", envReadInput, readOnly, async (input) => {
    const project = await requireProject(input.projectId);
    return summarize("Masked environment content", await readProjectEnv(project, input.envFile));
  });
  registerTool(server, ctx, "read", "yanto_project_env_preview", "Preview an environment file parse.", envInput, readOnly, (input) =>
    summarize("Environment preview", previewEnvContent(input.content, input.envFile ?? ".env"))
  );
  registerTool(server, ctx, "write", "yanto_project_env_write", "Write full project environment content.", envWriteTextInput, write, async (input) => {
    const project = await requireProject(input.projectId);
    const preview = await writeProjectEnv(project, input.content, input.envFile);
    await recordAuditLog({ actor: ctx.actor, action: "project.env.write", entityType: "project", entityId: project.id, projectId: project.id, metadata: { envFile: preview.envFile, entryCount: preview.entryCount } });
    return summarize("Environment written", preview);
  });
  registerTool(server, ctx, "write", "yanto_project_env_variables_write", "Patch project environment variables; masked values are preserved.", envWriteVariablesInput, write, async (input) => {
    const project = await requireProject(input.projectId);
    const preview = await writeProjectEnvVariables(project, input.variables, input.envFile);
    await recordAuditLog({ actor: ctx.actor, action: "project.env.write", entityType: "project", entityId: project.id, projectId: project.id, metadata: { envFile: preview.envFile, entryCount: preview.entryCount } });
    return summarize("Environment variables written", preview);
  });
  registerTool(server, ctx, "admin", "yanto_project_deploy_token_reveal", "Reveal a project deploy token. Requires confirm: true.", deleteInput, destructive, async (input) => {
    requireConfirm(input.confirm, "Project deploy token reveal");
    const deployToken = await getProjectDeployToken(input.id);
    if (!deployToken) throw new HttpError(404, "Project not found.");
    await recordAuditLog({ actor: ctx.actor, action: "project.deploy_token.reveal", entityType: "project", entityId: input.id, projectId: input.id });
    return toolResult("Project deploy token revealed.", { deployToken });
  });
  registerTool(server, ctx, "write", "yanto_project_deploy", "Start a deployment and return promptly with deployment ID.", deploymentMcpInput, externalWrite, async (input) => {
    const project = await requireProject(input.projectId);
    if (!project.manualDeployEnabled) {
      throw new HttpError(403, "Manual deployments are disabled for this project.");
    }
    const result = await startDeployment(input.projectId, "manual", { targetRef: input.targetRef, pendingEnv: pendingEnvFromDeploymentInput(input) });
    await recordAuditLog({ actor: ctx.actor, action: "deployment.start", entityType: "deployment", entityId: result.deployment.id, projectId: input.projectId, metadata: { trigger: "manual", targetRef: input.targetRef ?? null, reused: result.reused } });
    return toolResult(result.reused ? "Deployment already running." : "Deployment started.", { deploymentId: result.deployment.id, deployment: result.deployment, reused: result.reused });
  });
  registerTool(server, ctx, "read", "yanto_project_rollback_preview", "Preview a rollback target.", rollbackPreviewInput, externalRead, async (input) =>
    summarize("Rollback preview", await previewRollbackForProject(input.projectId, input.targetRef), "preview")
  );
  registerTool(server, ctx, "admin", "yanto_project_rollback", "Execute rollback deployment. Requires confirm: true.", rollbackExecuteInput, externalDestructive, async (input) => {
    requireConfirm(input.confirm, "Rollback");
    const target = await rollbackTargetForProject(input.projectId, input.deploymentId, input.targetRef);
    const result = await startDeployment(input.projectId, "rollback", { targetRef: target.targetRef, rollbackFromDeploymentId: target.rollbackFromDeploymentId ?? undefined });
    await recordAuditLog({ actor: ctx.actor, action: "deployment.rollback", entityType: "deployment", entityId: result.deployment.id, projectId: input.projectId, metadata: { targetRef: target.targetRef, rollbackFromDeploymentId: target.rollbackFromDeploymentId, reused: result.reused } });
    return toolResult(result.reused ? "Rollback deployment already running." : "Rollback deployment started.", { deploymentId: result.deployment.id, deployment: result.deployment, reused: result.reused });
  });
  registerTool(server, ctx, "admin", "yanto_project_stop", "Stop a local project compose stack. Requires confirm: true.", deleteInput, destructive, async (input) => {
    requireConfirm(input.confirm, "Project stop");
    const project = await requireProject(input.id);
    if (project.targetNodeId !== config.localNodeId) throw new HttpError(400, "Stopping projects on worker nodes is not supported yet.");
    const logs = await stopProjectCompose(project);
    await recordAuditLog({ actor: ctx.actor, action: "project.compose.stop", entityType: "project", entityId: project.id, projectId: project.id });
    return toolResult("Project stopped.", { logs });
  });
  registerTool(server, ctx, "admin", "yanto_project_restart", "Restart a local project compose stack. Requires confirm: true.", deleteInput, destructive, async (input) => {
    requireConfirm(input.confirm, "Project restart");
    const project = await requireProject(input.id);
    if (project.targetNodeId !== config.localNodeId) throw new HttpError(400, "Restarting projects on worker nodes is not supported yet.");
    const logs = await restartProjectCompose(project);
    await recordAuditLog({ actor: ctx.actor, action: "project.compose.restart", entityType: "project", entityId: project.id, projectId: project.id });
    return toolResult("Project restarted.", { logs });
  });

  registerTool(server, ctx, "read", "yanto_deployments_list", "List deployments.", limitInput, readOnly, async (input) =>
    summarize("Deployments", await latestDeployments(input.limit ?? 500), "deployments")
  );
  registerTool(server, ctx, "read", "yanto_deployment_get", "Get deployment.", idInput, readOnly, async (input) => {
    const deployment = await findDeployment(input.id);
    if (!deployment) throw new HttpError(404, "Deployment not found.");
    return summarize("Deployment", deployment, "deployment");
  });
  registerTool(server, ctx, "read", "yanto_deployment_logs", "Read deployment logs.", logsInput, readOnly, async (input) => {
    const deployment = await findDeployment(input.id);
    if (!deployment) throw new HttpError(404, "Deployment not found.");
    return toolResult("Deployment logs", { logs: limitText(deployment.logs, input.tailChars ?? 80_000), status: deployment.status });
  });

  registerTool(server, ctx, "read", "yanto_containers_list", "List Docker containers.", z.object({}), readOnly, async () => summarize("Containers", await listContainers(), "containers"));
  registerTool(server, ctx, "read", "yanto_container_logs", "Read container logs.", logsInput, readOnly, async (input) =>
    toolResult("Container logs", { logs: limitText(await containerLogs(input.id), input.tailChars ?? 80_000) })
  );
  registerTool(server, ctx, "write", "yanto_container_start", "Start a container.", idInput, write, async (input) => {
    await startContainer(input.id);
    await recordAuditLog({ actor: ctx.actor, action: "container.start", entityType: "container", entityId: input.id });
    return toolResult("Container started.", { ok: true });
  });
  registerTool(server, ctx, "admin", "yanto_container_stop", "Stop a container. Requires confirm: true.", deleteInput, destructive, async (input) => {
    requireConfirm(input.confirm, "Container stop");
    await stopContainer(input.id);
    await recordAuditLog({ actor: ctx.actor, action: "container.stop", entityType: "container", entityId: input.id });
    return toolResult("Container stopped.", { ok: true });
  });
  registerTool(server, ctx, "admin", "yanto_container_restart", "Restart a container. Requires confirm: true.", deleteInput, destructive, async (input) => {
    requireConfirm(input.confirm, "Container restart");
    await restartContainer(input.id);
    await recordAuditLog({ actor: ctx.actor, action: "container.restart", entityType: "container", entityId: input.id });
    return toolResult("Container restarted.", { ok: true });
  });

  registerTool(server, ctx, "read", "yanto_backups_list", "List backups.", z.object({ limit: z.number().int().min(1).max(200).optional() }), readOnly, async (input) =>
    summarize("Backups", await listBackups(input.limit ?? 50), "backups")
  );
  registerTool(server, ctx, "read", "yanto_backups_postgres_targets_list", "List Postgres backup targets.", z.object({}), readOnly, async () =>
    summarize("Postgres backup targets", await listPostgresBackupTargets(), "targets")
  );
  registerTool(server, ctx, "write", "yanto_backup_create", "Create a Postgres backup.", backupInput, write, async (input) => {
    const backup = await createPostgresBackup(input.containerId);
    await recordAuditLog({ actor: ctx.actor, action: "backup.create", entityType: "backup", entityId: backup.id, projectId: backup.projectId, metadata: { kind: backup.kind, status: backup.status, fileSizeBytes: backup.fileSizeBytes, note: backup.note } });
    return summarize("Backup created", backup, "backup");
  });
  registerTool(server, ctx, "write", "yanto_backup_upload_r2", "Upload a backup to R2.", idInput, externalWrite, async (input) => {
    const { backup, result } = await uploadBackupToR2(input.id);
    await recordAuditLog({ actor: ctx.actor, action: "backup.r2_upload", entityType: "backup", entityId: backup.id, projectId: backup.projectId, metadata: { bucket: result.bucket, key: result.key, size: result.size } });
    return summarize("Backup uploaded to R2", result);
  });
  registerTool(server, ctx, "admin", "yanto_backup_delete", "Delete a backup. Requires confirm: true.", backupDeleteInput, destructive, async (input) => {
    requireConfirm(input.confirm, "Backup deletion");
    const backup = await deleteBackup(input.id);
    if (!backup) throw new HttpError(404, "Backup not found.");
    await recordAuditLog({ actor: ctx.actor, action: "backup.delete", entityType: "backup", entityId: backup.id, projectId: backup.projectId, metadata: { filename: backup.filename } });
    return toolResult("Backup deleted.", { ok: true, id: input.id });
  });

  registerTool(server, ctx, "read", "yanto_settings_read", "Read public settings.", z.object({}), readOnly, async () => summarize("Settings", await settingsSnapshot(), "settings"));
  registerTool(server, ctx, "admin", "yanto_settings_r2_update", "Update R2 settings; credentials are write-only. Requires confirm: true.", r2SettingsMcpInput, externalWrite, async (input) => {
    requireConfirm(input.confirm, "R2 settings update");
    const r2 = await saveR2Settings(input);
    await recordAuditLog({ actor: ctx.actor, action: "settings.r2.save", entityType: "settings", metadata: { enabled: r2.enabled, bucket: r2.bucket, prefix: r2.prefix } });
    return summarize("R2 settings saved", r2, "r2");
  });
  registerTool(server, ctx, "admin", "yanto_settings_cloudflare_update", "Update legacy Cloudflare settings; API token is write-only. Requires confirm: true.", cloudflareSettingsMcpInput, externalWrite, async (input) => {
    requireConfirm(input.confirm, "Cloudflare settings update");
    const cf = await saveCloudflareSettings(input);
    await recordAuditLog({ actor: ctx.actor, action: "settings.cloudflare.save", entityType: "settings", metadata: { accountId: cf.accountId, zoneId: cf.zoneId } });
    return summarize("Cloudflare settings saved", cf, "cf");
  });
  registerTool(server, ctx, "admin", "yanto_settings_cloudflare_validate", "Validate legacy Cloudflare credentials.", cloudflareSettingsMcpInput, externalRead, async (input) => {
    requireConfirm(input.confirm, "Cloudflare credentials validation");
    const result = await validateCloudflareSettings(input);
    await recordAuditLog({ actor: ctx.actor, action: "settings.cloudflare.validate", entityType: "settings" });
    return summarize("Cloudflare credentials valid", result);
  });
  registerTool(server, ctx, "admin", "yanto_settings_ssh_key_save", "Save managed SSH private key; key is write-only. Requires confirm: true.", sshKeyInput, destructive, async (input) => {
    requireConfirm(input.confirm, "SSH key save");
    const sshKey = await saveManagedSshPrivateKey(input.privateKey);
    await recordAuditLog({ actor: ctx.actor, action: "settings.ssh_key.save", entityType: "settings" });
    return summarize("SSH key saved", sshKey, "sshKey");
  });
  registerTool(server, ctx, "admin", "yanto_settings_ssh_key_generate", "Generate managed SSH key. Requires confirm: true.", settingsConfirmInput, destructive, async (input) => {
    requireConfirm(input.confirm, "SSH key generation");
    const sshKey = await generateManagedSshPrivateKey();
    await recordAuditLog({ actor: ctx.actor, action: "settings.ssh_key.generate", entityType: "settings" });
    return summarize("SSH key generated", sshKey, "sshKey");
  });
  registerTool(server, ctx, "write", "yanto_settings_setup_state_update", "Update setup wizard state.", setupWizardInput, write, async (input) => {
    const setupWizard = await saveSetupWizardSettings(input.action);
    await recordAuditLog({ actor: ctx.actor, action: `settings.setup_wizard.${input.action}`, entityType: "settings" });
    return summarize("Setup wizard state saved", setupWizard, "setupWizard");
  });
  registerTool(server, ctx, "write", "yanto_settings_multi_node_update", "Update multi-node setting.", multiNodeSettingsInput, write, async (input) => {
    const multiNode = await saveMultiNodeSettings(input);
    await recordAuditLog({ actor: ctx.actor, action: "settings.multi_node.save", entityType: "settings", metadata: { enabled: multiNode.enabled, releaseStage: multiNode.releaseStage } });
    return summarize("Multi-node settings saved", multiNode, "multiNode");
  });

  registerTool(server, ctx, "read", "yanto_cloudflare_clients_list", "List Cloudflare clients.", z.object({}), readOnly, async () => summarize("Cloudflare clients", await listCloudflareClients(), "clients"));
  registerTool(server, ctx, "admin", "yanto_cloudflare_client_validate", "Validate a Cloudflare client token. Requires confirm: true.", cloudflareClientMcpInput, externalRead, async (input) => {
    requireConfirm(input.confirm, "Cloudflare client validation");
    return summarize("Cloudflare client valid", await validateCloudflareClient({ accountId: input.accountId, zoneId: input.zoneId, apiToken: input.apiToken ?? "" }));
  });
  registerTool(server, ctx, "admin", "yanto_cloudflare_client_create", "Create a Cloudflare client; token is write-only. Requires confirm: true.", cloudflareClientMcpInput, externalWrite, async (input) => {
    requireConfirm(input.confirm, "Cloudflare client creation");
    if (!input.apiToken) throw new HttpError(400, "API token is required.");
    const client = await createCloudflareClient({ ...input, apiToken: input.apiToken });
    await recordAuditLog({ actor: ctx.actor, action: "cloudflare.client.create", entityType: "cloudflare_client", entityId: client.id });
    return summarize("Cloudflare client created", client, "client");
  });
  registerTool(server, ctx, "admin", "yanto_cloudflare_client_update", "Update a Cloudflare client; token is write-only. Requires confirm: true.", cloudflareClientUpdateInput, externalWrite, async (input) => {
    requireConfirm(input.confirm, "Cloudflare client update");
    const { id, confirm, ...patch } = input;
    void confirm;
    const client = await updateCloudflareClient(id, patch);
    await recordAuditLog({ actor: ctx.actor, action: "cloudflare.client.update", entityType: "cloudflare_client", entityId: client.id });
    return summarize("Cloudflare client updated", client, "client");
  });
  registerTool(server, ctx, "admin", "yanto_cloudflare_client_delete", "Delete a Cloudflare client. Requires confirm: true.", deleteInput, externalDestructive, async (input) => {
    requireConfirm(input.confirm, "Cloudflare client deletion");
    await deleteCloudflareClient(input.id);
    await recordAuditLog({ actor: ctx.actor, action: "cloudflare.client.delete", entityType: "cloudflare_client", entityId: input.id });
    return toolResult("Cloudflare client deleted.", { ok: true, id: input.id });
  });
  registerTool(server, ctx, "read", "yanto_cloudflare_zones_list", "List zones for a Cloudflare client.", cloudflareClientIdInput, externalRead, async (input) =>
    summarize("Cloudflare zones", await listCloudflareZones(input.clientId), "zones")
  );
  registerTool(server, ctx, "read", "yanto_cloudflare_tunnels_list", "List managed Cloudflare tunnels.", z.object({}), readOnly, async () => summarize("Cloudflare tunnels", await listPublicTunnels(), "tunnels"));
  registerTool(server, ctx, "write", "yanto_cloudflare_tunnel_create", "Create a managed Cloudflare tunnel.", cloudflareTunnelInput, externalWrite, async (input) => {
    const tunnel = await createManagedTunnel(input);
    await recordAuditLog({ actor: ctx.actor, action: "cloudflare.tunnel.create", entityType: "cloudflare_tunnel", entityId: tunnel.id });
    return summarize("Cloudflare tunnel created", publicTunnel(tunnel), "tunnel");
  });
  registerTool(server, ctx, "admin", "yanto_cloudflare_tunnel_delete", "Delete or force-delete a managed tunnel. Requires confirm: true.", cloudflareDeleteTunnelInput, externalDestructive, async (input) => {
    requireConfirm(input.confirm, input.force ? "Cloudflare tunnel force deletion" : "Cloudflare tunnel deletion");
    await deleteManagedTunnel(input.id, input.force ?? false);
    await recordAuditLog({ actor: ctx.actor, action: input.force ? "cloudflare.tunnel.force_delete" : "cloudflare.tunnel.delete", entityType: "cloudflare_tunnel", entityId: input.id });
    return toolResult("Cloudflare tunnel deleted.", { ok: true, id: input.id, force: input.force ?? false });
  });
  registerTool(server, ctx, "read", "yanto_cloudflare_assignments_list", "List Cloudflare tunnel assignments.", assignmentListInput, readOnly, async (input) =>
    summarize("Cloudflare assignments", await listTunnelAssignments(input.tunnelId), "assignments")
  );
  registerTool(server, ctx, "write", "yanto_cloudflare_assignment_create", "Create a Cloudflare tunnel assignment.", cloudflareAssignmentInput, write, async (input) => {
    const assignment = await createTunnelAssignment(input);
    await recordAuditLog({ actor: ctx.actor, action: "cloudflare.assignment.create", entityType: "cloudflare_assignment", entityId: assignment.id });
    return summarize("Cloudflare assignment created", assignment, "assignment");
  });
  registerTool(server, ctx, "admin", "yanto_cloudflare_assignment_delete", "Delete a Cloudflare tunnel assignment. Requires confirm: true.", assignmentDeleteInput, destructive, async (input) => {
    requireConfirm(input.confirm, "Cloudflare assignment deletion");
    await deleteTunnelAssignment(input.id);
    await recordAuditLog({ actor: ctx.actor, action: "cloudflare.assignment.delete", entityType: "cloudflare_assignment", entityId: input.id });
    return toolResult("Cloudflare assignment deleted.", { ok: true, id: input.id });
  });
  registerTool(server, ctx, "read", "yanto_cloudflare_hostnames_list", "List managed hostnames.", z.object({}), readOnly, async () => summarize("Managed hostnames", await listManagedHostnames(), "hostnames"));
  registerTool(server, ctx, "write", "yanto_cloudflare_hostname_create", "Create a managed hostname route.", cloudflareHostnameInput, externalWrite, async (input) => {
    const hostname = await createManagedHostname(input);
    await recordAuditLog({ actor: ctx.actor, action: "cloudflare.hostname.create", entityType: "cloudflare_route", entityId: hostname.id, projectId: hostname.projectId });
    return summarize("Managed hostname created", hostname, "hostname");
  });
  registerTool(server, ctx, "write", "yanto_cloudflare_hostname_retry", "Retry a managed hostname sync.", hostnameRetryInput, externalWrite, async (input) => {
    const hostname = await retryManagedHostname(input.id);
    await recordAuditLog({ actor: ctx.actor, action: "cloudflare.hostname.retry", entityType: "cloudflare_route", entityId: hostname.id, projectId: hostname.projectId });
    return summarize("Managed hostname retried", hostname, "hostname");
  });
  registerTool(server, ctx, "admin", "yanto_cloudflare_hostname_delete", "Delete a managed hostname. Requires confirm: true.", hostnameDeleteInput, externalDestructive, async (input) => {
    requireConfirm(input.confirm, "Managed hostname deletion");
    const result = await deleteManagedHostname(input.id);
    await recordAuditLog({ actor: ctx.actor, action: "cloudflare.hostname.delete", entityType: "cloudflare_route", entityId: input.id, metadata: result.warnings.length ? { warnings: result.warnings } : undefined });
    return toolResult("Managed hostname deleted.", { ok: true, warnings: result.warnings });
  });
  registerTool(server, ctx, "read", "yanto_cloudflare_dns_list", "List legacy Cloudflare DNS records.", z.object({}), externalRead, async () => summarize("DNS records", await listDnsRecords(), "records"));
  registerTool(server, ctx, "write", "yanto_cloudflare_dns_create", "Create legacy Cloudflare DNS record.", cloudflareDnsRecordInput, externalWrite, async (input) => {
    const record = await createDnsRecord(input);
    await recordAuditLog({ actor: ctx.actor, action: "cloudflare.dns.create", entityType: "cloudflare_dns_record", entityId: record.id, metadata: { type: record.type, name: record.name } });
    return summarize("DNS record created", record, "record");
  });
  registerTool(server, ctx, "write", "yanto_cloudflare_dns_update", "Update legacy Cloudflare DNS record.", dnsUpdateInput, externalWrite, async (input) => {
    const { id, ...body } = input;
    const record = await updateDnsRecord(id, body);
    await recordAuditLog({ actor: ctx.actor, action: "cloudflare.dns.update", entityType: "cloudflare_dns_record", entityId: record.id, metadata: { type: record.type, name: record.name } });
    return summarize("DNS record updated", record, "record");
  });
  registerTool(server, ctx, "admin", "yanto_cloudflare_dns_delete", "Delete legacy Cloudflare DNS record. Requires confirm: true.", deleteInput, externalDestructive, async (input) => {
    requireConfirm(input.confirm, "DNS record deletion");
    await deleteDnsRecord(input.id);
    await recordAuditLog({ actor: ctx.actor, action: "cloudflare.dns.delete", entityType: "cloudflare_dns_record", entityId: input.id });
    return toolResult("DNS record deleted.", { ok: true, id: input.id });
  });
  registerTool(server, ctx, "read", "yanto_cloudflare_client_dns_list", "List DNS records for a Cloudflare client.", cloudflareClientIdInput, externalRead, async (input) =>
    summarize("Client DNS records", await listClientDnsRecords(input.clientId), "records")
  );
  registerTool(server, ctx, "write", "yanto_cloudflare_client_dns_create", "Create DNS record for a Cloudflare client.", cloudflareClientDnsInput, externalWrite, async (input) => {
    const { clientId, ...body } = input;
    const record = await createClientDnsRecord(clientId, body);
    await recordAuditLog({ actor: ctx.actor, action: "cloudflare.client_dns.create", entityType: "cloudflare_dns_record", entityId: record.id, metadata: { clientId, type: record.type, name: record.name } });
    return summarize("Client DNS record created", record, "record");
  });
  registerTool(server, ctx, "write", "yanto_cloudflare_client_dns_update", "Update DNS record for a Cloudflare client.", cloudflareClientDnsUpdateInput, externalWrite, async (input) => {
    const { clientId, id, ...body } = input;
    const record = await updateClientDnsRecord(clientId, id, body);
    await recordAuditLog({ actor: ctx.actor, action: "cloudflare.client_dns.update", entityType: "cloudflare_dns_record", entityId: record.id, metadata: { clientId, type: record.type, name: record.name } });
    return summarize("Client DNS record updated", record, "record");
  });
  registerTool(server, ctx, "admin", "yanto_cloudflare_client_dns_delete", "Delete DNS record for a Cloudflare client. Requires confirm: true.", cloudflareClientDnsDeleteInput, externalDestructive, async (input) => {
    requireConfirm(input.confirm, "Client DNS record deletion");
    await deleteClientDnsRecord(input.clientId, input.id);
    await recordAuditLog({ actor: ctx.actor, action: "cloudflare.client_dns.delete", entityType: "cloudflare_dns_record", entityId: input.id, metadata: { clientId: input.clientId } });
    return toolResult("Client DNS record deleted.", { ok: true, id: input.id });
  });
  registerTool(server, ctx, "read", "yanto_cloudflare_routes_diagnostics", "List Cloudflare route diagnostics.", z.object({}), externalRead, async () =>
    summarize("Route diagnostics", await listRouteDiagnostics(), "diagnostics")
  );
  registerTool(server, ctx, "read", "yanto_cloudflare_tunnel_node_status", "Read tunnel runtime and health for a node.", cloudflareNodeInput, externalRead, async (input) => {
    const tunnel = await getTunnelForNode(input.nodeId);
    if (!tunnel) throw new HttpError(404, "No tunnel found.");
    const [runtime, health] = await Promise.all([getCloudflaredStatus(input.nodeId), getTunnelHealth(tunnel)]);
    return summarize("Cloudflare tunnel node status", { tunnel: publicTunnel(tunnel), runtime, health });
  });
  registerTool(server, ctx, "write", "yanto_cloudflare_tunnel_start", "Start cloudflared for a node.", cloudflareNodeInput, externalWrite, async (input) => {
    const tunnel = await getTunnelForNode(input.nodeId);
    if (!tunnel) throw new HttpError(404, "No tunnel found.");
    await startCloudflared(tunnel);
    await recordAuditLog({ actor: ctx.actor, action: "cloudflare.tunnel.start", entityType: "cloudflare_tunnel", entityId: tunnel.id, metadata: { nodeId: input.nodeId } });
    return toolResult("Cloudflared started.", { ok: true });
  });
  registerTool(server, ctx, "admin", "yanto_cloudflare_tunnel_stop", "Stop cloudflared for a node. Requires confirm: true.", tunnelRuntimeActionInput, externalDestructive, async (input) => {
    requireConfirm(input.confirm, "Cloudflared stop");
    await stopCloudflared(input.nodeId);
    await recordAuditLog({ actor: ctx.actor, action: "cloudflare.tunnel.stop", entityType: "cloudflare_tunnel", metadata: { nodeId: input.nodeId } });
    return toolResult("Cloudflared stopped.", { ok: true });
  });
  registerTool(server, ctx, "admin", "yanto_cloudflare_tunnel_restart", "Restart cloudflared for a node. Requires confirm: true.", tunnelRuntimeActionInput, externalDestructive, async (input) => {
    requireConfirm(input.confirm, "Cloudflared restart");
    await restartCloudflared(input.nodeId);
    await recordAuditLog({ actor: ctx.actor, action: "cloudflare.tunnel.restart", entityType: "cloudflare_tunnel", metadata: { nodeId: input.nodeId } });
    return toolResult("Cloudflared restarted.", { ok: true });
  });
  registerTool(server, ctx, "read", "yanto_project_cloudflare_routes_list", "List Cloudflare routes for a project.", projectIdInput, readOnly, async (input) =>
    summarize("Project Cloudflare routes", await listRoutesForProject(input.projectId), "routes")
  );
  registerTool(server, ctx, "write", "yanto_project_cloudflare_route_publish", "Publish a Cloudflare route for a project.", cloudflareProjectRouteInput, externalWrite, async (input) => {
    const route = await publishProjectRoute(input.projectId, input.hostname, input.serviceTarget, input.noTlsVerify, input.nodeId);
    await recordAuditLog({ actor: ctx.actor, action: "cloudflare.route.publish", entityType: "cloudflare_route", entityId: route.id, projectId: input.projectId, metadata: { hostname: route.hostname, serviceTarget: route.serviceTarget, noTlsVerify: route.noTlsVerify } });
    return summarize("Project route published", route, "route");
  });
  registerTool(server, ctx, "write", "yanto_cloudflare_route_enable", "Enable a Cloudflare project route.", routeActionInput, externalWrite, async (input) => {
    const route = await enableProjectRoute(input.id);
    await recordAuditLog({ actor: ctx.actor, action: "cloudflare.route.enable", entityType: "cloudflare_route", entityId: route.id, projectId: route.projectId });
    return summarize("Route enabled", route, "route");
  });
  registerTool(server, ctx, "write", "yanto_cloudflare_route_disable", "Disable a Cloudflare project route.", routeActionInput, externalWrite, async (input) => {
    const route = await disableProjectRoute(input.id);
    await recordAuditLog({ actor: ctx.actor, action: "cloudflare.route.disable", entityType: "cloudflare_route", entityId: route.id, projectId: route.projectId });
    return summarize("Route disabled", route, "route");
  });
  registerTool(server, ctx, "admin", "yanto_cloudflare_route_delete", "Delete a Cloudflare project route. Requires confirm: true.", routeDeleteInput, externalDestructive, async (input) => {
    requireConfirm(input.confirm, "Cloudflare route deletion");
    await deleteProjectRoute(input.id);
    await recordAuditLog({ actor: ctx.actor, action: "cloudflare.route.delete", entityType: "cloudflare_route", entityId: input.id });
    return toolResult("Route deleted.", { ok: true, id: input.id });
  });

  registerTool(server, ctx, "read", "yanto_frp_overview", "Read FRP overview.", z.object({}), readOnly, async () => summarize("FRP overview", await frpOverview(), "overview"));
  registerTool(server, ctx, "write", "yanto_frp_settings_update", "Update FRP settings.", frpSettingsInput, write, async (input) => {
    const settings = await saveFrpSettings(input.publicHost);
    await recordAuditLog({ actor: ctx.actor, action: "frp.settings.save", entityType: "frp", metadata: { publicHost: settings.publicHost } });
    return summarize("FRP settings saved", settings, "settings");
  });
  registerTool(server, ctx, "write", "yanto_frp_server_start", "Start FRP server.", z.object({}), write, async () => {
    const serverState = await controlFrpServer("start");
    await recordAuditLog({ actor: ctx.actor, action: "frp.server.start", entityType: "frp_server" });
    return summarize("FRP server started", serverState, "server");
  });
  registerTool(server, ctx, "admin", "yanto_frp_server_stop", "Stop FRP server. Requires confirm: true.", frpServerInput, destructive, async (input) => {
    requireConfirm(input.confirm, "FRP server stop");
    const serverState = await controlFrpServer("stop");
    await recordAuditLog({ actor: ctx.actor, action: "frp.server.stop", entityType: "frp_server" });
    return summarize("FRP server stopped", serverState, "server");
  });
  registerTool(server, ctx, "admin", "yanto_frp_server_restart", "Restart FRP server. Requires confirm: true.", frpServerInput, destructive, async (input) => {
    requireConfirm(input.confirm, "FRP server restart");
    const serverState = await controlFrpServer("restart");
    await recordAuditLog({ actor: ctx.actor, action: "frp.server.restart", entityType: "frp_server" });
    return summarize("FRP server restarted", serverState, "server");
  });
  registerTool(server, ctx, "write", "yanto_frp_tunnel_create", "Create FRP tunnel.", frpTunnelInput, write, async (input) => {
    const tunnel = await createFrpTunnel(input);
    await recordAuditLog({ actor: ctx.actor, action: "frp.tunnel.create", entityType: "frp_tunnel", entityId: tunnel.id, metadata: { protocol: tunnel.protocol, remotePort: tunnel.remotePort } });
    return summarize("FRP tunnel created", tunnel, "tunnel");
  });
  registerTool(server, ctx, "write", "yanto_frp_tunnel_update", "Update FRP tunnel.", frpTunnelUpdateMcpInput, write, async (input) => {
    const { id, ...patch } = input;
    const tunnel = await updateFrpTunnel(id, patch);
    await recordAuditLog({ actor: ctx.actor, action: "frp.tunnel.update", entityType: "frp_tunnel", entityId: tunnel.id, metadata: { enabled: tunnel.enabled, remotePort: tunnel.remotePort } });
    return summarize("FRP tunnel updated", tunnel, "tunnel");
  });
  registerTool(server, ctx, "admin", "yanto_frp_tunnel_delete", "Delete FRP tunnel. Requires confirm: true.", deleteInput, destructive, async (input) => {
    requireConfirm(input.confirm, "FRP tunnel deletion");
    const tunnel = await deleteFrpTunnel(input.id);
    await recordAuditLog({ actor: ctx.actor, action: "frp.tunnel.delete", entityType: "frp_tunnel", entityId: tunnel.id });
    return toolResult("FRP tunnel deleted.", { ok: true, id: input.id });
  });

  if (hasMcpAccess(ctx.accessLevel, "read")) {
    server.registerResource("yanto_overview", "yanto://overview", { title: "Yanto overview", mimeType: "application/json" }, async (uri) => ({
      contents: [{ uri: uri.href, mimeType: "application/json", text: JSON.stringify(await settingsSnapshot(), null, 2) }]
    }));
    server.registerResource("yanto_project_compose", new ResourceTemplate("yanto://projects/{id}/compose", { list: undefined }), { title: "Project compose", mimeType: "text/yaml" }, async (uri, variables) => {
      const project = await requireProject(String(variables.id));
      const compose = await readProjectCompose(project);
      return { contents: [{ uri: uri.href, mimeType: "text/yaml", text: compose.content }] };
    });
    server.registerResource("yanto_project_env", new ResourceTemplate("yanto://projects/{id}/env", { list: undefined }), { title: "Project masked environment", mimeType: "text/plain" }, async (uri, variables) => {
      const project = await requireProject(String(variables.id));
      const env = await readProjectEnv(project);
      return { contents: [{ uri: uri.href, mimeType: "text/plain", text: env.content }] };
    });
    server.registerResource("yanto_deployment_logs", new ResourceTemplate("yanto://deployments/{id}/logs", { list: undefined }), { title: "Deployment logs", mimeType: "text/plain" }, async (uri, variables) => {
      const deployment = await findDeployment(String(variables.id));
      if (!deployment) throw new HttpError(404, "Deployment not found.");
      return { contents: [{ uri: uri.href, mimeType: "text/plain", text: deployment.logs }] };
    });
    server.registerResource("yanto_container_logs", new ResourceTemplate("yanto://containers/{id}/logs", { list: undefined }), { title: "Container logs", mimeType: "text/plain" }, async (uri, variables) => ({
      contents: [{ uri: uri.href, mimeType: "text/plain", text: await containerLogs(String(variables.id)) }]
    }));
    server.registerResource("yanto_system_logs", "yanto://system/logs", { title: "System logs", mimeType: "text/plain" }, async (uri) => ({
      contents: [{ uri: uri.href, mimeType: "text/plain", text: logger.history() || "No system log entries recorded yet." }]
    }));
  }

  return server;
}
