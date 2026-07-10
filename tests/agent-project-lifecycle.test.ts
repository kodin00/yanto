import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => {
  const project = {
    id: "prj_test", name: "Project", gitUrl: "git@example.test:project.git", branch: "main", folderName: "project",
    localPath: "/projects/project", composeFile: "compose.yml", composeContent: null, envFile: ".env", autoStart: false,
    manualDeployEnabled: true, githubWebhookEnabled: true, targetNodeId: "node_master_local", deployToken: "token",
    sshPrivateKeyPath: null, sshPublicKey: null, agentImage: "", createdAt: new Date(), updatedAt: new Date()
  };
  const runningTask = { id: "agt_running", projectId: project.id, status: "running", worktreePath: "/worktrees/agt_running" };
  const events: string[] = [];
  const state = { tasks: [runningTask] };
  const tableName = (table: object) => (table as Record<symbol, unknown>)[Symbol.for("drizzle:Name")] as string;
  const select = vi.fn(() => ({
    from(table: object) {
      const rows = tableName(table) === "projects" ? [project] : tableName(table) === "agent_tasks" ? state.tasks : [];
      const query = {
        where: () => query,
        limit: (count: number) => Promise.resolve(rows.slice(0, count)),
        then: (resolve: (value: unknown[]) => unknown) => Promise.resolve(rows).then(resolve)
      };
      return query;
    }
  }));
  return {
    events,
    project,
    runningTask,
    state,
    db: {
      select,
      delete: vi.fn((table: object) => ({ where: vi.fn(async () => {
        events.push("database");
        if (tableName(table) === "projects") state.tasks = [];
      }) }))
    },
    cleanupTaskWorktree: vi.fn(async (_project: unknown, task: { id: string }) => { events.push(`cleanup:${task.id}`); }),
    pruneTaskWorktrees: vi.fn(async () => { events.push("prune"); }),
    removeProjectDirectory: vi.fn(async () => { events.push("deployment-directory"); }),
    removeProjectWorktreeDirectory: vi.fn(async () => { events.push("worktree-directory"); })
  };
});

vi.mock("../src/server/db/index.js", () => ({ db: mocks.db }));
vi.mock("../src/server/services/docker.js", () => ({ listContainers: vi.fn(async () => []) }));
vi.mock("../src/server/services/nodes.js", () => ({ assertDeployableNode: vi.fn() }));
vi.mock("../src/server/services/compose.js", () => ({ assertComposePortsAvailable: vi.fn() }));
vi.mock("../src/server/services/agent-worktrees.js", () => ({
  cleanupTaskWorktree: mocks.cleanupTaskWorktree,
  pruneTaskWorktrees: mocks.pruneTaskWorktrees
}));
vi.mock("../src/server/services/paths.js", () => ({
  ensureProjectsRoot: vi.fn(),
  normalizeComposeFile: vi.fn((value: string) => value),
  normalizeEnvFile: vi.fn((value: string) => value),
  pathExists: vi.fn(async () => false),
  projectPath: vi.fn((value: string) => `/projects/${value}`),
  removeProjectDirectory: mocks.removeProjectDirectory,
  removeProjectWorktreeDirectory: mocks.removeProjectWorktreeDirectory,
  slugifyFolderName: vi.fn((value: string) => value.toLowerCase())
}));

import { deleteProject } from "../src/server/services/projects.js";
import { agentProjectLifecycleKey, agentTaskLifecycleKey, withAgentLifecycleLock } from "../src/server/services/agent-lifecycle.js";

describe("agent project lifecycle", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.events.length = 0;
    mocks.state.tasks = [mocks.runningTask];
  });

  it("rejects project deletion while one of its agent tasks is running", async () => {
    await expect(deleteProject("prj_test")).rejects.toMatchObject({ status: 409 });

    expect(mocks.db.delete).not.toHaveBeenCalled();
    expect(mocks.cleanupTaskWorktree).not.toHaveBeenCalled();
    expect(mocks.pruneTaskWorktrees).not.toHaveBeenCalled();
    expect(mocks.removeProjectDirectory).not.toHaveBeenCalled();
    expect(mocks.removeProjectWorktreeDirectory).not.toHaveBeenCalled();
  });

  it("removes task worktrees and prunes them before cascading project history", async () => {
    mocks.state.tasks = [
      { ...mocks.runningTask, id: "agt_review", status: "review", worktreePath: "/worktrees/agt_review" },
      { ...mocks.runningTask, id: "agt_backlog", status: "backlog", worktreePath: null }
    ];

    await deleteProject("prj_test");

    expect(mocks.cleanupTaskWorktree).toHaveBeenCalledTimes(2);
    expect(mocks.cleanupTaskWorktree).toHaveBeenCalledWith(mocks.project, expect.objectContaining({ id: "agt_review" }));
    expect(mocks.events).toEqual([
      "cleanup:agt_review",
      "cleanup:agt_backlog",
      "prune",
      "database",
      "deployment-directory",
      "worktree-directory"
    ]);
  });

  it("does not admit a run while project deletion is cleaning task worktrees", async () => {
    mocks.state.tasks = [{ ...mocks.runningTask, status: "review" }];
    let releaseCleanup!: () => void;
    mocks.cleanupTaskWorktree.mockImplementationOnce(async () => {
      await new Promise<void>((resolve) => { releaseCleanup = resolve; });
      mocks.events.push("cleanup:agt_running");
    });

    const deletion = deleteProject("prj_test");
    await vi.waitFor(() => expect(mocks.cleanupTaskWorktree).toHaveBeenCalledOnce());
    let admitted = false;
    const admission = withAgentLifecycleLock([
      agentProjectLifecycleKey("prj_test"),
      agentTaskLifecycleKey("agt_running")
    ], async () => {
      admitted = mocks.state.tasks.some((task) => task.id === "agt_running");
    });
    await new Promise((resolve) => setImmediate(resolve));
    expect(admitted).toBe(false);

    releaseCleanup();
    await Promise.all([deletion, admission]);
    expect(admitted).toBe(false);
    expect(mocks.events.indexOf("database")).toBeGreaterThan(mocks.events.indexOf("cleanup:agt_running"));
  });
});
