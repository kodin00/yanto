import { describe, expect, it, vi } from "vitest";
import type { NextFunction, Request, Response } from "express";
import jwt from "jsonwebtoken";
import type { AuthPrincipal } from "../src/server/account-types.js";
import { config } from "../src/server/config.js";

const accountMocks = vi.hoisted(() => ({
  loadPrincipalByUserId: vi.fn(),
  authenticateUser: vi.fn()
}));

vi.mock("../src/server/services/accounts.js", () => accountMocks);

import { currentUser, requireAuth, verifyAdminPassword } from "../src/server/auth.js";

const principal: AuthPrincipal = {
  id: "usr_owner",
  username: "Owner",
  role: "owner",
  status: "active",
  sessionVersion: 3,
  projectAccess: []
};

function signSession(payload: Record<string, unknown> = {}, expiresIn: jwt.SignOptions["expiresIn"] = "7d") {
  return jwt.sign(
    { sub: principal.id, username: principal.username, sessionVersion: principal.sessionVersion, ...payload },
    config.jwtSecret,
    { algorithm: "HS256", audience: "yanto-dashboard", expiresIn, issuer: "yanto" }
  );
}

function mockReq(cookies: Record<string, string> = {}) {
  return { cookies } as unknown as Request;
}

function mockRes() {
  const res: any = {};
  res.status = vi.fn().mockReturnValue(res);
  res.json = vi.fn().mockReturnValue(res);
  res.cookie = vi.fn();
  res.clearCookie = vi.fn();
  return res as Response & { status: ReturnType<typeof vi.fn>; json: ReturnType<typeof vi.fn>; cookie: ReturnType<typeof vi.fn>; clearCookie: ReturnType<typeof vi.fn> };
}

describe("verifyAdminPassword compatibility", () => {
  it("delegates to database-backed authentication", async () => {
    accountMocks.authenticateUser.mockResolvedValueOnce(principal);
    expect(await verifyAdminPassword("Owner", "password")).toBe(true);
    expect(accountMocks.authenticateUser).toHaveBeenCalledWith("Owner", "password");
  });

  it("returns false when authentication fails", async () => {
    accountMocks.authenticateUser.mockResolvedValueOnce(null);
    expect(await verifyAdminPassword("unknown", "wrong")).toBe(false);
  });
});

describe("currentUser", () => {
  it("returns null when no cookie", () => {
    expect(currentUser(mockReq())).toBeNull();
  });

  it("returns the signed database identity for a valid JWT", () => {
    const user = currentUser(mockReq({ yanto_session: signSession() }));
    expect(user).toEqual({ sub: principal.id, username: principal.username, sessionVersion: principal.sessionVersion });
  });

  it("rejects invalid, expired, and legacy JWTs", () => {
    expect(currentUser(mockReq({ yanto_session: "invalid.token.here" }))).toBeNull();
    expect(currentUser(mockReq({ yanto_session: signSession({}, "-1s") }))).toBeNull();
    const legacy = jwt.sign({ sub: "admin", username: "admin" }, config.jwtSecret, { expiresIn: "7d" });
    expect(currentUser(mockReq({ yanto_session: legacy }))).toBeNull();
  });
});

describe("requireAuth", () => {
  it("reloads the principal and attaches it to the request", async () => {
    accountMocks.loadPrincipalByUserId.mockResolvedValueOnce(principal);
    const req = mockReq({ yanto_session: signSession() });
    const res = mockRes();
    const next = vi.fn() as unknown as NextFunction;

    requireAuth(req, res, next);
    await vi.waitFor(() => expect(next).toHaveBeenCalled());

    expect(req.yantoAuth).toEqual(principal);
    expect(res.status).not.toHaveBeenCalled();
  });

  it("rejects a revoked session version", async () => {
    accountMocks.loadPrincipalByUserId.mockResolvedValueOnce({ ...principal, sessionVersion: 4 });
    const req = mockReq({ yanto_session: signSession() });
    const res = mockRes();
    const next = vi.fn() as unknown as NextFunction;

    requireAuth(req, res, next);
    await vi.waitFor(() => expect(res.status).toHaveBeenCalledWith(401));

    expect(next).not.toHaveBeenCalled();
    expect(res.clearCookie).toHaveBeenCalled();
  });

  it("returns 401 when no session exists", async () => {
    const req = mockReq();
    const res = mockRes();
    const next = vi.fn() as unknown as NextFunction;

    requireAuth(req, res, next);
    await vi.waitFor(() => expect(res.status).toHaveBeenCalledWith(401));

    expect(next).not.toHaveBeenCalled();
    expect(res.json).toHaveBeenCalledWith({ message: "Authentication required." });
  });
});
