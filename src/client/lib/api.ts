import type { AuditLog, Backup, ContainerInfo, Deployment, PostgresBackupTarget, Project, SystemUsage } from "../../shared/types";

export type BackupRecord = Backup;
export type AuditLogEntry = AuditLog;
export type PostgresTarget = PostgresBackupTarget;

export type ProjectEnvVariable = {
  key: string;
  value?: string | null;
  masked?: boolean;
};

async function request<T>(path: string, options: RequestInit = {}): Promise<T> {
  const response = await fetch(path, {
    credentials: "include",
    headers: {
      "Content-Type": "application/json",
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
      .map((row) => ({ key: String(row.key), value: row.value == null ? "" : String(row.value), masked: Boolean(row.masked) }));
  }
  if (payload && typeof payload === "object") {
    return Object.entries(payload).map(([key, value]) => ({
      key,
      value: value == null ? "" : String(value),
      masked: true
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
  createProject: (payload: Pick<Project, "name" | "branch" | "folderName" | "composeFile" | "autoStart"> & { gitUrl?: string | null; composeContent?: string | null }) =>
    request<Project>("/api/projects", {
      method: "POST",
      body: JSON.stringify(payload)
    }),
  updateProject: (id: string, payload: Partial<Pick<Project, "name" | "branch" | "folderName" | "composeFile" | "autoStart"> & { gitUrl?: string | null; composeContent?: string | null }>) =>
    request<Project>(`/api/projects/${id}`, {
      method: "PATCH",
      body: JSON.stringify(payload)
    }),
  deleteProject: (id: string) => request<void>(`/api/projects/${id}`, { method: "DELETE" }),
  deployProject: (id: string) => request<{ deployment: Deployment; reused: boolean }>(`/api/projects/${id}/deploy`, { method: "POST" }),
  stopProject: (id: string) => request<{ ok: true }>(`/api/projects/${id}/stop`, { method: "POST" }),
  restartProject: (id: string) => request<{ ok: true }>(`/api/projects/${id}/restart`, { method: "POST" }),
  rollbackProject: (id: string, deploymentId: string) =>
    request<{ deployment: Deployment }>(`/api/projects/${id}/rollback`, {
      method: "POST",
      body: JSON.stringify({ deploymentId })
    }),
  projectEnv: async (id: string) => normalizeProjectEnv(await request<unknown>(`/api/projects/${id}/env`)),
  updateProjectEnv: (id: string, variables: ProjectEnvVariable[]) =>
    request<{ ok: true }>(`/api/projects/${id}/env`, {
      method: "PATCH",
      body: JSON.stringify({ variables })
    }),
  deployments: () => request<Deployment[]>("/api/deployments"),
  deploymentLogs: (id: string) => request<string>(`/api/deployments/${id}/logs`),
  deploymentLogStream: (id: string) => `/api/deployments/${id}/logs/stream`,
  backups: () => request<BackupRecord[]>("/api/backups"),
  postgresBackupTargets: () => request<PostgresTarget[]>("/api/backups/postgres-targets"),
  createBackup: (containerId?: string) =>
    request<BackupRecord>("/api/backups", {
      method: "POST",
      body: JSON.stringify(containerId ? { containerId } : {})
    }),
  deleteBackup: (id: string) => request<void>(`/api/backups/${id}`, { method: "DELETE" }),
  backupDownloadUrl: (id: string) => `/api/backups/${id}/download`,
  auditLog: () => request<AuditLogEntry[]>("/api/audit-logs"),
  containers: () => request<ContainerInfo[]>("/api/containers"),
  containerLogs: (id: string) => request<string>(`/api/containers/${id}/logs`),
  containerLogStream: (id: string) => `/api/containers/${id}/logs/stream`,
  stopContainer: (id: string) => request<{ ok: true }>(`/api/containers/${id}/stop`, { method: "POST" }),
  restartContainer: (id: string) => request<{ ok: true }>(`/api/containers/${id}/restart`, { method: "POST" }),
  systemUsage: () => request<SystemUsage>("/api/system/usage"),
  systemLogs: () => request<string>("/api/system/logs"),
  cleanupPreview: () => request<{ logs: string }>("/api/system/cleanup/preview"),
  cleanup: () => request<{ logs: string }>("/api/system/cleanup", { method: "POST" }),
  saveSshKey: (privateKey: string) =>
    request<{ ok: true; sshKey: { privateKeyPath: string; publicKey: string } }>("/api/settings/ssh-key", {
      method: "POST",
      body: JSON.stringify({ privateKey })
    }),
  settings: () =>
    request<{
      projectsRoot: string;
      hostProjectsRoot: string;
      sshKeysDir: string;
      appBaseUrl: string;
      projectCount: number;
      sshKey: {
        hasManagedKey: boolean;
        hasMountedKey: boolean;
        managedPrivateKeyPath: string;
        mountedPrivateKeyPath: string;
        activePrivateKeyPath: string | null;
        publicKey: string | null;
      };
    }>("/api/settings")
};
