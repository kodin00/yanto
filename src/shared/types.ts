export type DeploymentStatus = "running" | "success" | "failed";
export type DeploymentTrigger = "manual" | "webhook" | "github" | "rollback";

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
  containerCount?: number;
  cloudflareRoutes?: CloudflareRoute[];
  createdAt: string;
  updatedAt: string;
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

export type SetupWizardStatus = {
  completedAt: string | null;
  dismissedAt: string | null;
  updatedAt: string | null;
};

export type MultiNodePublicSettings = {
  enabled: boolean;
  releaseStage: "beta";
};

export type CloudflareTunnel = {
  id: string;
  nodeId: string;
  cfAccountId: string;
  cfTunnelId: string;
  tunnelName: string;
  status: string;
  lastHealthCheckAt: string | null;
  createdAt: string;
  updatedAt: string;
};

export type CloudflareRoute = {
  id: string;
  tunnelId: string;
  projectId: string;
  hostname: string;
  serviceTarget: string;
  noTlsVerify: boolean;
  enabled: boolean;
  cfDnsRecordId: string | null;
  lastPublishedAt: string | null;
  createdAt: string;
  updatedAt: string;
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
