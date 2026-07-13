import express, { type NextFunction, type Request, type Response } from "express";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const projectMocks = vi.hoisted(() => ({
  getProjectDeployToken: vi.fn(),
  listProjectsWithContainerCounts: vi.fn(),
  publicProject: vi.fn((project: Record<string, unknown>) => {
    const { deployToken, sshPrivateKeyPath, ...publicFields } = project;
    void deployToken;
    void sshPrivateKeyPath;
    return publicFields;
  }),
  createProject: vi.fn(),
  updateProject: vi.fn(),
  deleteProject: vi.fn(),
  getProject: vi.fn()
}));

const deploymentMocks = vi.hoisted(() => ({
  previewRollbackForProject: vi.fn(),
  rollbackTargetForProject: vi.fn(),
  startDeployment: vi.fn()
}));

vi.mock("../src/server/auth.js", () => ({
  currentUser: () => ({ username: "admin" }),
  requireAuth: (req: Request, res: Response, next: NextFunction) => {
    if (req.header("authorization") === "ok") {
      next();
      return;
    }
    res.status(401).json({ message: "Authentication required." });
  }
}));

vi.mock("../src/server/services/projects.js", () => projectMocks);

vi.mock("../src/server/services/audit.js", () => ({
  recordAuditLog: vi.fn()
}));

vi.mock("../src/server/services/compose.js", () => ({
  readProjectCompose: vi.fn()
}));

vi.mock("../src/server/services/deployments.js", () => deploymentMocks);

vi.mock("../src/server/services/project-env.js", () => ({
  previewEnvContent: vi.fn(),
  previewProjectEnv: vi.fn(),
  readProjectEnv: vi.fn(),
  readProjectEnvVariables: vi.fn(),
  writeProjectEnv: vi.fn(),
  writeProjectEnvVariables: vi.fn()
}));

vi.mock("../src/server/services/project-runtime.js", () => ({
  restartProjectCompose: vi.fn(),
  stopProjectCompose: vi.fn()
}));

const { default: projectsRouter } = await import("../src/server/routes/projects.js");

function createApp() {
  const app = express();
  app.use(express.json());
  app.use(projectsRouter);
  return app;
}

async function request(path: string, authorization?: string, init?: { method?: string; body?: unknown }) {
  const server = createApp().listen(0);
  try {
    const address = server.address();
    if (!address || typeof address === "string") throw new Error("Server did not start.");
    const response = await fetch(`http://127.0.0.1:${address.port}${path}`, {
      method: init?.method,
      headers: {
        ...(authorization ? { authorization } : {}),
        ...(init?.body ? { "content-type": "application/json" } : {})
      },
      body: init?.body ? JSON.stringify(init.body) : undefined
    });
    return { status: response.status, body: await response.json() as unknown };
  } finally {
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
}

describe("project routes", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    deploymentMocks.startDeployment.mockResolvedValue({
      deployment: { id: "dep_1", status: "running" },
      reused: false
    });
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("reveals deploy token for an authenticated project", async () => {
    projectMocks.getProjectDeployToken.mockResolvedValue("token-1");

    const response = await request("/api/projects/p1/deploy-token", "ok");

    expect(response.status).toBe(200);
    expect(response.body).toEqual({ deployToken: "token-1" });
  });

  it("returns 404 when deploy token project is missing", async () => {
    projectMocks.getProjectDeployToken.mockResolvedValue(undefined);

    const response = await request("/api/projects/missing/deploy-token", "ok");

    expect(response.status).toBe(404);
    expect(response.body).toEqual({ message: "Project not found." });
  });

  it("blocks unauthenticated deploy token reveal", async () => {
    const response = await request("/api/projects/p1/deploy-token");

    expect(response.status).toBe(401);
    expect(response.body).toEqual({ message: "Authentication required." });
  });

  it("returns a one-time deploy token without exposing the private SSH path", async () => {
    projectMocks.createProject.mockResolvedValue({
      id: "p1",
      name: "Example",
      deployToken: "token-1",
      sshPrivateKeyPath: "/root/.ssh/id_ed25519"
    });

    const response = await request("/api/projects", "ok", { method: "POST", body: { name: "Example" } });

    expect(response.status).toBe(201);
    expect(response.body).toEqual({ id: "p1", name: "Example", deployToken: "token-1" });
  });

  it("returns rollback preview for a commit or tag target", async () => {
    const preview = {
      requestedRef: "v1.2.3",
      current: { ref: "HEAD", sha: "a".repeat(40), message: "Current" },
      target: { ref: "v1.2.3", sha: "b".repeat(40), message: "Target" },
      commitsToApply: 0,
      commitsToLeaveBehind: 2,
      filesChanged: 1,
      additions: 1,
      deletions: 3,
      files: [{ path: "app.ts", additions: 1, deletions: 3, binary: false }]
    };
    deploymentMocks.previewRollbackForProject.mockResolvedValue(preview);

    const response = await request("/api/projects/p1/rollback/preview", "ok", { method: "POST", body: { targetRef: "v1.2.3" } });

    expect(response.status).toBe(200);
    expect(response.body).toEqual(preview);
    expect(deploymentMocks.previewRollbackForProject).toHaveBeenCalledWith("p1", "v1.2.3");
  });

  it("starts rollback using targetRef instead of a deployment history row", async () => {
    deploymentMocks.rollbackTargetForProject.mockResolvedValue({ targetRef: "v1.2.3", rollbackFromDeploymentId: null });

    const response = await request("/api/projects/p1/rollback", "ok", { method: "POST", body: { targetRef: "v1.2.3" } });

    expect(response.status).toBe(202);
    expect(deploymentMocks.rollbackTargetForProject).toHaveBeenCalledWith("p1", undefined, "v1.2.3");
    expect(deploymentMocks.startDeployment).toHaveBeenCalledWith("p1", "rollback", { targetRef: "v1.2.3", rollbackFromDeploymentId: undefined });
  });
});
