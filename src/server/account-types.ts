import type { ProjectPermission, UserProjectAccess, UserRole, UserStatus } from "../shared/types.js";

export type AuthPrincipal = {
  id: string;
  username: string;
  role: UserRole;
  status: UserStatus;
  sessionVersion: number;
  projectAccess: UserProjectAccess[];
};

export type ProjectAccessInput = {
  projectId: string;
  permissions: ProjectPermission[];
};

declare module "express-serve-static-core" {
  interface Request {
    yantoAuth?: AuthPrincipal;
  }
}
