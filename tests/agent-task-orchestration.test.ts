import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentMessageRow, AgentRunRow, AgentTaskRow, ProjectRow } from "../src/server/db/schema.js";

type TestState = {
  tasks: AgentTaskRow[];
  messages: AgentMessageRow[];
  runs: AgentRunRow[];
  events: Array<Record<string, unknown>>;
};

const mocks = vi.hoisted(() => {
  const state: TestState = { tasks: [], messages: [], runs: [], events: [] };
  const tableName = (table: object) => (table as Record<symbol, unknown>)[Symbol.for("drizzle:Name")] as string;
  const conditionValues = (condition: unknown): unknown[] => {
    if (!condition || typeof condition !== "object") return [];
    const record = condition as { queryChunks?: unknown[]; value?: unknown };
    const own = Object.prototype.hasOwnProperty.call(record, "value") && record.constructor.name === "Param" ? [record.value] : [];
    return [...own, ...(record.queryChunks ?? []).flatMap(conditionValues)];
  };
  const matchingId = (condition: unknown) => conditionValues(condition).find((value): value is string => typeof value === "string");

  const rowsFor = (table: object, selection: unknown, condition: unknown) => {
    const name = tableName(table);
    const id = matchingId(condition);
    if (name === "agent_tasks") {
      const tasks = id ? state.tasks.filter((task) => task.id === id) : state.tasks;
      return selection
        ? tasks.map((task) => ({ task, projectName: "Project", modelName: "Model", providerName: "Provider" }))
        : tasks;
    }
    if (name === "projects") return [project];
    if (name === "agent_messages") return id ? state.messages.filter((message) => message.taskId === id) : state.messages;
    if (name === "agent_runs") {
      const runs = id ? state.runs.filter((run) => run.taskId === id || run.id === id || run.status === id) : state.runs;
      return selection ? runs.map((run) => ({ id: run.id })) : runs;
    }
    if (name === "agent_events") return state.events;
    return [];
  };

  class SelectQuery implements PromiseLike<unknown[]> {
    private table?: object;
    private condition?: unknown;
    constructor(private readonly selection?: unknown) {}
    from(table: object) { this.table = table; return this; }
    innerJoin() { return this; }
    where(condition: unknown) { this.condition = condition; return this; }
    orderBy() { return this; }
    limit(count: number) { return Promise.resolve(rowsFor(this.table!, this.selection, this.condition).slice(0, count)); }
    then<TResult1 = unknown[], TResult2 = never>(
      onfulfilled?: ((value: unknown[]) => TResult1 | PromiseLike<TResult1>) | null,
      onrejected?: ((reason: unknown) => TResult2 | PromiseLike<TResult2>) | null
    ) { return Promise.resolve(rowsFor(this.table!, this.selection, this.condition)).then(onfulfilled, onrejected); }
  }

  const insert = vi.fn((table: object) => ({
    values(values: Record<string, unknown>) {
      const name = tableName(table);
      const row = { ...values } as never;
      if (name === "agent_messages") state.messages.push(row);
      if (name === "agent_runs") state.runs.push(row);
      if (name === "agent_events") state.events.push(row);
      const promise = Promise.resolve(undefined);
      return Object.assign(promise, {
        returning: () => Promise.resolve([row]),
        onConflictDoNothing: () => Promise.resolve(undefined)
      });
    }
  }));

  const update = vi.fn((table: object) => ({
    set(patch: Record<string, unknown>) {
      return {
        where(condition: unknown) {
          const id = matchingId(condition);
          const updated: Array<Record<string, unknown>> = [];
          if (tableName(table) === "agent_tasks") {
            for (const task of state.tasks.filter((candidate) => !id || candidate.id === id || candidate.status === id)) {
              Object.assign(task, patch);
              updated.push(task);
            }
          }
          if (tableName(table) === "agent_runs") {
            for (const run of state.runs.filter((candidate) => !id || candidate.id === id || candidate.status === id)) {
              Object.assign(run, patch);
              updated.push(run);
            }
          }
          const promise = Promise.resolve(updated);
          return Object.assign(promise, { returning: () => Promise.resolve(updated) });
        }
      };
    }
  }));

  const db = {
    select: vi.fn((selection?: unknown) => new SelectQuery(selection)),
    insert,
    update,
    delete: vi.fn((table: object) => ({
      where: vi.fn(async (condition: unknown) => {
        const id = matchingId(condition);
        if (tableName(table) === "agent_tasks" && id) {
          state.tasks = state.tasks.filter((candidate) => candidate.id !== id);
          state.messages = state.messages.filter((candidate) => candidate.taskId !== id);
          state.runs = state.runs.filter((candidate) => candidate.taskId !== id);
        }
      })
    })),
    transaction: vi.fn()
  };
  db.transaction.mockImplementation(async (callback: (tx: unknown) => unknown) => callback(db));
  return {
    state,
    db,
    resolveProviderModel: vi.fn(),
    prepareTaskWorktree: vi.fn(),
    cleanupTaskWorktree: vi.fn(),
    commitTaskWorktree: vi.fn(),
    pushTaskWorktree: vi.fn(),
    taskGitPreview: vi.fn(),
    runAgentProvider: vi.fn(),
    runCodexAccount: vi.fn(),
    sandboxStart: vi.fn(async () => undefined),
    sandboxStop: vi.fn(async () => undefined),
    runCommand: vi.fn(async () => ({ exitCode: 0, output: "" }))
  };
});

vi.mock("../src/server/db/index.js", () => ({ db: mocks.db }));
vi.mock("../src/server/services/ai-providers.js", () => ({ resolveProviderModel: mocks.resolveProviderModel }));
vi.mock("../src/server/services/agent-worktrees.js", () => ({
  cleanupTaskWorktree: mocks.cleanupTaskWorktree,
  commitTaskWorktree: mocks.commitTaskWorktree,
  fetchProjectBranches: vi.fn(),
  prepareTaskWorktree: mocks.prepareTaskWorktree,
  pushTaskWorktree: mocks.pushTaskWorktree,
  taskGitPreview: mocks.taskGitPreview
}));
vi.mock("../src/server/services/agent-provider-runner.js", () => ({ runAgentProvider: mocks.runAgentProvider }));
vi.mock("../src/server/services/codex-account-runner.js", () => ({ runCodexAccount: mocks.runCodexAccount }));
vi.mock("../src/server/services/agent-tools.js", () => ({
  AgentSandbox: class {
    start = mocks.sandboxStart;
    stop = mocks.sandboxStop;
  }
}));
vi.mock("../src/server/services/commands.js", () => ({ runCommand: mocks.runCommand }));
vi.mock("../src/server/logger.js", () => ({ logger: { error: vi.fn() } }));

const project: ProjectRow = {
  id: "prj_test", name: "Project", gitUrl: "git@example.test:project.git", branch: "main", folderName: "project",
  localPath: "/projects/project", composeFile: "compose.yml", composeContent: null, envFile: ".env", autoStart: false,
  manualDeployEnabled: true, githubWebhookEnabled: true, targetNodeId: "node_master_local", deployToken: "token",
  sshPrivateKeyPath: null, sshPublicKey: null, agentImage: "", createdAt: new Date(), updatedAt: new Date()
};

function task(id: string, overrides: Partial<AgentTaskRow> = {}): AgentTaskRow {
  return {
    id, projectId: project.id, modelId: "aim_test", title: id, prompt: "Initial instruction", status: "backlog",
    sourceBranch: "main", taskBranch: `task/${id}`, sourceSha: null, worktreePath: null, codexThreadId: null,
    resumeExistingBranch: false, autoCommit: false, autoPush: false, autoCleanup: false, lastError: "previous error",
    createdAt: new Date("2026-01-01T00:00:00Z"), updatedAt: new Date("2026-01-01T00:00:00Z"), startedAt: null,
    finishedAt: null, pushedAt: null, ...overrides
  };
}

function run(id: string, taskId: string, overrides: Partial<AgentRunRow> = {}): AgentRunRow {
  return {
    id, taskId, status: "running", providerProtocol: "openai_responses", modelName: "test-model",
    assistantText: "", error: null, startedAt: new Date("2026-01-01T00:00:00Z"), finishedAt: null,
    ...overrides
  };
}

const resolvedModel = {
  provider: { id: "aip_test", protocol: "openai_responses", baseUrl: "https://example.test", name: "Provider" },
  model: { id: "aim_test", modelId: "test-model", displayName: "Model" },
  apiKey: "fake-test-key"
};

async function loadService() {
  vi.resetModules();
  return import("../src/server/services/agent-tasks.js");
}

describe("agent task orchestration", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.state.tasks = [task("agt_one")];
    mocks.state.messages = [];
    mocks.state.runs = [];
    mocks.state.events = [];
    mocks.resolveProviderModel.mockResolvedValue(resolvedModel);
    mocks.prepareTaskWorktree.mockImplementation(() => new Promise(() => undefined));
    mocks.runAgentProvider.mockImplementation(() => new Promise(() => undefined));
    mocks.runCommand.mockResolvedValue({ exitCode: 0, output: "" });
    mocks.db.transaction.mockImplementation(async (callback: (tx: unknown) => unknown) => callback(mocks.db));
    mocks.cleanupTaskWorktree.mockResolvedValue(undefined);
    mocks.commitTaskWorktree.mockResolvedValue("commit-sha");
    mocks.pushTaskWorktree.mockResolvedValue("pushed-sha");
    mocks.taskGitPreview.mockResolvedValue({ isClean: true });
  });

  it("admits exactly one of two simultaneous starts for the same task", async () => {
    const { startAgentTask } = await loadService();

    const results = await Promise.allSettled([
      startAgentTask("agt_one", "first follow-up"),
      startAgentTask("agt_one", "second follow-up")
    ]);

    expect(results.filter((result) => result.status === "fulfilled")).toHaveLength(1);
    expect(results.filter((result) => result.status === "rejected")).toHaveLength(1);
    expect(mocks.state.runs.filter((run) => run.status === "running")).toHaveLength(1);
    expect(mocks.state.messages.filter((message) => message.role === "user")).toHaveLength(1);
    expect(mocks.state.messages[0]?.runId).toBe(mocks.state.runs[0]?.id);
  });

  it("reserves the final global capacity slot before a concurrent start can pass admission", async () => {
    const { startAgentTask } = await loadService();
    const { config } = await import("../src/server/config.js");
    const previousCapacity = config.agentMaxConcurrentRuns;
    config.agentMaxConcurrentRuns = 1;
    mocks.state.tasks = [task("agt_one"), task("agt_two")];
    try {
      const results = await Promise.allSettled([startAgentTask("agt_one"), startAgentTask("agt_two")]);

      expect(results.filter((result) => result.status === "fulfilled")).toHaveLength(1);
      const rejected = results.find((result) => result.status === "rejected") as PromiseRejectedResult;
      expect(rejected.reason).toMatchObject({ status: 429 });
      expect(mocks.state.runs.filter((run) => run.status === "running")).toHaveLength(1);
    } finally {
      config.agentMaxConcurrentRuns = previousCapacity;
    }
  });

  it("does not persist a follow-up or mutate task state when provider resolution fails", async () => {
    const original = { ...mocks.state.tasks[0] };
    mocks.resolveProviderModel.mockRejectedValueOnce(new Error("Provider is unavailable."));
    const { startAgentTask } = await loadService();

    await expect(startAgentTask("agt_one", "must not be orphaned")).rejects.toThrow("Provider is unavailable.");

    expect(mocks.state.messages).toEqual([]);
    expect(mocks.state.runs).toEqual([]);
    expect(mocks.state.tasks[0]).toEqual(original);
  });

  it("releases a reserved capacity slot when transactional admission fails", async () => {
    const { startAgentTask } = await loadService();
    const { config } = await import("../src/server/config.js");
    const previousCapacity = config.agentMaxConcurrentRuns;
    config.agentMaxConcurrentRuns = 1;
    mocks.state.tasks = [task("agt_one"), task("agt_two")];
    mocks.db.transaction.mockRejectedValueOnce(new Error("database unavailable"));
    try {
      await expect(startAgentTask("agt_one")).rejects.toThrow("database unavailable");
      await expect(startAgentTask("agt_two")).resolves.toMatchObject({ taskId: "agt_two", status: "running" });
    } finally {
      config.agentMaxConcurrentRuns = previousCapacity;
    }
  });

  it("translates the running-run unique conflict to the existing 409 response", async () => {
    mocks.db.transaction.mockRejectedValueOnce(Object.assign(new Error("duplicate key"), { code: "23505" }));
    const { startAgentTask } = await loadService();

    await expect(startAgentTask("agt_one", "not accepted")).rejects.toMatchObject({
      status: 409,
      message: "Task already has an active run."
    });

    expect(mocks.state.messages).toEqual([]);
    expect(mocks.state.runs).toEqual([]);
  });

  it("stopping during worktree preparation prevents either runner from starting", async () => {
    let finishPreparation!: (value: { worktreePath: string; sourceSha: string }) => void;
    mocks.prepareTaskWorktree.mockImplementationOnce(() => new Promise((resolve) => { finishPreparation = resolve; }));
    const { startAgentTask, stopAgentTask } = await loadService();
    await startAgentTask("agt_one");
    await vi.waitFor(() => expect(mocks.prepareTaskWorktree).toHaveBeenCalledOnce());

    await stopAgentTask("agt_one");
    finishPreparation({ worktreePath: "/worktrees/agt_one", sourceSha: "abc123" });
    await new Promise((resolve) => setImmediate(resolve));

    expect(mocks.runAgentProvider).not.toHaveBeenCalled();
    expect(mocks.runCodexAccount).not.toHaveBeenCalled();
  });

  it("keeps the task running until provider cleanup has completed", async () => {
    let releaseCleanup!: () => void;
    mocks.prepareTaskWorktree.mockResolvedValueOnce({ worktreePath: "/worktrees/agt_one", sourceSha: "abc123" });
    mocks.runAgentProvider.mockResolvedValueOnce("Finished");
    mocks.sandboxStop.mockImplementationOnce(() => new Promise<void>((resolve) => { releaseCleanup = resolve; }));
    const { startAgentTask } = await loadService();

    await startAgentTask("agt_one");
    await vi.waitFor(() => expect(mocks.runAgentProvider).toHaveBeenCalledOnce());
    expect(mocks.state.tasks[0].status).toBe("running");

    releaseCleanup();
    await vi.waitFor(() => expect(mocks.state.tasks[0].status).toBe("review"));
    expect(mocks.state.runs[0].status).toBe("succeeded");
  });

  it("keeps ownership locked and retries a transient terminal database failure", async () => {
    let transactionCount = 0;
    let rejectTerminalWrite!: () => void;
    mocks.db.transaction.mockImplementation(async (callback: (tx: unknown) => unknown) => {
      transactionCount += 1;
      if (transactionCount === 2) {
        await new Promise<void>((_resolve, reject) => {
          rejectTerminalWrite = () => reject(new Error("transient terminal write failure"));
        });
      }
      return callback(mocks.db);
    });
    mocks.prepareTaskWorktree.mockResolvedValueOnce({ worktreePath: "/worktrees/agt_one", sourceSha: "abc123" });
    mocks.runAgentProvider.mockResolvedValueOnce("Agent result survives");
    const { startAgentTask } = await loadService();

    const accepted = await startAgentTask("agt_one");
    await vi.waitFor(() => expect(transactionCount).toBeGreaterThanOrEqual(2));
    expect(mocks.state.tasks[0].status).toBe("running");
    await expect(startAgentTask("agt_one", "must not be accepted yet")).rejects.toMatchObject({ status: 409 });
    rejectTerminalWrite();

    await vi.waitFor(() => expect(mocks.state.tasks[0].status).toBe("review"));
    expect(mocks.state.runs.find((candidate) => candidate.id === accepted.id)).toMatchObject({
      status: "succeeded",
      assistantText: "Agent result survives",
      error: null
    });
    expect(mocks.state.messages.filter((message) => message.role === "assistant")).toEqual([
      expect.objectContaining({ runId: accepted.id, content: "Agent result survives" })
    ]);
  });

  it("preserves agent success and the worktree when automatic Git push fails", async () => {
    mocks.state.tasks[0] = task("agt_one", { autoCommit: true, autoPush: true });
    mocks.prepareTaskWorktree.mockResolvedValueOnce({ worktreePath: "/worktrees/agt_one", sourceSha: "abc123" });
    mocks.runAgentProvider.mockResolvedValueOnce("Implementation completed");
    mocks.taskGitPreview.mockResolvedValueOnce({ isClean: false });
    mocks.pushTaskWorktree.mockRejectedValueOnce(new Error("origin unavailable"));
    const { startAgentTask } = await loadService();

    const accepted = await startAgentTask("agt_one");

    await vi.waitFor(() => expect(mocks.state.tasks[0].status).toBe("review"));
    expect(mocks.state.tasks[0]).toMatchObject({
      worktreePath: "/worktrees/agt_one",
      lastError: "Git automation failed: origin unavailable"
    });
    expect(mocks.state.runs.find((candidate) => candidate.id === accepted.id)).toMatchObject({
      status: "succeeded",
      assistantText: "Implementation completed",
      error: null
    });
    expect(mocks.state.messages).toContainEqual(expect.objectContaining({
      runId: accepted.id,
      role: "assistant",
      content: "Implementation completed"
    }));
    expect(mocks.cleanupTaskWorktree).not.toHaveBeenCalled();
  });

  it("requires a clean post-push worktree before marking an automatic run done", async () => {
    mocks.state.tasks[0] = task("agt_one", { autoCommit: true, autoPush: true });
    mocks.prepareTaskWorktree.mockResolvedValueOnce({ worktreePath: "/worktrees/agt_one", sourceSha: "abc123" });
    mocks.runAgentProvider.mockResolvedValueOnce("Implementation completed");
    mocks.taskGitPreview
      .mockResolvedValueOnce({ isClean: true })
      .mockResolvedValueOnce({ isClean: false });
    const { startAgentTask } = await loadService();

    await startAgentTask("agt_one");

    await vi.waitFor(() => expect(mocks.state.tasks[0].status).toBe("review"));
    expect(mocks.state.tasks[0].pushedAt).toBeInstanceOf(Date);
    expect(mocks.state.tasks[0].lastError).toBe("Git automation failed: Branch pushed, but uncommitted changes remain in the worktree.");
    expect(mocks.state.runs[0].status).toBe("succeeded");
  });

  it("marks an automatic run done only after push, cleanliness verification, and requested cleanup", async () => {
    mocks.state.tasks[0] = task("agt_one", { autoCommit: true, autoPush: true, autoCleanup: true });
    mocks.prepareTaskWorktree.mockResolvedValueOnce({ worktreePath: "/worktrees/agt_one", sourceSha: "abc123" });
    mocks.runAgentProvider.mockResolvedValueOnce("Implementation completed");
    mocks.taskGitPreview.mockResolvedValue({ isClean: true });
    const { startAgentTask } = await loadService();

    await startAgentTask("agt_one");

    await vi.waitFor(() => expect(mocks.state.tasks[0].status).toBe("done"));
    expect(mocks.pushTaskWorktree).toHaveBeenCalledOnce();
    expect(mocks.taskGitPreview).toHaveBeenCalledTimes(2);
    expect(mocks.cleanupTaskWorktree).toHaveBeenCalledOnce();
    expect(mocks.state.tasks[0]).toMatchObject({ worktreePath: null, lastError: null });
  });

  it("does not clear lastError during a manual push outside accepted-run admission", async () => {
    mocks.state.tasks[0] = task("agt_one", {
      status: "review",
      worktreePath: "/worktrees/agt_one",
      lastError: "Previous actionable error"
    });
    mocks.taskGitPreview.mockResolvedValueOnce({ isClean: true });
    const { pushAgentTask } = await loadService();

    await expect(pushAgentTask("agt_one")).resolves.toBe("pushed-sha");

    expect(mocks.state.tasks[0]).toMatchObject({
      status: "done",
      lastError: "Previous actionable error"
    });
  });

  it("keeps a manually pushed task in review when requested cleanup fails", async () => {
    mocks.state.tasks[0] = task("agt_one", {
      status: "review",
      worktreePath: "/worktrees/agt_one",
      autoCleanup: true
    });
    mocks.taskGitPreview.mockResolvedValueOnce({ isClean: true });
    mocks.cleanupTaskWorktree.mockRejectedValueOnce(new Error("worktree is busy"));
    const { pushAgentTask } = await loadService();

    await expect(pushAgentTask("agt_one")).rejects.toThrow("worktree is busy");

    expect(mocks.state.tasks[0]).toMatchObject({
      status: "review",
      worktreePath: "/worktrees/agt_one",
      lastError: "Git automation failed: worktree is busy"
    });
    expect(mocks.state.tasks[0].pushedAt).toBeInstanceOf(Date);
  });

  it("gracefully drains API-key and Codex-account runs", async () => {
    const codexStop = vi.fn(async () => undefined);
    mocks.state.tasks = [
      task("agt_api"),
      task("agt_codex", { modelId: "aim_codex" })
    ];
    mocks.prepareTaskWorktree.mockImplementation(async (_project, detail: AgentTaskRow) => ({
      worktreePath: `/worktrees/${detail.id}`,
      sourceSha: "abc123"
    }));
    mocks.resolveProviderModel.mockImplementation(async (modelId: string) => modelId === "aim_codex"
      ? { ...resolvedModel, provider: { ...resolvedModel.provider, protocol: "codex_account" } }
      : resolvedModel);
    mocks.runAgentProvider.mockImplementation(({ signal }: { signal: AbortSignal }) => new Promise((_resolve, reject) => {
      signal.addEventListener("abort", () => reject(signal.reason), { once: true });
    }));
    mocks.runCodexAccount.mockImplementation((input: {
      signal: AbortSignal;
      registerStop?: (stop: () => Promise<void>) => void;
    }) => new Promise((_resolve, reject) => {
      input.registerStop?.(codexStop);
      input.signal.addEventListener("abort", () => reject(input.signal.reason), { once: true });
    }));
    const { drainActiveAgentRuns, startAgentTask } = await loadService();

    await Promise.all([startAgentTask("agt_api"), startAgentTask("agt_codex")]);
    await vi.waitFor(() => {
      expect(mocks.runAgentProvider).toHaveBeenCalledOnce();
      expect(mocks.runCodexAccount).toHaveBeenCalledOnce();
    });

    await expect(drainActiveAgentRuns(1_000)).resolves.toEqual({ drained: true, activeRunCount: 2 });
    expect(codexStop).toHaveBeenCalledOnce();
    expect(mocks.state.runs.every((candidate) => candidate.status === "canceled")).toBe(true);
    expect(mocks.state.tasks.every((candidate) => candidate.status === "review")).toBe(true);
  });

  it("removes only labeled containers for running rows before startup interruption", async () => {
    mocks.state.tasks = [task("agt_one", { status: "running" })];
    mocks.state.runs = [run("agr_running", "agt_one")];
    let finishRemoval!: () => void;
    mocks.runCommand.mockImplementation(async (_command: string, args: string[]) => {
      if (args[0] === "ps") return { exitCode: 0, output: "ctr_running\tagr_running\nctr_unowned\tagr_not_running\n" };
      if (args[0] === "rm") {
        await new Promise<void>((resolve) => { finishRemoval = resolve; });
        return { exitCode: 0, output: "ctr_running" };
      }
      throw new Error(`Unexpected command: ${args.join(" ")}`);
    });
    const { recoverInterruptedAgentRuns } = await loadService();

    const recovery = recoverInterruptedAgentRuns();
    await vi.waitFor(() => expect(mocks.runCommand).toHaveBeenCalledTimes(2));
    expect(mocks.state.runs[0].status).toBe("running");
    finishRemoval();

    await expect(recovery).resolves.toBe(1);
    expect(mocks.runCommand).toHaveBeenNthCalledWith(1, "docker", expect.arrayContaining([
      "--filter", "label=com.yanto.agent=true"
    ]), expect.any(Object));
    expect(mocks.runCommand).toHaveBeenNthCalledWith(2, "docker", ["rm", "-f", "ctr_running"], expect.any(Object));
    expect(mocks.state.runs[0].status).toBe("failed");
    expect(mocks.state.tasks[0].status).toBe("review");
  });

  it("keeps interrupted ownership locked when orphan container removal fails", async () => {
    mocks.state.tasks = [task("agt_one", { status: "running" })];
    mocks.state.runs = [run("agr_running", "agt_one")];
    mocks.runCommand.mockImplementation(async (_command: string, args: string[]) => {
      if (args[0] === "ps") return { exitCode: 0, output: "ctr_running\tagr_running\n" };
      if (args[0] === "rm") return { exitCode: 1, output: "daemon unavailable" };
      throw new Error(`Unexpected command: ${args.join(" ")}`);
    });
    const { recoverInterruptedAgentRuns } = await loadService();

    await expect(recoverInterruptedAgentRuns()).rejects.toThrow("Unable to recover interrupted agent run agr_running: daemon unavailable");

    expect(mocks.state.runs[0]).toMatchObject({ status: "running", error: null, finishedAt: null });
    expect(mocks.state.tasks[0]).toMatchObject({ status: "running", lastError: "previous error", finishedAt: null });
  });

  it("rejects deletion of a running task before touching its worktree or row", async () => {
    mocks.state.tasks[0].status = "running";
    const { deleteAgentTask } = await loadService();

    await expect(deleteAgentTask("agt_one")).rejects.toMatchObject({ status: 409 });

    expect(mocks.db.delete).not.toHaveBeenCalled();
  });

  it("does not admit a run while task deletion is cleaning its worktree", async () => {
    mocks.state.tasks[0] = task("agt_one", { status: "review", worktreePath: "/worktrees/agt_one" });
    let releaseCleanup!: () => void;
    mocks.cleanupTaskWorktree.mockImplementationOnce(() => new Promise<void>((resolve) => { releaseCleanup = resolve; }));
    const { deleteAgentTask, startAgentTask } = await loadService();

    const deletion = deleteAgentTask("agt_one");
    await vi.waitFor(() => expect(mocks.cleanupTaskWorktree).toHaveBeenCalledOnce());
    const start = startAgentTask("agt_one", "must not be accepted");
    let startSettled = false;
    void start.finally(() => { startSettled = true; }).catch(() => undefined);
    await new Promise((resolve) => setImmediate(resolve));
    expect(startSettled).toBe(false);

    releaseCleanup();
    await deletion;
    await expect(start).rejects.toMatchObject({ status: 404, message: "Task not found." });
    expect(mocks.state.runs).toEqual([]);
    expect(mocks.state.messages).toEqual([]);
  });
});
