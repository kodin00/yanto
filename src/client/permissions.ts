import type { AppView, ProjectPermission, SessionUser } from "../shared/types";

const ownerOnlyViews = new Set<AppView>(["tasks", "nodes", "dns", "frp", "settings"]);

export function canView(user: SessionUser, view: AppView): boolean {
  if (user.role === "owner") return true;
  return !ownerOnlyViews.has(view) && user.allowedViews.includes(view);
}

export function canManageProject(user: SessionUser, projectId: string, permission: ProjectPermission): boolean {
  if (user.role === "owner") return true;
  return user.projectAccess.some((access) => access.projectId === projectId && access.permissions.includes(permission));
}

export function hasProjectPermission(user: SessionUser, permission: ProjectPermission): boolean {
  return user.role === "owner" || user.projectAccess.some((access) => access.permissions.includes(permission));
}
