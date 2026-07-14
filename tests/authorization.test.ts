import type { Request } from "express";
import { afterEach, describe, expect, it, vi } from "vitest";
import { accessibleProjectIds, assertProjectAccess, assertProjectPermission, assertProjectServiceTarget, filterByProjectAccess, filterByProjectPermission, revalidateRequestPrincipal, startStreamAuthorizationGuard } from "../src/server/authorization.js";
import { HttpError } from "../src/server/http-utils.js";

const mocks = vi.hoisted(() => ({
  loadPrincipalByUserId: vi.fn(),
  listContainers: vi.fn(),
  listProjects: vi.fn()
}));

vi.mock("../src/server/services/accounts.js", () => ({ loadPrincipalByUserId: mocks.loadPrincipalByUserId }));
vi.mock("../src/server/services/docker.js", () => ({ listContainers: mocks.listContainers }));
vi.mock("../src/server/services/projects.js", () => ({ listProjects: mocks.listProjects }));

function request(role: "owner" | "member", permissions: Array<"deploy" | "runtime" | "config" | "secrets" | "backups" | "tasks" | "hostnames"> = []) {
  return {
    yantoAuth: {
      id: role === "owner" ? "usr_owner" : "usr_member",
      username: role,
      role,
      status: "active" as const,
      sessionVersion: 1,
      projectAccess: role === "owner" ? [] : [{ projectId: "prj_allowed", projectName: "Allowed", permissions }]
    }
  } as unknown as Request;
}

describe("project authorization", () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
  });
  it("treats assignment as baseline project access", () => {
    const req = request("member");
    expect([...accessibleProjectIds(req)!]).toEqual(["prj_allowed"]);
    expect(() => assertProjectAccess(req, "prj_allowed")).not.toThrow();
    expect(() => assertProjectAccess(req, "prj_other")).toThrowError(expect.objectContaining<HttpError>({ status: 404 }));
  });

  it("distinguishes an out-of-scope project from a missing capability", () => {
    const req = request("member", ["runtime"]);
    expect(() => assertProjectPermission(req, "prj_other", "runtime")).toThrowError(expect.objectContaining<HttpError>({ status: 404 }));
    expect(() => assertProjectPermission(req, "prj_allowed", "deploy")).toThrowError(expect.objectContaining<HttpError>({ status: 403 }));
    expect(() => assertProjectPermission(req, "prj_allowed", "runtime")).not.toThrow();
  });

  it("filters scoped lists and excludes unowned resources", () => {
    const rows = [{ projectId: "prj_allowed" }, { projectId: "prj_other" }, { projectId: null }];
    expect(filterByProjectAccess(request("member"), rows)).toEqual([{ projectId: "prj_allowed" }]);
    expect(filterByProjectPermission(request("member", ["backups"]), rows, "backups")).toEqual([{ projectId: "prj_allowed" }]);
    expect(filterByProjectPermission(request("member"), rows, "backups")).toEqual([]);
    expect(filterByProjectAccess(request("owner"), rows)).toEqual(rows);
  });

  it("terminates a stream when a project grant is removed", async () => {
    vi.useFakeTimers();
    const req = request("member", ["runtime"]);
    mocks.loadPrincipalByUserId.mockResolvedValue({ ...req.yantoAuth, projectAccess: [] });
    const revoked = vi.fn();
    const guard = startStreamAuthorizationGuard(req, () => assertProjectPermission(req, "prj_allowed", "runtime"), revoked, 5);

    await vi.advanceTimersByTimeAsync(5);

    expect(revoked).toHaveBeenCalledOnce();
    guard.stop();
  });

  it("rejects a refreshed disabled or reset session", async () => {
    const req = request("member", ["backups"]);
    mocks.loadPrincipalByUserId.mockResolvedValue(null);
    await expect(revalidateRequestPrincipal(req)).rejects.toMatchObject({ status: 401 });
  });

  it("accepts only uniquely project-owned Cloudflare service targets", async () => {
    mocks.listProjects.mockResolvedValue([{ id: "prj_allowed", folderName: "allowed" }, { id: "prj_other", folderName: "other" }]);
    mocks.listContainers.mockResolvedValue([
      { id: "one", name: "allowed-app-1", composeProject: "allowed", composeService: "web" },
      { id: "two", name: "other-app-1", composeProject: "other", composeService: "api" }
    ]);

    await expect(assertProjectServiceTarget("prj_allowed", "http://allowed-app-1:3000")).resolves.toBeUndefined();
    await expect(assertProjectServiceTarget("prj_allowed", "http://other-app-1:3000")).rejects.toMatchObject({ status: 400 });
    mocks.listContainers.mockResolvedValue([
      { id: "one", name: "allowed-app-1", composeProject: "allowed", composeService: "web" },
      { id: "two", name: "other-app-1", composeProject: "other", composeService: "web" }
    ]);
    await expect(assertProjectServiceTarget("prj_allowed", "http://web:3000")).rejects.toMatchObject({ status: 400 });
  });
});
