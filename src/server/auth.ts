import type { NextFunction, Request, Response } from "express";
import jwt from "jsonwebtoken";
import type { ProjectPermission } from "../shared/types.js";
import type { AuthPrincipal } from "./account-types.js";
import { config } from "./config.js";
import { loadPrincipalByUserId } from "./services/accounts.js";

const cookieName = "yanto_session";
const sessionIssuer = "yanto";
const sessionAudience = "yanto-dashboard";

type SessionIdentity = {
  sub: string;
  username: string;
  sessionVersion: number;
};

export async function verifyAdminPassword(username: string, password: string) {
  const { authenticateUser } = await import("./services/accounts.js");
  return Boolean(await authenticateUser(username, password));
}

export function setSessionCookie(res: Response, principal: AuthPrincipal) {
  const token = jwt.sign(
    {
      sub: principal.id,
      username: principal.username,
      sessionVersion: principal.sessionVersion
    },
    config.jwtSecret,
    {
      algorithm: "HS256",
      audience: sessionAudience,
      expiresIn: "7d",
      issuer: sessionIssuer
    }
  );

  res.cookie(cookieName, token, {
    httpOnly: true,
    path: "/",
    sameSite: "lax",
    secure: config.cookieSecure,
    maxAge: 7 * 24 * 60 * 60 * 1000
  });
}

export function clearSessionCookie(res: Response) {
  res.clearCookie(cookieName, {
    httpOnly: true,
    path: "/",
    sameSite: "lax",
    secure: config.cookieSecure
  });
}

export function currentUser(req: Request): SessionIdentity | AuthPrincipal | null {
  if (req.yantoAuth) return req.yantoAuth;
  const token = req.cookies?.[cookieName] as string | undefined;
  if (!token) return null;
  try {
    const payload = jwt.verify(token, config.jwtSecret, {
      algorithms: ["HS256"],
      audience: sessionAudience,
      issuer: sessionIssuer
    });
    if (
      typeof payload === "string" ||
      typeof payload.sub !== "string" ||
      typeof payload.username !== "string" ||
      typeof payload.sessionVersion !== "number" ||
      !Number.isInteger(payload.sessionVersion)
    ) {
      return null;
    }
    return { sub: payload.sub, username: payload.username, sessionVersion: payload.sessionVersion };
  } catch {
    return null;
  }
}

async function authenticateRequest(req: Request) {
  if (req.yantoAuth) return req.yantoAuth;
  const identity = currentUser(req);
  if (!identity) return null;
  const principal = await loadPrincipalByUserId("sub" in identity ? identity.sub : identity.id);
  if (
    !principal ||
    principal.status !== "active" ||
    principal.username !== identity.username ||
    principal.sessionVersion !== identity.sessionVersion
  ) {
    return null;
  }
  req.yantoAuth = principal;
  return principal;
}

export function requireAuth(req: Request, res: Response, next: NextFunction) {
  void authenticateRequest(req).then((principal) => {
    if (!principal) {
      clearSessionCookie(res);
      res.status(401).json({ message: "Authentication required." });
      return;
    }
    next();
  }).catch(next);
}

export function requireOwner(req: Request, res: Response, next: NextFunction) {
  void authenticateRequest(req).then((principal) => {
    if (!principal) {
      clearSessionCookie(res);
      res.status(401).json({ message: "Authentication required." });
      return;
    }
    if (principal.role !== "owner") {
      res.status(403).json({ message: "Owner access required." });
      return;
    }
    next();
  }).catch(next);
}

export function authPrincipal(req: Request) {
  return req.yantoAuth ?? null;
}

export function canAccessProject(principal: AuthPrincipal, projectId: string) {
  return principal.role === "owner" || principal.projectAccess.some((access) => access.projectId === projectId);
}

export function hasProjectPermission(principal: AuthPrincipal, projectId: string, permission: ProjectPermission) {
  return principal.role === "owner" || principal.projectAccess.some((access) => access.projectId === projectId && access.permissions.includes(permission));
}

export function requireProjectAccess(paramName = "id") {
  return (req: Request, res: Response, next: NextFunction) => {
    void authenticateRequest(req).then((principal) => {
      if (!principal) {
        clearSessionCookie(res);
        res.status(401).json({ message: "Authentication required." });
        return;
      }
      const projectId = String(req.params[paramName] ?? "");
      if (!canAccessProject(principal, projectId)) {
        res.status(404).json({ message: "Project not found." });
        return;
      }
      next();
    }).catch(next);
  };
}

export function requireProjectPermission(permission: ProjectPermission, paramName = "id") {
  return (req: Request, res: Response, next: NextFunction) => {
    void authenticateRequest(req).then((principal) => {
      if (!principal) {
        clearSessionCookie(res);
        res.status(401).json({ message: "Authentication required." });
        return;
      }
      const projectId = String(req.params[paramName] ?? "");
      if (!canAccessProject(principal, projectId)) {
        res.status(404).json({ message: "Project not found." });
        return;
      }
      if (!hasProjectPermission(principal, projectId, permission)) {
        res.status(403).json({ message: `Project ${permission} permission required.` });
        return;
      }
      next();
    }).catch(next);
  };
}
