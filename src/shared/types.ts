export type DeploymentStatus = "running" | "success" | "failed";
export type DeploymentTrigger = "manual" | "webhook" | "rollback";

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
  deployToken: string;
  sshPublicKey: string | null;
  containerCount?: number;
  createdAt: string;
  updatedAt: string;
};

export type Deployment = {
  id: string;
  projectId: string;
  projectName?: string;
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
