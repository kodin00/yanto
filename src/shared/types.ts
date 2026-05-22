export type DeploymentStatus = "running" | "success" | "failed";
export type DeploymentTrigger = "manual" | "webhook";

export type Project = {
  id: string;
  name: string;
  gitUrl: string | null;
  branch: string;
  folderName: string;
  localPath: string;
  composeFile: string;
  composeContent: string | null;
  autoStart: boolean;
  deployToken: string;
  sshPublicKey: string | null;
  createdAt: string;
  updatedAt: string;
};

export type Deployment = {
  id: string;
  projectId: string;
  projectName?: string;
  status: DeploymentStatus;
  trigger: DeploymentTrigger;
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
  cpuPercent: string;
  memoryUsage: string;
  memoryPercent: string;
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

export type ApiError = {
  message: string;
};
