import type { Request } from "express";
import { authPrincipal } from "./auth.js";
import { HttpError } from "./http-utils.js";
import { listContainers } from "./services/docker.js";
import { listProjects } from "./services/projects.js";
import { loadPrincipalByUserId } from "./services/accounts.js";

export type ProjectPermission = "deploy" | "runtime" | "config" | "secrets" | "backups" | "tasks" | "hostnames";
export const STREAM_AUTHORIZATION_INTERVAL_MS = 15_000;

export async function revalidateRequestPrincipal(req: Request) {
  const initial = principal(req);
  const fresh = await loadPrincipalByUserId(initial.id);
  if (
    !fresh ||
    fresh.status !== "active" ||
    fresh.sessionVersion !== initial.sessionVersion ||
    fresh.username !== initial.username
  ) {
    throw new HttpError(401, "Session access was revoked.");
  }
  req.yantoAuth = fresh;
  return fresh;
}

function principal(req: Request) {
  const value = authPrincipal(req);
  if (!value) throw new HttpError(401, "Authentication required.");
  return value;
}

export function isOwner(req: Request) {
  return principal(req).role === "owner";
}

export function accessibleProjectIds(req: Request) {
  const user = principal(req);
  return user.role === "owner" ? null : new Set(user.projectAccess.map((grant) => grant.projectId));
}

export function assertProjectAccess(req: Request, projectId: string) {
  const user = principal(req);
  if (user.role === "owner") return;
  if (!user.projectAccess.some((grant) => grant.projectId === projectId)) {
    throw new HttpError(404, "Project not found.");
  }
}

export function assertProjectPermission(req: Request, projectId: string, permission: ProjectPermission) {
  const user = principal(req);
  if (user.role === "owner") return;
  const grant = user.projectAccess.find((item) => item.projectId === projectId);
  if (!grant) throw new HttpError(404, "Project not found.");
  if (!grant.permissions.includes(permission)) throw new HttpError(403, `Project ${permission} permission is required.`);
}

export function hasProjectPermission(req: Request, projectId: string, permission: ProjectPermission) {
  const user = principal(req);
  return user.role === "owner" || Boolean(user.projectAccess.find((grant) => grant.projectId === projectId)?.permissions.includes(permission));
}

export function assertAnyProjectPermission(req: Request, permission: ProjectPermission) {
  const user = principal(req);
  if (user.role === "owner") return;
  if (!user.projectAccess.some((grant) => grant.permissions.includes(permission))) {
    throw new HttpError(403, `Project ${permission} permission is required.`);
  }
}

export async function projectForContainer(req: Request, containerId: string, permission?: ProjectPermission) {
  if (isOwner(req)) return null;
  const [containers, projects] = await Promise.all([listContainers(), listProjects()]);
  const container = containers.find((item) => item.id === containerId || item.name === containerId);
  if (!container?.composeProject) throw new HttpError(404, "Container not found.");
  const matches = projects.filter((project) => project.folderName === container.composeProject);
  if (matches.length !== 1) throw new HttpError(404, "Container not found.");
  const project = matches[0];
  if (permission) assertProjectPermission(req, project.id, permission);
  else assertProjectAccess(req, project.id);
  return project;
}

export async function listAccessibleContainers(req: Request) {
  const containers = await listContainers();
  if (isOwner(req)) return containers;
  const projects = await listProjects();
  const ids = accessibleProjectIds(req) ?? new Set<string>();
  const projectsByFolder = new Map<string, typeof projects>();
  for (const project of projects) {
    projectsByFolder.set(project.folderName, [...(projectsByFolder.get(project.folderName) ?? []), project]);
  }
  return containers.filter((container) => {
    if (!container.composeProject) return false;
    const matches = projectsByFolder.get(container.composeProject) ?? [];
    return matches.length === 1 && ids.has(matches[0].id);
  });
}

export async function assertProjectServiceTarget(projectId: string, serviceTarget: string) {
  const [containers, projects] = await Promise.all([listContainers(), listProjects()]);
  const project = projects.find((item) => item.id === projectId);
  if (!project) throw new HttpError(404, "Project not found.");
  if (projects.filter((item) => item.folderName === project.folderName).length !== 1) {
    throw new HttpError(409, "Project container mapping is ambiguous until folder names are unique.");
  }

  let targetHost = "";
  try {
    targetHost = new URL(serviceTarget).hostname.toLowerCase();
  } catch {
    throw new HttpError(400, "Service target must be a valid URL.");
  }
  const matches = containers.filter((container) =>
    container.name.toLowerCase() === targetHost || container.composeService?.toLowerCase() === targetHost
  );
  if (!matches.length || matches.some((container) => container.composeProject !== project.folderName)) {
    throw new HttpError(400, "Service target must uniquely identify a container or Compose service belonging to this project.");
  }
}

export function filterByProjectAccess<T extends { projectId: string | null }>(req: Request, rows: T[]) {
  const ids = accessibleProjectIds(req);
  return ids === null ? rows : rows.filter((row) => row.projectId !== null && ids.has(row.projectId));
}

export function filterByProjectPermission<T extends { projectId: string | null }>(req: Request, rows: T[], permission: ProjectPermission) {
  const user = principal(req);
  if (user.role === "owner") return rows;
  const ids = new Set(user.projectAccess.filter((grant) => grant.permissions.includes(permission)).map((grant) => grant.projectId));
  return rows.filter((row) => row.projectId !== null && ids.has(row.projectId));
}

export function startStreamAuthorizationGuard(
  req: Request,
  revalidate: () => void | Promise<void>,
  onRevoked: () => void,
  intervalMs = STREAM_AUTHORIZATION_INTERVAL_MS
) {
  const initial = principal(req);
  let stopped = false;
  let checking = false;

  const stop = () => {
    if (stopped) return;
    stopped = true;
    clearInterval(timer);
  };

  const revalidateNow = async () => {
    if (stopped || checking) return;
    checking = true;
    try {
      const fresh = await revalidateRequestPrincipal(req);
      if (fresh.id !== initial.id || fresh.sessionVersion !== initial.sessionVersion) throw new HttpError(401, "Session access was revoked.");
      await revalidate();
    } catch {
      stop();
      onRevoked();
    } finally {
      checking = false;
    }
  };

  const timer = setInterval(() => void revalidateNow(), intervalMs);
  timer.unref();
  return { stop, revalidateNow };
}
