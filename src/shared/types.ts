export type DeploymentStatus = "running" | "success" | "failed";
export type DeploymentTrigger = "manual" | "webhook" | "github" | "rollback";
export type McpAccessLevel = "read" | "write" | "admin";

export type McpAccessToken = {
  id: string;
  name: string;
  accessLevel: McpAccessLevel;
  lastUsedAt: string | null;
  revokedAt: string | null;
  createdAt: string;
  updatedAt: string;
};

export type Project = {
  id: string;
  name: string;
  gitUrl: string | null;
  branch: string;
  folderName: string;
  localPath: string;
  composeFile: string;
  composeContent: string | null;
  envFile: string;
  autoStart: boolean;
  manualDeployEnabled: boolean;
  githubWebhookEnabled: boolean;
  targetNodeId: string;
  sshPublicKey: string | null;
  agentImage: string;
  containerCount?: number;
  cloudflareRoutes?: CloudflareRoute[];
  createdAt: string;
  updatedAt: string;
};

export type AiProviderProtocol = "openai_responses" | "openai_chat" | "anthropic_messages" | "codex_account";

export type CodexAccountStatus = {
  connected: boolean;
  email: string | null;
  planType: string | null;
  login: { loginId: string; verificationUrl: string; userCode: string } | null;
};

export type AiModel = {
  id: string;
  providerId: string;
  modelId: string;
  displayName: string;
  enabled: boolean;
  createdAt: string;
  updatedAt: string;
};

export type AiProvider = {
  id: string;
  name: string;
  protocol: AiProviderProtocol;
  baseUrl: string;
  hasApiKey: boolean;
  enabled: boolean;
  models: AiModel[];
  createdAt: string;
  updatedAt: string;
};

export type AgentTaskStatus = "backlog" | "running" | "review" | "done";
export type AgentRunStatus = "running" | "succeeded" | "failed" | "canceled";

export type AgentRun = {
  id: string;
  taskId: string;
  status: AgentRunStatus;
  providerProtocol: AiProviderProtocol;
  modelName: string;
  assistantText: string;
  error: string | null;
  startedAt: string;
  finishedAt: string | null;
};

export type AgentMessage = {
  id: string;
  taskId: string;
  runId: string | null;
  role: "user" | "assistant";
  content: string;
  createdAt: string;
};

export type AgentTask = {
  id: string;
  projectId: string;
  projectName: string;
  modelId: string;
  modelName: string;
  providerName: string;
  title: string;
  prompt: string;
  status: AgentTaskStatus;
  sourceBranch: string;
  taskBranch: string;
  sourceSha: string | null;
  worktreePath: string | null;
  codexThreadId: string | null;
  resumeExistingBranch: boolean;
  autoCommit: boolean;
  autoPush: boolean;
  autoCleanup: boolean;
  lastError: string | null;
  createdAt: string;
  updatedAt: string;
  startedAt: string | null;
  finishedAt: string | null;
  pushedAt: string | null;
  latestRun: AgentRun | null;
};

export type AgentTaskDetail = AgentTask & {
  messages: AgentMessage[];
  runs: AgentRun[];
};

export type AgentGitFile = {
  path: string;
  status: string;
  additions: number | null;
  deletions: number | null;
  binary: boolean;
};

export type AgentGitPreview = {
  branch: string;
  baseBranch: string;
  headSha: string;
  isClean: boolean;
  ahead: number;
  behind: number;
  files: AgentGitFile[];
  diff: string;
};

export type ProjectBranch = {
  name: string;
  sha: string;
  remote: boolean;
};

export type ProjectWithDeployToken = Project & {
  deployToken: string;
};

export type Deployment = {
  id: string;
  projectId: string;
  projectName?: string;
  nodeId: string;
  nodeName?: string | null;
  status: DeploymentStatus;
  trigger: DeploymentTrigger;
  targetRef: string | null;
  commitSha: string | null;
  commitMessage: string | null;
  rollbackFromDeploymentId: string | null;
  logs: string;
  exitCode: number | null;
  startedAt: string;
  finishedAt: string | null;
};

export type GitRefSummary = {
  ref: string;
  sha: string;
  message: string;
};

export type RollbackDiffFile = {
  path: string;
  additions: number | null;
  deletions: number | null;
  binary: boolean;
};

export type RollbackPreview = {
  requestedRef: string;
  current: GitRefSummary;
  target: GitRefSummary;
  commitsToApply: number;
  commitsToLeaveBehind: number;
  filesChanged: number;
  additions: number;
  deletions: number;
  files: RollbackDiffFile[];
};

export type DeploymentNode = {
  id: string;
  name: string;
  role: "master" | "worker" | string;
  status: "online" | "offline" | string;
  lastSeenAt: string | null;
  dockerVersion: string | null;
  labels: Record<string, string>;
  projectCount?: number;
  runningDeploymentCount?: number;
  createdAt: string;
  updatedAt: string;
};

export type ContainerInfo = {
  id: string;
  name: string;
  image: string;
  status: string;
  state: string;
  ports: string;
  createdAt: string | null;
  cpuPercent: string;
  memoryUsage: string;
  memoryPercent: string;
  composeProject?: string | null;
  composeService?: string | null;
  isPostgresCandidate?: boolean;
};

export type Backup = {
  id: string;
  projectId: string | null;
  kind: string;
  status: string;
  filename: string;
  filePath: string;
  fileSizeBytes: number | null;
  error: string | null;
  note: string | null;
  createdAt: string;
  finishedAt: string | null;
  downloadedAt: string | null;
  downloadCount: number;
};

export type PostgresBackupTarget = {
  containerId: string;
  containerName: string;
  image: string;
  status: string;
  state: string;
  composeProject: string | null;
  composeService: string | null;
  projectId: string | null;
  projectName: string | null;
  databaseName: string;
  databaseUser: string;
};

export type R2PublicSettings = {
  enabled: boolean;
  accountId: string;
  bucket: string;
  maskedAccessKeyId: string;
  hasAccessKeyId: boolean;
  hasSecretAccessKey: boolean;
  prefix: string;
};

export type CloudflarePublicSettings = {
  accountId: string;
  zoneId: string;
  hasApiToken: boolean;
};

export type CloudflareClient = {
  id: string;
  name: string;
  accountId: string;
  zoneId: string;
  hasApiToken: boolean;
  createdAt: string;
  updatedAt: string;
};

export type CloudflareZone = { id: string; name: string; status: string };

export type SetupWizardStatus = {
  completedAt: string | null;
  dismissedAt: string | null;
  updatedAt: string | null;
};

export type MultiNodePublicSettings = {
  enabled: boolean;
  releaseStage: "beta";
};

export type FrpProtocol = "tcp" | "udp";
export type FrpTunnelStatus = "disabled" | "error" | "syncing" | "online" | "offline";

export type FrpSettings = {
  publicHost: string;
  bindPort: number;
  portStart: number;
  portEnd: number;
  configured: boolean;
};

export type FrpTunnel = {
  id: string;
  nodeId: string | null;
  nodeName: string | null;
  name: string;
  protocol: FrpProtocol;
  localHost: string;
  localPort: number;
  remotePort: number;
  enabled: boolean;
  syncStatus: FrpTunnelStatus;
  lastError: string | null;
  lastSyncedAt: string | null;
  trafficInBytes: number;
  trafficOutBytes: number;
  currentConnections: number;
  createdAt: string;
  updatedAt: string;
};

export type FrpClientSetup = {
  serverAddr: string;
  serverPort: number;
  authToken: string;
  tokenConfigured: boolean;
  tunnelCount: number;
  frpcToml: string;
  installScript: string;
};

export type FrpServerStatus = {
  running: boolean;
  containerStatus: string | null;
  version: string | null;
  uptimeSeconds: number | null;
  trafficInBytes: number;
  trafficOutBytes: number;
  error: string | null;
};

export type FrpOverview = {
  settings: FrpSettings;
  server: FrpServerStatus;
  tunnels: FrpTunnel[];
};

export type CloudflareTunnel = {
  id: string;
  clientId: string;
  nodeId: string;
  cfAccountId: string;
  cfTunnelId: string;
  tunnelName: string;
  dockerNetworkName: string;
  status: string;
  lastHealthCheckAt: string | null;
  createdAt: string;
  updatedAt: string;
};

export type CloudflareTunnelAssignment = {
  id: string;
  tunnelId: string;
  targetType: "compose_service" | "container";
  projectId: string | null;
  composeProject: string | null;
  composeService: string | null;
  containerName: string | null;
  createdAt: string;
};

export type CloudflareRoute = {
  id: string;
  tunnelId: string;
  projectId: string | null;
  assignmentId: string | null;
  zoneId: string;
  hostname: string;
  serviceTarget: string;
  protocol: string;
  port: number;
  noTlsVerify: boolean;
  enabled: boolean;
  syncStatus: string;
  lastError: string | null;
  cfDnsRecordId: string | null;
  lastPublishedAt: string | null;
  createdAt: string;
  updatedAt: string;
};

export type CloudflareDnsRecordType = "A" | "AAAA" | "CNAME" | "TXT" | "MX" | "NS";

export type CloudflareDnsRecord = {
  id: string;
  type: CloudflareDnsRecordType | string;
  name: string;
  content: string;
  ttl: number;
  proxiable: boolean;
  proxied: boolean;
  priority: number | null;
  comment: string | null;
  createdOn: string | null;
  modifiedOn: string | null;
};

export type CloudflareRouteDnsStatus = "ok" | "missing" | "mismatch" | "conflict" | "unknown";
export type CloudflareRouteTunnelStatus = "running" | "stopped" | "missing" | "unhealthy" | "unknown";
export type CloudflareRouteReachabilityStatus = "ok" | "failed" | "skipped" | "unknown";

export type CloudflareRouteDiagnosticDnsRecord = Pick<CloudflareDnsRecord, "id" | "type" | "name" | "content" | "proxied">;

export type CloudflareRouteDiagnostic = {
  routeId: string;
  tunnelId: string;
  projectId: string | null;
  projectName: string | null;
  hostname: string;
  serviceTarget: string;
  routeEnabled: boolean;
  expectedDnsTarget: string | null;
  actualDnsRecords: CloudflareRouteDiagnosticDnsRecord[];
  dnsStatus: CloudflareRouteDnsStatus;
  tunnelStatus: CloudflareRouteTunnelStatus;
  reachabilityStatus: CloudflareRouteReachabilityStatus;
  messages: string[];
  recommendedFixes: string[];
  checkedAt: string;
};

export type CloudflareTunnelStatus = {
  tunnel: CloudflareTunnel;
  runtime: {
    running: boolean;
    containerId?: string;
    status?: string;
  };
  health: {
    healthy: boolean;
    connectors?: number;
    status?: string;
  };
};

export type AuditLog = {
  id: string;
  actor: string;
  action: string;
  entityType: string;
  entityId: string | null;
  projectId: string | null;
  metadata: Record<string, unknown>;
  createdAt: string;
};

export type EnvPreviewEntry = {
  line: number;
  key: string | null;
  hasValue: boolean;
  maskedValue: string | null;
  comment: string | null;
};

export type EnvPreview = {
  envFile: string;
  entryCount: number;
  entries: EnvPreviewEntry[];
};

export type SystemUsage = {
  cpuLoadPercent: number;
  memory: {
    total: number;
    used: number;
    free: number;
    usedPercent: number;
  };
  storage: {
    filesystem: string;
    size: number;
    used: number;
    available: number;
    usedPercent: number;
    mount: string;
  }[];
};

export type HealthStatus = {
  ok: boolean;
  uptimeSeconds: number;
  checkedAt: string;
  checks: {
    database: {
      ok: boolean;
      message?: string;
    };
    docker: {
      ok: boolean;
      message?: string;
    };
  };
};

export type ApiError = {
  message: string;
};
