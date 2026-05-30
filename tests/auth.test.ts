import { describe, expect, it, vi } from "vitest";
import type { NextFunction, Request, Response } from "express";
import jwt from "jsonwebtoken";
import { verifyAdminPassword, currentUser, requireAuth } from "../src/server/auth.js";
import { config } from "../src/server/config.js";

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

describe("verifyAdminPassword", () => {
  it("returns true for correct username and password", async () => {
    expect(await verifyAdminPassword(config.adminUsername, config.adminPassword)).toBe(true);
  });

  it("returns false for wrong username", async () => {
    expect(await verifyAdminPassword("wronguser", config.adminPassword)).toBe(false);
  });

  it("returns false for wrong password", async () => {
    expect(await verifyAdminPassword(config.adminUsername, "wrongpassword")).toBe(false);
  });
});

describe("currentUser", () => {
  it("returns null when no cookie", () => {
    const req = mockReq();
    expect(currentUser(req)).toBeNull();
  });

  it("returns payload for valid JWT", () => {
    const token = jwt.sign({ sub: "admin", username: config.adminUsername }, config.jwtSecret, { expiresIn: "7d" });
    const req = mockReq({ yanto_session: token });
    const user = currentUser(req);
    expect(user).not.toBeNull();
    expect(user!.sub).toBe("admin");
    expect(user!.username).toBe(config.adminUsername);
  });

  it("returns null for invalid JWT", () => {
    const req = mockReq({ yanto_session: "invalid.token.here" });
    expect(currentUser(req)).toBeNull();
  });

  it("returns null for expired JWT", () => {
    const token = jwt.sign({ sub: "admin", username: config.adminUsername }, config.jwtSecret, { expiresIn: "-1s" });
    const req = mockReq({ yanto_session: token });
    expect(currentUser(req)).toBeNull();
  });
});

describe("requireAuth", () => {
  it("calls next() when valid session exists", () => {
    const token = jwt.sign({ sub: "admin", username: config.adminUsername }, config.jwtSecret, { expiresIn: "7d" });
    const req = mockReq({ yanto_session: token });
    const res = mockRes();
    const next = vi.fn() as unknown as NextFunction;

    requireAuth(req, res, next);

    expect(next).toHaveBeenCalled();
    expect(res.status).not.toHaveBeenCalled();
  });

  it("returns 401 when no session", () => {
    const req = mockReq();
    const res = mockRes();
    const next = vi.fn() as unknown as NextFunction;

    requireAuth(req, res, next);

    expect(next).not.toHaveBeenCalled();
    expect(res.status).toHaveBeenCalledWith(401);
    expect(res.json).toHaveBeenCalledWith({ message: "Authentication required." });
  });
});
