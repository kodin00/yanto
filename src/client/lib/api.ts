import type { AgentGitPreview, AgentRun, AgentTask, AgentTaskDetail, AiModel, AiProvider, AiProviderProtocol, AuditLog, Backup, CloudflareClient, CloudflareDnsRecord, CloudflareDnsRecordType, CloudflarePublicSettings, CloudflareRoute, CloudflareRouteDiagnostic, CloudflareTunnel, CloudflareTunnelAssignment, CloudflareTunnelStatus, CloudflareZone, ContainerInfo, Deployment, DeploymentNode, FrpClientSetup, FrpOverview, FrpSettings, FrpTunnel, McpAccessLevel, McpAccessToken, MultiNodePublicSettings, PostgresBackupTarget, Project, ProjectBranch, ProjectWithDeployToken, R2PublicSettings, RollbackPreview, SetupWizardStatus, SystemUsage } from "../../shared/types";

export type BackupRecord = Backup;
export type AuditLogEntry = AuditLog;
export type PostgresTarget = PostgresBackupTarget;

export type ProjectEnvVariable = {
  key: string;
  value?: string | null;
  masked?: boolean;
};
export type ProjectEnvContent = {
  envFile: string;
  content: string;
};
export type DeploymentEnvPayload =
  | { envContent: string; envFile?: string }
  | { envVariables: ProjectEnvVariable[]; envFile?: string };
export type ProjectComposeContent = {
  composeFile: string;
  content: string;
  exists: boolean;
};

export type R2SettingsPayload = Omit<R2PublicSettings, "maskedAccessKeyId" | "hasAccessKeyId" | "hasSecretAccessKey"> & {
  accessKeyId?: string;
  secretAccessKey?: string;
};

export type CloudflareSettingsPayload = {
  accountId?: string;
  zoneId?: string;
  apiToken?: string;
};

export type MultiNodeSettingsPayload = {
  enabled: boolean;
};

export type CloudflareRoutePayload = {
  hostname: string;
  serviceTarget: string;
  noTlsVerify?: boolean;
  nodeId?: string;
};

export type CloudflareDnsRecordPayload = {
  type: CloudflareDnsRecordType;
  name: string;
  content: string;
  ttl?: number;
  proxied?: boolean;
  priority?: number | null;
  comment?: string | null;
};

export type FrpTunnelPayload = {
  name: string;
  nodeId?: string | null;
  protocol: "tcp" | "udp";
  localHost: string;
  localPort: number;
  remotePort: number;
  enabled: boolean;
};

export type SshKeyStatus = {
  hasManagedKey: boolean;
  hasMountedKey: boolean;
  managedPrivateKeyPath: string;
  mountedPrivateKeyPath: string;
  activePrivateKeyPath: string | null;
  publicKey: string | null;
};

function getCsrfToken(): string {
  const match = document.cookie.match(/(?:^|;\s*)yanto_csrf=([^;]*)/);
  return match ? decodeURIComponent(match[1]) : "";
}

async function request<T>(path: string, options: RequestInit = {}): Promise<T> {
  const csrfToken = getCsrfToken();
  const response = await fetch(path, {
    credentials: "include",
    headers: {
      "Content-Type": "application/json",
      ...(csrfToken ? { "X-CSRF-Token": csrfToken } : {}),
      ...options.headers
    },
    ...options
  });

  if (!response.ok) {
    const body = (await response.json().catch(() => ({ message: response.statusText }))) as { message?: string };
    throw new Error(body.message ?? "Request failed.");
  }

  if (response.status === 204) {
    return undefined as T;
  }

  const contentType = response.headers.get("content-type") ?? "";
  if (contentType.includes("text/plain")) {
    return (await response.text()) as T;
  }
  return response.json() as Promise<T>;
}

function normalizeProjectEnv(payload: unknown): ProjectEnvVariable[] {
  if (Array.isArray(payload)) {
    return payload
      .filter((row): row is ProjectEnvVariable => Boolean(row) && typeof row === "object" && "key" in row)
      .map((row) => ({ key: String(row.key), value: row.value == null ? "" : String(row.value) }));
  }
  if (payload && typeof payload === "object") {
    return Object.entries(payload).map(([key, value]) => ({
      key,
      value: value == null ? "" : String(value)
    }));
  }
  return [];
}

export const api = {
  login: (username: string, password: string) =>
    request<{ username: string }>("/api/auth/login", {
      method: "POST",
      body: JSON.stringify({ username, password })
    }),
  logout: () => request<{ ok: true }>("/api/auth/logout", { method: "POST" }),
  me: () => request<{ username: string }>("/api/auth/me"),
  projects: () => request<Project[]>("/api/projects"),
  nodes: () => request<DeploymentNode[]>("/api/nodes"),
  workerJoinToken: () => request<{ token: string; command: string }>("/api/nodes/join-token", { method: "POST" }),
  createProject: (
    payload: Pick<Project, "name" | "branch" | "folderName" | "composeFile" | "autoStart" | "manualDeployEnabled" | "githubWebhookEnabled" | "targetNodeId" | "agentImage"> & {
      gitUrl?: string | null;
      composeContent?: string | null;
    }
  ) =>
    request<ProjectWithDeployToken>("/api/projects", {
      method: "POST",
      body: JSON.stringify(payload)
    }),
  updateProject: (
    id: string,
    payload: Partial<
      Pick<Project, "name" | "branch" | "folderName" | "composeFile" | "autoStart" | "manualDeployEnabled" | "githubWebhookEnabled" | "targetNodeId" | "agentImage"> & {
        gitUrl?: string | null;
        composeContent?: string | null;
      }
    >
  ) =>
    request<Project>(`/api/projects/${id}`, {
      method: "PATCH",
      body: JSON.stringify(payload)
    }),
  deleteProject: (id: string) => request<void>(`/api/projects/${id}`, { method: "DELETE" }),
  projectDeployToken: (id: string) => request<{ deployToken: string }>(`/api/projects/${id}/deploy-token`),
  deployProject: (id: string, payload?: DeploymentEnvPayload) =>
    request<{ deployment: Deployment; reused: boolean }>(`/api/projects/${id}/deploy`, {
      method: "POST",
      body: JSON.stringify(payload ?? {})
    }),
  stopProject: (id: string) => request<{ ok: true }>(`/api/projects/${id}/stop`, { method: "POST" }),
  restartProject: (id: string) => request<{ ok: true }>(`/api/projects/${id}/restart`, { method: "POST" }),
  rollbackPreview: (id: string, targetRef: string) =>
    request<RollbackPreview>(`/api/projects/${id}/rollback/preview`, {
      method: "POST",
      body: JSON.stringify({ targetRef })
    }),
  rollbackProject: (id: string, targetRef: string) =>
    request<{ deployment: Deployment }>(`/api/projects/${id}/rollback`, {
      method: "POST",
      body: JSON.stringify({ targetRef })
    }),
  projectEnv: async (id: string) => normalizeProjectEnv(await request<unknown>(`/api/projects/${id}/env`)),
  projectEnvContent: (id: string) => request<ProjectEnvContent>(`/api/projects/${id}/env/content`),
  projectComposeContent: (id: string) => request<ProjectComposeContent>(`/api/projects/${id}/compose/content`),
  updateProjectEnv: (id: string, variables: ProjectEnvVariable[]) =>
    request<{ ok: true }>(`/api/projects/${id}/env`, {
      method: "PATCH",
      body: JSON.stringify({ variables })
    }),
  updateProjectEnvContent: (id: string, content: string) =>
    request<{ envFile: string; entryCount: number }>(`/api/projects/${id}/env`, {
      method: "PUT",
      body: JSON.stringify({ content })
    }),
  aiProviders: () => request<AiProvider[]>("/api/ai/providers"),
  createAiProvider: (payload: { name: string; protocol: AiProviderProtocol; baseUrl: string; apiKey: string; enabled?: boolean }) =>
    request<AiProvider>("/api/ai/providers", { method: "POST", body: JSON.stringify(payload) }),
  updateAiProvider: (id: string, payload: Partial<{ name: string; protocol: AiProviderProtocol; baseUrl: string; apiKey: string; enabled: boolean }>) =>
    request<AiProvider>(`/api/ai/providers/${id}`, { method: "PATCH", body: JSON.stringify(payload) }),
  deleteAiProvider: (id: string) => request<void>(`/api/ai/providers/${id}`, { method: "DELETE" }),
  discoverAiModels: (id: string) => request<{ ok: true; models: Array<{ id: string; name: string }> }>(`/api/ai/providers/${id}/discover`, { method: "POST" }),
  addAiModel: (providerId: string, payload: { modelId: string; displayName?: string }) =>
    request<AiModel>(`/api/ai/providers/${providerId}/models`, { method: "POST", body: JSON.stringify(payload) }),
  updateAiModel: (id: string, payload: Partial<{ displayName: string; enabled: boolean }>) =>
    request<AiModel>(`/api/ai/models/${id}`, { method: "PATCH", body: JSON.stringify(payload) }),
  deleteAiModel: (id: string) => request<void>(`/api/ai/models/${id}`, { method: "DELETE" }),
  agentTasks: () => request<AgentTask[]>("/api/agent/tasks"),
  agentTask: (id: string) => request<AgentTaskDetail>(`/api/agent/tasks/${id}`),
  createAgentTask: (payload: { projectId: string; modelId: string; title: string; prompt: string; sourceBranch: string; taskBranch: string; resumeExistingBranch: boolean; autoCommit: boolean; autoPush: boolean; autoCleanup: boolean }) =>
    request<AgentTaskDetail>("/api/agent/tasks", { method: "POST", body: JSON.stringify(payload) }),
  updateAgentTask: (id: string, payload: Partial<{ title: string; modelId: string; autoCommit: boolean; autoPush: boolean; autoCleanup: boolean }>) =>
    request<AgentTaskDetail>(`/api/agent/tasks/${id}`, { method: "PATCH", body: JSON.stringify(payload) }),
  deleteAgentTask: (id: string, force = false) => request<void>(`/api/agent/tasks/${id}${force ? "?force=true" : ""}`, { method: "DELETE" }),
  runAgentTask: (id: string, message?: string) => request<AgentRun>(`/api/agent/tasks/${id}/run`, { method: "POST", body: JSON.stringify(message ? { message } : {}) }),
  stopAgentTask: (id: string) => request<{ ok: true }>(`/api/agent/tasks/${id}/stop`, { method: "POST" }),
  projectAgentBranches: (projectId: string) => request<ProjectBranch[]>(`/api/projects/${projectId}/agent-branches`),
  agentTaskGit: (id: string) => request<AgentGitPreview>(`/api/agent/tasks/${id}/git`),
  commitAgentTask: (id: string, message: string, paths?: string[]) => request<{ sha: string }>(`/api/agent/tasks/${id}/commit`, { method: "POST", body: JSON.stringify({ message, paths }) }),
  pushAgentTask: (id: string) => request<{ sha: string }>(`/api/agent/tasks/${id}/push`, { method: "POST" }),
  cleanupAgentTask: (id: string, force = false) => request<void>(`/api/agent/tasks/${id}/worktree${force ? "?force=true" : ""}`, { method: "DELETE" }),
  agentTaskEventStream: (id: string) => `/api/agent/tasks/${id}/events/stream`,
  deployments: (limit = 500) => request<Deployment[]>(`/api/deployments?limit=${limit}`),
  deploymentLogs: (id: string) => request<string>(`/api/deployments/${id}/logs`),
  deploymentLogStream: (id: string) => `/api/deployments/${id}/logs/stream`,
  backups: () => request<BackupRecord[]>("/api/backups"),
  postgresBackupTargets: () => request<PostgresTarget[]>("/api/backups/postgres-targets"),
  createBackup: (containerId?: string) =>
    request<BackupRecord>("/api/backups", {
      method: "POST",
      body: JSON.stringify(containerId ? { containerId } : {})
    }),
  restorePostgresTarget: async (containerId: string, file: File) => {
    const csrfToken = getCsrfToken();
    const response = await fetch(`/api/backups/postgres-targets/${containerId}/restore`, {
      method: "POST",
      credentials: "include",
      headers: {
        "Content-Type": file.type || "application/octet-stream",
        "X-Filename": encodeURIComponent(file.name),
        ...(csrfToken ? { "X-CSRF-Token": csrfToken } : {})
      },
      body: file
    });
    if (!response.ok) {
      const body = (await response.json().catch(() => ({ message: response.statusText }))) as { message?: string };
      throw new Error(body.message ?? "Restore failed.");
    }
    return response.json() as Promise<{ ok: true; target: PostgresTarget }>;
  },
  deleteBackup: (id: string) => request<void>(`/api/backups/${id}`, { method: "DELETE" }),
  uploadBackupToR2: (id: string) => request<{ bucket: string; key: string; endpoint: string; filename: string; size: number }>(`/api/backups/${id}/upload-r2`, { method: "POST" }),
  backupDownloadUrl: (id: string) => `/api/backups/${id}/download`,
  auditLog: () => request<AuditLogEntry[]>("/api/audit-logs"),
  containers: () => request<ContainerInfo[]>("/api/containers"),
  containerLogs: (id: string) => request<string>(`/api/containers/${id}/logs`),
  containerLogStream: (id: string) => `/api/containers/${id}/logs/stream`,
  stopContainer: (id: string) => request<{ ok: true }>(`/api/containers/${id}/stop`, { method: "POST" }),
  startContainer: (id: string) => request<{ ok: true }>(`/api/containers/${id}/start`, { method: "POST" }),
  restartContainer: (id: string) => request<{ ok: true }>(`/api/containers/${id}/restart`, { method: "POST" }),
  systemUsage: () => request<SystemUsage>("/api/system/usage"),
  systemLogs: () => request<string>("/api/system/logs"),
  cleanupPreview: () => request<{ logs: string }>("/api/system/cleanup/preview"),
  cleanup: () => request<{ logs: string }>("/api/system/cleanup", { method: "POST" }),
  mcpTokens: () => request<McpAccessToken[]>("/api/mcp-tokens"),
  createMcpToken: (payload: { name: string; accessLevel: McpAccessLevel }) =>
    request<{ token: string; accessToken: McpAccessToken }>("/api/mcp-tokens", {
      method: "POST",
      body: JSON.stringify(payload)
    }),
  revokeMcpToken: (id: string) => request<void>(`/api/mcp-tokens/${id}`, { method: "DELETE" }),
  saveSshKey: (privateKey: string) =>
    request<{ ok: true; sshKey: SshKeyStatus }>("/api/settings/ssh-key", {
      method: "POST",
      body: JSON.stringify({ privateKey })
    }),
  generateSshKey: () =>
    request<{ ok: true; sshKey: SshKeyStatus }>("/api/settings/ssh-key/generate", {
      method: "POST"
    }),
  saveR2Settings: (payload: R2SettingsPayload) =>
    request<{ ok: true; r2: R2PublicSettings }>("/api/settings/r2", {
      method: "POST",
      body: JSON.stringify(payload)
    }),
  settings: () =>
    request<{
      projectsRoot: string;
      hostProjectsRoot: string;
      sshKeysDir: string;
      appBaseUrl: string;
      projectCount: number;
      r2: R2PublicSettings;
      cf: CloudflarePublicSettings;
      setupWizard: SetupWizardStatus;
      multiNode: MultiNodePublicSettings;
      sshKey: SshKeyStatus;
    }>("/api/settings"),
  saveSetupWizard: (action: "completed" | "dismissed") =>
    request<{ ok: true; setupWizard: SetupWizardStatus }>("/api/settings/setup-wizard", {
      method: "POST",
      body: JSON.stringify({ action })
    }),
  saveMultiNodeSettings: (payload: MultiNodeSettingsPayload) =>
    request<{ ok: true; multiNode: MultiNodePublicSettings }>("/api/settings/multi-node", {
      method: "POST",
      body: JSON.stringify(payload)
    }),
  saveCloudflareSettings: (payload: CloudflareSettingsPayload) =>
    request<{ ok: true; cf: CloudflarePublicSettings }>("/api/settings/cloudflare", {
      method: "POST",
      body: JSON.stringify(payload)
    }),
  validateCloudflareSettings: (payload: CloudflareSettingsPayload) =>
    request<{ ok: boolean; accountName?: string; zoneName?: string }>("/api/settings/cloudflare/validate", {
      method: "POST",
      body: JSON.stringify(payload)
    }),
  cloudflareTunnels: () => request<CloudflareTunnel[]>("/api/cloudflare/tunnels"),
  cloudflareClients: () => request<CloudflareClient[]>("/api/cloudflare/clients"),
  createCloudflareClient: (payload: { name: string; accountId: string; zoneId: string; apiToken: string }) => request<CloudflareClient>("/api/cloudflare/clients", { method: "POST", body: JSON.stringify(payload) }),
  updateCloudflareClient: (id: string, payload: Partial<{ name: string; accountId: string; zoneId: string; apiToken: string }>) => request<CloudflareClient>(`/api/cloudflare/clients/${id}`, { method: "PATCH", body: JSON.stringify(payload) }),
  deleteCloudflareClient: (id: string) => request<void>(`/api/cloudflare/clients/${id}`, { method: "DELETE" }),
  cloudflareZones: (clientId: string) => request<CloudflareZone[]>(`/api/cloudflare/clients/${clientId}/zones`),
  createCloudflareTunnel: (payload: { clientId: string; name: string }) => request<CloudflareTunnel>("/api/cloudflare/tunnels", { method: "POST", body: JSON.stringify(payload) }),
  deleteCloudflareTunnel: (id: string, force = false) => request<void>(`/api/cloudflare/tunnels/${id}${force ? "?force=true" : ""}`, { method: "DELETE" }),
  cloudflareAssignments: () => request<CloudflareTunnelAssignment[]>("/api/cloudflare/assignments"),
  createCloudflareAssignment: (payload: { tunnelId: string; projectId?: string; composeProject?: string; composeService?: string; containerName?: string }) => request<CloudflareTunnelAssignment>("/api/cloudflare/assignments", { method: "POST", body: JSON.stringify(payload) }),
  deleteCloudflareAssignment: (id: string) => request<void>(`/api/cloudflare/assignments/${id}`, { method: "DELETE" }),
  cloudflareHostnames: () => request<CloudflareRoute[]>("/api/cloudflare/hostnames"),
  createCloudflareHostname: (payload: { tunnelId: string; assignmentId: string; zoneId: string; hostname: string; protocol: "http" | "https"; port: number; noTlsVerify?: boolean }) => request<CloudflareRoute>("/api/cloudflare/hostnames", { method: "POST", body: JSON.stringify(payload) }),
  deleteCloudflareHostname: (id: string) => request<void>(`/api/cloudflare/hostnames/${id}`, { method: "DELETE" }),
  retryCloudflareHostname: (id: string) => request<CloudflareRoute>(`/api/cloudflare/hostnames/${id}/retry`, { method: "POST" }),
  cloudflareRouteDiagnostics: () => request<CloudflareRouteDiagnostic[]>("/api/cloudflare/routes/diagnostics"),
  cloudflareDnsRecords: () => request<CloudflareDnsRecord[]>("/api/cloudflare/dns-records"),
  cloudflareClientDnsRecords: (clientId: string) => request<CloudflareDnsRecord[]>(`/api/cloudflare/clients/${clientId}/dns-records`),
  createCloudflareDnsRecord: (payload: CloudflareDnsRecordPayload) =>
    request<CloudflareDnsRecord>("/api/cloudflare/dns-records", {
      method: "POST",
      body: JSON.stringify(payload)
    }),
  createCloudflareClientDnsRecord: (clientId: string, payload: CloudflareDnsRecordPayload) =>
    request<CloudflareDnsRecord>(`/api/cloudflare/clients/${clientId}/dns-records`, {
      method: "POST",
      body: JSON.stringify(payload)
    }),
  updateCloudflareDnsRecord: (id: string, payload: CloudflareDnsRecordPayload) =>
    request<CloudflareDnsRecord>(`/api/cloudflare/dns-records/${id}`, {
      method: "PATCH",
      body: JSON.stringify(payload)
    }),
  updateCloudflareClientDnsRecord: (clientId: string, id: string, payload: CloudflareDnsRecordPayload) =>
    request<CloudflareDnsRecord>(`/api/cloudflare/clients/${clientId}/dns-records/${id}`, {
      method: "PATCH",
      body: JSON.stringify(payload)
    }),
  deleteCloudflareDnsRecord: (id: string) =>
    request<void>(`/api/cloudflare/dns-records/${id}`, { method: "DELETE" }),
  deleteCloudflareClientDnsRecord: (clientId: string, id: string) =>
    request<void>(`/api/cloudflare/clients/${clientId}/dns-records/${id}`, { method: "DELETE" }),
  cloudflareTunnelStatus: (nodeId: string) =>
    request<CloudflareTunnelStatus>(`/api/cloudflare/tunnels/node/${nodeId}`),
  startCloudflared: (nodeId: string) =>
    request<{ ok: true }>(`/api/cloudflare/tunnels/node/${nodeId}/start`, { method: "POST" }),
  stopCloudflared: (nodeId: string) =>
    request<{ ok: true }>(`/api/cloudflare/tunnels/node/${nodeId}/stop`, { method: "POST" }),
  restartCloudflared: (nodeId: string) =>
    request<{ ok: true }>(`/api/cloudflare/tunnels/node/${nodeId}/restart`, { method: "POST" }),
  projectCfRoutes: (projectId: string) =>
    request<CloudflareRoute[]>(`/api/projects/${projectId}/cf-routes`),
  publishCfRoute: (projectId: string, payload: CloudflareRoutePayload) =>
    request<CloudflareRoute>(`/api/projects/${projectId}/cf-routes`, {
      method: "POST",
      body: JSON.stringify(payload)
    }),
  enableCfRoute: (routeId: string) =>
    request<CloudflareRoute>(`/api/cloudflare/routes/${routeId}/enable`, { method: "PATCH" }),
  disableCfRoute: (routeId: string) =>
    request<CloudflareRoute>(`/api/cloudflare/routes/${routeId}/disable`, { method: "PATCH" }),
  deleteCfRoute: (routeId: string) =>
    request<void>(`/api/cloudflare/routes/${routeId}`, { method: "DELETE" }),
  frpOverview: () => request<FrpOverview>("/api/frp/overview"),
  frpClientSetup: () => request<FrpClientSetup>("/api/frp/client-setup"),
  saveFrpSettings: (publicHost: string) =>
    request<FrpSettings>("/api/frp/settings", { method: "PUT", body: JSON.stringify({ publicHost }) }),
  controlFrpServer: (action: "start" | "stop" | "restart") =>
    request<FrpOverview["server"]>(`/api/frp/server/${action}`, { method: "POST" }),
  createFrpTunnel: (payload: FrpTunnelPayload) =>
    request<FrpTunnel>("/api/frp/tunnels", { method: "POST", body: JSON.stringify(payload) }),
  updateFrpTunnel: (id: string, payload: Partial<FrpTunnelPayload>) =>
    request<FrpTunnel>(`/api/frp/tunnels/${id}`, { method: "PATCH", body: JSON.stringify(payload) }),
  deleteFrpTunnel: (id: string) =>
    request<void>(`/api/frp/tunnels/${id}`, { method: "DELETE" })
};
