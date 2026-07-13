import bcrypt from "bcryptjs";
import type { NextFunction, Request, Response } from "express";
import jwt from "jsonwebtoken";
import { config } from "./config.js";

const cookieName = "yanto_session";
const sessionIssuer = "yanto";
const sessionAudience = "yanto-dashboard";

type SessionPayload = {
  sub: string;
  username: string;
};

let resolvedPasswordHash: Promise<string> | null = null;

export async function verifyAdminPassword(username: string, password: string) {
  if (username !== config.adminUsername) {
    return false;
  }
  if (resolvedPasswordHash === null) {
    resolvedPasswordHash = config.adminPassword.startsWith("$2")
      ? Promise.resolve(config.adminPassword)
      : bcrypt.hash(config.adminPassword, 12);
  }
  return bcrypt.compare(password, await resolvedPasswordHash);
}

export function setSessionCookie(res: Response) {
  const token = jwt.sign(
    {
      sub: "admin",
      username: config.adminUsername
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

export function currentUser(req: Request): SessionPayload | null {
  const token = req.cookies?.[cookieName] as string | undefined;
  if (!token) {
    return null;
  }
  try {
    const payload = jwt.verify(token, config.jwtSecret, {
      algorithms: ["HS256"],
      audience: sessionAudience,
      issuer: sessionIssuer
    });
    if (
      typeof payload === "string" ||
      payload.sub !== "admin" ||
      payload.username !== config.adminUsername
    ) {
      return null;
    }
    return { sub: payload.sub, username: payload.username };
  } catch {
    return null;
  }
}

export function requireAuth(req: Request, res: Response, next: NextFunction) {
  const user = currentUser(req);
  if (!user) {
    res.status(401).json({ message: "Authentication required." });
    return;
  }
  next();
}
