import { describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => {
  const tableName = (table: object) => (table as Record<symbol, unknown>)[Symbol.for("drizzle:Name")] as string;
  const deployment = {
    id: "dep_shutdown",
    projectId: "prj_shutdown",
    nodeId: "node_master_local",
    status: "running",
    trigger: "manual",
    targetRef: null,
    commitSha: null,
    commitMessage: null,
    rollbackFromDeploymentId: null,
    exitCode: null,
    logs: "",
    startedAt: new Date(),
    finishedAt: null
  };
  const project = {
    id: "prj_shutdown",
    name: "Shutdown test",
    targetNodeId: "node_master_local",
    localPath: "/projects/shutdown",
    gitUrl: null,
    dockerImage: "",
    branch: "main",
    composeFile: "compose.yml",
    composeContent: null,
    envFile: ".env",
    autoStart: false,
    folderName: "shutdown"
  };
  const node = { id: "node_master_local", name: "Local", role: "master" };

  const update = vi.fn(() => ({
    set: vi.fn(() => ({
      where: vi.fn(() => Object.assign(Promise.resolve([]), {
        returning: vi.fn(async () => [{ logs: deployment.logs, status: deployment.status }])
      }))
    }))
  }));
  const select = vi.fn(() => ({
    from: vi.fn((table: object) => ({
      where: vi.fn(() => ({
        limit: vi.fn(async () => tableName(table) === "deployments" ? [deployment] : [])
      }))
    }))
  }));
  const tx = {
    execute: vi.fn(async () => undefined),
    select: vi.fn(() => ({
      from: vi.fn((table: object) => ({
        where: vi.fn(() => ({
          limit: vi.fn(async () => {
            const name = tableName(table);
            if (name === "projects") return [project];
            if (name === "deployment_nodes") return [node];
            return [];
          })
        }))
      }))
    })),
    insert: vi.fn(() => ({
      values: vi.fn(() => ({ returning: vi.fn(async () => [deployment]) }))
    }))
  };

  return {
    deployment,
    db: {
      transaction: vi.fn(async (callback: (transaction: typeof tx) => unknown) => callback(tx)),
      update,
      select
    },
    runProjectDeployment: vi.fn()
  };
});

vi.mock("../src/server/db/index.js", () => ({ db: mocks.db }));
vi.mock("../src/server/services/deployment-runner.js", () => ({ runProjectDeployment: mocks.runProjectDeployment }));
vi.mock("../src/server/services/deployment-events.js", () => ({ deploymentEvents: { emitLogUpdate: vi.fn() } }));
vi.mock("../src/server/logger.js", () => ({ logger: { info: vi.fn(), error: vi.fn(), warn: vi.fn() } }));

describe("deployment shutdown", () => {
  it("cancels and drains a local deployment, then refuses new work", async () => {
    let deploymentSignal: AbortSignal | undefined;
    mocks.runProjectDeployment.mockImplementationOnce((_project, _deployment, callbacks: { signal?: AbortSignal }) => {
      deploymentSignal = callbacks.signal;
      return new Promise<void>((_resolve, reject) => {
        callbacks.signal?.addEventListener("abort", () => reject(callbacks.signal?.reason), { once: true });
      });
    });
    const { drainActiveDeployments, startDeployment } = await import("../src/server/services/deployments.js");

    await expect(startDeployment("prj_shutdown", "manual")).resolves.toMatchObject({
      deployment: { id: "dep_shutdown" },
      reused: false
    });
    expect(deploymentSignal?.aborted).toBe(false);

    await expect(drainActiveDeployments(1_000)).resolves.toEqual({
      drained: true,
      activeDeploymentCount: 0
    });
    expect(deploymentSignal?.aborted).toBe(true);
    expect(mocks.db.update).toHaveBeenCalled();
    await expect(startDeployment("prj_shutdown", "manual")).rejects.toThrow("server is shutting down");
  });
});
