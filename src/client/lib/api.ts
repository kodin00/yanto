import type { ContainerInfo, Deployment, Project, SystemUsage } from "../../shared/types";

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
  deployments: () => request<Deployment[]>("/api/deployments"),
  deploymentLogs: (id: string) => request<string>(`/api/deployments/${id}/logs`),
  deploymentLogStream: (id: string) => `/api/deployments/${id}/logs/stream`,
  containers: () => request<ContainerInfo[]>("/api/containers"),
  containerLogs: (id: string) => request<string>(`/api/containers/${id}/logs`),
  containerLogStream: (id: string) => `/api/containers/${id}/logs/stream`,
  stopContainer: (id: string) => request<{ ok: true }>(`/api/containers/${id}/stop`, { method: "POST" }),
  restartContainer: (id: string) => request<{ ok: true }>(`/api/containers/${id}/restart`, { method: "POST" }),
  systemUsage: () => request<SystemUsage>("/api/system/usage"),
  systemLogs: () => request<string>("/api/system/logs"),
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
