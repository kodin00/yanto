import { and, asc, desc, eq, inArray, isNotNull, isNull, lte, ne } from "drizzle-orm";
import path from "node:path";
import { db } from "../db/index.js";
import { agentEvents, agentMessages, agentRuns, agentTasks, aiModels, aiProviders, projects, type AgentTaskRow } from "../db/schema.js";
import { config } from "../config.js";
import { HttpError } from "../http-utils.js";
import { logger } from "../logger.js";
import { createId } from "./tokens.js";
import { resolveProviderModel } from "./ai-providers.js";
import { AgentWorkspace } from "./agent-tools.js";
import { runAgentProvider } from "./agent-provider-runner.js";
import { runCodexAccount } from "./codex-account-runner.js";
import { compactPersistedAgentEvent } from "./agent-event-payload.js";
import { agentEventBus } from "./agent-events.js";
import { agentProjectLifecycleKey, agentTaskLifecycleKey, withAgentLifecycleLock } from "./agent-lifecycle.js";
import { cleanupTaskWorktree, commitTaskWorktree, fetchProjectBranches, prepareTaskWorktree, pushTaskWorktree, taskGitPreview } from "./agent-worktrees.js";
import { pathExists } from "./paths.js";
import { runCommand } from "./commands.js";

type CreateAgentTaskInput = {
  projectId: string;
  modelId: string;
  title: string;
  prompt: string;
  sourceBranch: string;
  taskBranch: string;
  resumeExistingBranch?: boolean;
  autoCommit?: boolean;
  autoPush?: boolean;
  autoCleanup?: boolean;
};

type ActiveAgentRun = {
  runId: string;
  taskId: string;
  controller: AbortController;
  stop: () => Promise<void>;
  completion: Promise<void>;
};

type ActiveAgentRunState = {
  runnerStop: () => Promise<void>;
  stopPromise?: Promise<void>;
  resolveCompletion: () => void;
};

const activeRuns = new Map<string, ActiveAgentRun>();
const activeRunStates = new WeakMap<ActiveAgentRun, ActiveAgentRunState>();
const eventSequences = new Map<string, number>();

function createActiveAgentRun(taskId: string, runId: string, controller: AbortController) {
  let resolveCompletion!: () => void;
  const completion = new Promise<void>((resolve) => { resolveCompletion = resolve; });
  const state: ActiveAgentRunState = { runnerStop: async () => undefined, resolveCompletion };
  const handle: ActiveAgentRun = {
    runId,
    taskId,
    controller,
    completion,
    stop: () => {
      return state.stopPromise ??= (async () => {
        if (!controller.signal.aborted) controller.abort(new Error("Stopped by user."));
        await state.runnerStop();
      })();
    }
  };
  activeRunStates.set(handle, state);
  return handle;
}

function registerRunnerStop(handle: ActiveAgentRun, stop: () => Promise<void>) {
  const state = activeRunStates.get(handle);
  if (state) state.runnerStop = stop;
}

function completeActiveAgentRun(handle: ActiveAgentRun) {
  activeRunStates.get(handle)?.resolveCompletion();
}

function isActiveRunUniqueConflict(error: unknown) {
  const pgError = error as { code?: string; constraint?: string };
  return pgError.code === "23505"
    && (!pgError.constraint || pgError.constraint === "agent_runs_one_running_per_task_idx");
}

function taskView(row: {
  task: AgentTaskRow;
  projectName: string;
  modelName: string;
  providerName: string;
}, latestRun: typeof agentRuns.$inferSelect | null = null) {
  return { ...row.task, projectName: row.projectName, modelName: row.modelName, providerName: row.providerName, latestRun };
}

async function taskRows(taskId?: string, archived?: boolean) {
  const query = db.select({ task: agentTasks, projectName: projects.name, modelName: aiModels.displayName, providerName: aiProviders.name })
    .from(agentTasks)
    .innerJoin(projects, eq(agentTasks.projectId, projects.id))
    .innerJoin(aiModels, eq(agentTasks.modelId, aiModels.id))
    .innerJoin(aiProviders, eq(aiModels.providerId, aiProviders.id));
  if (taskId) return query.where(eq(agentTasks.id, taskId)).limit(1);
  if (archived === true) return query.where(isNotNull(agentTasks.archivedAt)).orderBy(desc(agentTasks.archivedAt));
  if (archived === false) return query.where(isNull(agentTasks.archivedAt)).orderBy(desc(agentTasks.createdAt));
  return query.orderBy(desc(agentTasks.createdAt));
}

export async function archiveCompletedAgentTasks(now = new Date()) {
  const cutoff = new Date(now.getTime() - (3 * 24 * 60 * 60 * 1_000));
  const archived = await db.update(agentTasks).set({ archivedAt: now })
    .where(and(eq(agentTasks.status, "done"), isNull(agentTasks.archivedAt), lte(agentTasks.updatedAt, cutoff)))
    .returning({ id: agentTasks.id });
  return archived.length;
}

export async function listAgentTasks(archived = false) {
  const rows = await taskRows(undefined, archived);
  const runs = rows.length
    ? await db.selectDistinctOn([agentRuns.taskId]).from(agentRuns)
      .where(inArray(agentRuns.taskId, rows.map((row) => row.task.id)))
      .orderBy(agentRuns.taskId, desc(agentRuns.startedAt), desc(agentRuns.id))
    : [];
  const latestRunByTask = new Map(runs.map((run) => [run.taskId, run]));
  return rows.map((row) => taskView(row, latestRunByTask.get(row.task.id) ?? null));
}

function displayWorktreePath(worktreePath: string) {
  for (const [internalRoot, hostRoot] of [
    [config.agentWorktreesRoot, config.hostAgentWorktreesRoot],
    [config.projectsRoot, config.hostProjectsRoot]
  ] as const) {
    const relative = path.relative(internalRoot, worktreePath);
    if (!path.isAbsolute(relative) && relative !== ".." && !relative.startsWith(`..${path.sep}`)) {
      return path.join(hostRoot, relative);
    }
  }
  return worktreePath;
}

export async function listAgentWorktrees() {
  const rows = await db.select({
    task: agentTasks,
    projectName: projects.name
  }).from(agentTasks)
    .innerJoin(projects, eq(agentTasks.projectId, projects.id))
    .where(isNotNull(agentTasks.worktreePath))
    .orderBy(desc(agentTasks.updatedAt));
  return Promise.all(rows.map(async ({ task, projectName }) => ({
    taskId: task.id,
    taskTitle: task.title,
    projectId: task.projectId,
    projectName,
    status: task.status,
    path: task.worktreePath!,
    hostPath: displayWorktreePath(task.worktreePath!),
    branch: task.taskBranch,
    updatedAt: task.updatedAt,
    finishedAt: task.finishedAt,
    archivedAt: task.archivedAt,
    exists: await pathExists(task.worktreePath!),
    removable: task.status !== "running"
  })));
}

export async function getAgentTask(id: string) {
  const [row] = await taskRows(id);
  if (!row) return undefined;
  const [messages, runs] = await Promise.all([
    db.select().from(agentMessages).where(eq(agentMessages.taskId, id)).orderBy(asc(agentMessages.createdAt)),
    db.select().from(agentRuns).where(eq(agentRuns.taskId, id)).orderBy(desc(agentRuns.startedAt))
  ]);
  const events = runs[0]
    ? await db.select().from(agentEvents).where(eq(agentEvents.runId, runs[0].id)).orderBy(asc(agentEvents.sequence))
    : [];
  return { ...taskView(row, runs[0] ?? null), messages, runs, events };
}

export async function createAgentTask(input: CreateAgentTaskInput) {
  const [project] = await db.select().from(projects).where(eq(projects.id, input.projectId)).limit(1);
  if (!project) throw new HttpError(404, "Project not found.");
  if (!project.gitUrl) throw new HttpError(400, "AI tasks require a Git-backed project.");
  await resolveProviderModel(input.modelId);
  if (!input.title.trim() || !input.prompt.trim()) throw new HttpError(400, "Task title and instructions are required.");
  if (input.autoPush && !input.autoCommit) throw new HttpError(400, "Auto-push requires auto-commit.");
  if (input.autoCleanup && !input.autoPush) throw new HttpError(400, "Auto-cleanup requires auto-push.");
  const id = createId("agt");
  try {
    const [task] = await db.insert(agentTasks).values({
      id,
      projectId: project.id,
      modelId: input.modelId,
      title: input.title.trim(),
      prompt: input.prompt.trim(),
      sourceBranch: input.sourceBranch.trim(),
      taskBranch: input.taskBranch.trim(),
      resumeExistingBranch: input.resumeExistingBranch ?? false,
      autoCommit: input.autoCommit ?? false,
      autoPush: input.autoPush ?? false,
      autoCleanup: input.autoCleanup ?? false,
      status: "backlog",
      createdAt: new Date(),
      updatedAt: new Date()
    }).returning();
    await db.insert(agentMessages).values({ id: createId("agm"), taskId: id, role: "user", content: task.prompt, createdAt: new Date() });
    return (await getAgentTask(task.id))!;
  } catch (error) {
    if ((error as { code?: string }).code === "23505") throw new HttpError(409, "That task branch is already assigned to another task in this project.");
    throw error;
  }
}

export async function updateAgentTask(id: string, input: Partial<Pick<CreateAgentTaskInput, "title" | "modelId" | "autoCommit" | "autoPush" | "autoCleanup">>) {
  const initial = await getAgentTask(id);
  if (!initial) return undefined;
  if (initial.status === "running") throw new HttpError(409, "Stop the active run before changing task settings.");
  if (input.modelId) await resolveProviderModel(input.modelId);
  return withAgentLifecycleLock([
    agentProjectLifecycleKey(initial.projectId),
    agentTaskLifecycleKey(id)
  ], async () => {
    const current = await getAgentTask(id);
    if (!current) return undefined;
    if (current.status === "running") throw new HttpError(409, "Stop the active run before changing task settings.");
    const autoCommit = input.autoCommit ?? current.autoCommit;
    const autoPush = input.autoPush ?? current.autoPush;
    const autoCleanup = input.autoCleanup ?? current.autoCleanup;
    if (autoPush && !autoCommit) throw new HttpError(400, "Auto-push requires auto-commit.");
    if (autoCleanup && !autoPush) throw new HttpError(400, "Auto-cleanup requires auto-push.");
    await db.update(agentTasks).set({
      ...(input.title !== undefined ? { title: input.title.trim() } : {}),
      ...(input.modelId !== undefined ? { modelId: input.modelId } : {}),
      autoCommit, autoPush, autoCleanup, updatedAt: new Date()
    }).where(eq(agentTasks.id, id));
    return getAgentTask(id);
  });
}

export async function setAgentTaskArchived(id: string, archived: boolean) {
  const initial = await getAgentTask(id);
  if (!initial) return undefined;
  return withAgentLifecycleLock([
    agentProjectLifecycleKey(initial.projectId),
    agentTaskLifecycleKey(id)
  ], async () => {
    const current = await getAgentTask(id);
    if (!current) return undefined;
    if (current.status === "running") throw new HttpError(409, "Stop the active run before archiving the task.");
    if (archived && current.status !== "done") throw new HttpError(409, "Only done tasks can be archived.");
    await db.update(agentTasks).set({ archivedAt: archived ? new Date() : null, updatedAt: new Date() }).where(eq(agentTasks.id, id));
    return getAgentTask(id);
  });
}

export async function deleteAgentTask(id: string, force = false) {
  const initial = await getAgentTask(id);
  if (!initial) return;
  await withAgentLifecycleLock([
    agentProjectLifecycleKey(initial.projectId),
    agentTaskLifecycleKey(id)
  ], async () => {
    const detail = await getAgentTask(id);
    if (!detail) return;
    if (detail.status === "running") throw new HttpError(409, "Stop the active run before deleting the task.");
    const [project] = await db.select().from(projects).where(eq(projects.id, detail.projectId)).limit(1);
    if (project) await cleanupTaskWorktree(project, detail, force);
    await db.delete(agentTasks).where(eq(agentTasks.id, id));
  });
}

async function recordEvent(taskId: string, runId: string, kind: string, payload: Record<string, unknown>, done = false) {
  const sequence = (eventSequences.get(runId) ?? 0) + 1;
  eventSequences.set(runId, sequence);
  try {
    if (kind !== "assistant_delta") {
      const persistedPayload = compactPersistedAgentEvent(kind, payload, config.agentPersistedToolPayloadMaxBytes);
      await db.insert(agentEvents).values({ id: createId("age"), runId, sequence, kind, payload: persistedPayload, createdAt: new Date() });
    }
  } finally {
    // A live subscriber should still see the event when persistence fails. The
    // caller receives the database error and can fail/reconcile the run safely.
    agentEventBus.publish({ taskId, runId, sequence, kind, payload, done });
  }
}

type TerminalRunState = {
  runStatus: "succeeded" | "failed" | "canceled";
  taskStatus: "review" | "done";
  assistantText: string;
  runError?: string;
  taskError?: string;
  pushedAt?: Date;
  worktreeCleaned?: boolean;
};

async function writeTerminalRunState(
  taskId: string,
  runId: string,
  state: TerminalRunState,
  finishedAt: Date,
  assistantMessageId: string
) {
  await db.transaction(async (tx) => {
    await tx.update(agentRuns).set({
      status: state.runStatus,
      assistantText: state.assistantText,
      error: state.runError ?? null,
      finishedAt
    }).where(eq(agentRuns.id, runId));
    await tx.update(agentTasks).set({
      status: state.taskStatus,
      lastError: state.taskError ?? null,
      finishedAt,
      updatedAt: finishedAt,
      ...(state.pushedAt ? { pushedAt: state.pushedAt } : {}),
      ...(state.worktreeCleaned ? { worktreePath: null } : {})
    }).where(eq(agentTasks.id, taskId));
    if (state.assistantText.trim()) {
      await tx.insert(agentMessages).values({
        id: assistantMessageId,
        taskId,
        runId,
        role: "assistant",
        content: state.assistantText.trim(),
        createdAt: finishedAt
      }).onConflictDoNothing({ target: agentMessages.id });
    }
  });
}

async function reconcileTerminalRun(taskId: string, runId: string, state: TerminalRunState) {
  const finishedAt = new Date();
  const assistantMessageId = `agm_${runId}`;
  let retry = 0;
  for (;;) {
    try {
      await writeTerminalRunState(taskId, runId, state, finishedAt, assistantMessageId);
      return;
    } catch (error) {
      retry += 1;
      logger.error("agent terminal reconciliation failed", {
        taskId,
        runId,
        retry,
        error: error instanceof Error ? error.message : String(error)
      });
      // Keep the active handle and database run in `running` until ownership is
      // certain. A transient outage must not admit another owner for this task.
      await new Promise<void>((resolve) => {
        const timer = setTimeout(resolve, Math.min(1_000, 25 * (2 ** Math.min(retry - 1, 5))));
        timer.unref();
      });
    }
  }
}

async function recordPostRunEvent(taskId: string, runId: string, kind: string, payload: Record<string, unknown>, done = false) {
  try {
    await recordEvent(taskId, runId, kind, payload, done);
  } catch (error) {
    logger.error("agent post-run event persistence failed", {
      taskId,
      runId,
      kind,
      error: error instanceof Error ? error.message : String(error)
    });
  }
}

async function executeAgentRun(
  active: ActiveAgentRun,
  resolved: Awaited<ReturnType<typeof resolveProviderModel>>
) {
  const { taskId, runId, controller } = active;
  let workspace: AgentWorkspace | undefined;
  let assistantText = "";
  const timeout = setTimeout(() => controller.abort(new Error("Agent run timed out.")), config.agentRunTimeoutMs);
  timeout.unref();
  try {
    controller.signal.throwIfAborted();
    await recordEvent(taskId, runId, "run_started", { model: resolved.model.modelId, protocol: resolved.provider.protocol });
    const detail = await getAgentTask(taskId);
    if (!detail) throw new Error("Task not found.");
    const [project] = await db.select().from(projects).where(eq(projects.id, detail.projectId)).limit(1);
    if (!project) throw new Error("Project not found.");
    controller.signal.throwIfAborted();
    const prepared = await prepareTaskWorktree(project, detail);
    controller.signal.throwIfAborted();
    await db.update(agentTasks).set({ worktreePath: prepared.worktreePath, sourceSha: prepared.sourceSha, updatedAt: new Date() }).where(eq(agentTasks.id, taskId));
    await recordEvent(taskId, runId, "worktree_ready", { path: prepared.worktreePath, sourceSha: prepared.sourceSha, branch: detail.taskBranch });
    controller.signal.throwIfAborted();

    const messages = detail.messages.slice(-50).map((message) => ({ role: message.role === "assistant" ? "assistant" as const : "user" as const, content: message.content }));
    if (resolved.provider.protocol === "codex_account") {
      await recordEvent(taskId, runId, "workspace_started", { runtime: "codex", mode: "host" });
      const lastUserMessage = [...detail.messages].reverse().find((message) => message.role === "user")?.content ?? detail.prompt;
      const result = await runCodexAccount({
        runId, taskId, worktreePath: prepared.worktreePath, prompt: lastUserMessage, model: resolved.model.modelId,
        threadId: detail.codexThreadId, signal: controller.signal,
        registerStop: (stop) => registerRunnerStop(active, stop),
        event: async (kind, payload) => {
          if (kind === "codex_thread" && typeof payload.threadId === "string") {
            await db.update(agentTasks).set({ codexThreadId: payload.threadId, updatedAt: new Date() }).where(eq(agentTasks.id, taskId));
          }
          await recordEvent(taskId, runId, kind, payload);
        }
      });
      assistantText = result.assistantText;
      if (result.threadId) await db.update(agentTasks).set({ codexThreadId: result.threadId, updatedAt: new Date() }).where(eq(agentTasks.id, taskId));
    } else {
      workspace = new AgentWorkspace(prepared.worktreePath, { signal: controller.signal });
      registerRunnerStop(active, () => workspace!.stop());
      try {
        controller.signal.throwIfAborted();
        await workspace.start();
        controller.signal.throwIfAborted();
        await recordEvent(taskId, runId, "workspace_started", { runtime: "provider", mode: "host" });
        assistantText = await runAgentProvider({
          protocol: resolved.provider.protocol as never,
          baseUrl: resolved.provider.baseUrl,
          apiKey: resolved.apiKey,
          model: resolved.model.modelId,
          messages,
          workspace,
          signal: controller.signal,
          event: (kind, payload) => recordEvent(taskId, runId, kind, payload)
        });
      } finally {
        await workspace.stop();
      }
    }
    controller.signal.throwIfAborted();

    // Settings cannot change while a task is running, so the accepted snapshot
    // remains authoritative and avoids turning a transient post-provider read
    // failure into a false agent failure.
    const refreshed = { ...detail, worktreePath: prepared.worktreePath, sourceSha: prepared.sourceSha };
    let finalTaskStatus: "review" | "done" = "review";
    let taskError: string | undefined;
    let pushedAt: Date | undefined;
    let worktreeCleaned = false;
    try {
      if (refreshed.autoCommit) {
        const preview = await taskGitPreview(project, refreshed);
        if (!preview.isClean) {
          const sha = await commitTaskWorktree(project, refreshed, `task: ${refreshed.title}`);
          await recordPostRunEvent(taskId, runId, "git_committed", { sha, message: `task: ${refreshed.title}` });
        }
        if (refreshed.autoPush) {
          const sha = await pushTaskWorktree(project, refreshed);
          pushedAt = new Date();
          await recordPostRunEvent(taskId, runId, "git_pushed", { sha, branch: refreshed.taskBranch });
          const pushedPreview = await taskGitPreview(project, refreshed);
          if (!pushedPreview.isClean) {
            throw new Error("Branch pushed, but uncommitted changes remain in the worktree.");
          }
          finalTaskStatus = "done";
          if (refreshed.autoCleanup) {
            await cleanupTaskWorktree(project, refreshed);
            worktreeCleaned = true;
            await recordPostRunEvent(taskId, runId, "worktree_cleaned", {});
          }
        }
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      taskError = `Git automation failed: ${message}`;
      finalTaskStatus = "done";
      logger.error("agent Git automation failed", { taskId, runId, error: message });
    }

    await reconcileTerminalRun(taskId, runId, {
      runStatus: taskError ? "failed" : "succeeded",
      taskStatus: finalTaskStatus,
      assistantText,
      runError: taskError,
      taskError,
      pushedAt,
      worktreeCleaned
    });
    await recordPostRunEvent(taskId, runId, "run_finished", {
      status: taskError ? "failed" : "succeeded",
      error: taskError ?? null,
      assistantText
    });
    if (taskError) {
      await recordPostRunEvent(taskId, runId, "git_automation_failed", { error: taskError.slice("Git automation failed: ".length) }, true);
    } else {
      await recordPostRunEvent(taskId, runId, "task_finished", { status: finalTaskStatus }, true);
    }
  } catch (error) {
    const canceled = controller.signal.aborted;
    const message = error instanceof Error ? error.message : String(error);
    // Once provider execution succeeds, all later Git failures are contained by
    // the nested block above. Reaching this catch means provider/setup failure.
    await reconcileTerminalRun(taskId, runId, {
      runStatus: canceled ? "canceled" : "failed",
      taskStatus: "done",
      assistantText,
      runError: message,
      taskError: message
    });
    await recordPostRunEvent(taskId, runId, "run_finished", {
      status: canceled ? "canceled" : "failed",
      error: message,
      assistantText
    });
    await recordPostRunEvent(taskId, runId, "task_finished", { status: "done", error: message }, true);
    logger.error("agent run failed", { taskId, runId, canceled, error: message });
  } finally {
    clearTimeout(timeout);
    if (activeRuns.get(taskId) === active) activeRuns.delete(taskId);
    eventSequences.delete(runId);
    completeActiveAgentRun(active);
  }
}

export async function startAgentTask(id: string, followUp?: string) {
  const initial = await getAgentTask(id);
  if (!initial) throw new HttpError(404, "Task not found.");
  if (initial.status === "running") throw new HttpError(409, "Task already has an active run.");
  let resolved = await resolveProviderModel(initial.modelId);

  return withAgentLifecycleLock([
    agentProjectLifecycleKey(initial.projectId),
    agentTaskLifecycleKey(id)
  ], async () => {
    const task = await getAgentTask(id);
    if (!task) throw new HttpError(404, "Task not found.");
    if (task.status === "running") throw new HttpError(409, "Task already has an active run.");
    if (task.modelId !== initial.modelId) resolved = await resolveProviderModel(task.modelId);

    if (activeRuns.has(id)) throw new HttpError(409, "Task already has an active run.");
    if (activeRuns.size >= config.agentMaxConcurrentRuns) throw new HttpError(429, "Agent run capacity is full. Try again after another task finishes.");

    const runId = createId("agr");
    const controller = new AbortController();
    const active = createActiveAgentRun(id, runId, controller);
    activeRuns.set(id, active);

    let run: typeof agentRuns.$inferSelect;
    try {
      run = await db.transaction(async (tx) => {
        const now = new Date();
        const [claimedTask] = await tx.update(agentTasks).set({
          status: "running", archivedAt: null, lastError: null, startedAt: now, finishedAt: null, updatedAt: now
        }).where(and(eq(agentTasks.id, id), ne(agentTasks.status, "running"))).returning({ id: agentTasks.id });
        if (!claimedTask) throw new HttpError(409, "Task already has an active run.");

        const [createdRun] = await tx.insert(agentRuns).values({
          id: runId, taskId: id, status: "running", providerProtocol: resolved.provider.protocol,
          modelName: resolved.model.modelId, assistantText: "", startedAt: now
        }).returning();
        if (followUp?.trim()) {
          await tx.insert(agentMessages).values({
            id: createId("agm"), taskId: id, runId, role: "user", content: followUp.trim(), createdAt: now
          });
        }
        return createdRun;
      });
    } catch (error) {
      activeRuns.delete(id);
      completeActiveAgentRun(active);
      if (isActiveRunUniqueConflict(error)) throw new HttpError(409, "Task already has an active run.");
      throw error;
    }

    void executeAgentRun(active, resolved).catch((error) => {
      logger.error("agent run cleanup failed", { taskId: id, runId: run.id, error: error instanceof Error ? error.message : String(error) });
    });
    return run;
  });
}

export async function stopAgentTask(id: string) {
  const active = activeRuns.get(id);
  if (!active) throw new HttpError(409, "Task has no active run.");
  await active.stop();
}

export async function agentTaskEvents(id: string, latestRunId?: string, limit = 100) {
  let runId = latestRunId;
  if (!runId) {
    const [latestRun] = await db.select({ id: agentRuns.id }).from(agentRuns)
      .where(eq(agentRuns.taskId, id)).orderBy(desc(agentRuns.startedAt)).limit(1);
    runId = latestRun?.id;
  }
  if (!runId) return [];
  const recent = await db.select().from(agentEvents).where(eq(agentEvents.runId, runId))
    .orderBy(desc(agentEvents.sequence)).limit(Math.max(1, Math.min(limit, 200)));
  return recent.reverse().map((event) => {
    if (event.kind !== "run_finished" || !("assistantText" in event.payload)) return event;
    const payload = { ...event.payload };
    delete payload.assistantText;
    return { ...event, payload };
  });
}

export async function branchesForProject(projectId: string) {
  const [project] = await db.select().from(projects).where(eq(projects.id, projectId)).limit(1);
  if (!project) throw new HttpError(404, "Project not found.");
  return fetchProjectBranches(project);
}

export async function gitPreviewForTask(id: string) {
  const detail = await getAgentTask(id);
  if (!detail) throw new HttpError(404, "Task not found.");
  const [project] = await db.select().from(projects).where(eq(projects.id, detail.projectId)).limit(1);
  if (!project) throw new HttpError(404, "Project not found.");
  return taskGitPreview(project, detail);
}

export async function commitAgentTask(id: string, message: string, paths?: string[]) {
  const initial = await getAgentTask(id);
  if (!initial) throw new HttpError(404, "Task not found.");
  return withAgentLifecycleLock([
    agentProjectLifecycleKey(initial.projectId),
    agentTaskLifecycleKey(id)
  ], async () => {
    const detail = await getAgentTask(id);
    if (!detail) throw new HttpError(404, "Task not found.");
    if (detail.status === "running") throw new HttpError(409, "Wait for the active run to finish.");
    const [project] = await db.select().from(projects).where(eq(projects.id, detail.projectId)).limit(1);
    return commitTaskWorktree(project, detail, message, paths);
  });
}

export async function pushAgentTask(id: string) {
  const initial = await getAgentTask(id);
  if (!initial) throw new HttpError(404, "Task not found.");
  return withAgentLifecycleLock([
    agentProjectLifecycleKey(initial.projectId),
    agentTaskLifecycleKey(id)
  ], async () => {
    const detail = await getAgentTask(id);
    if (!detail) throw new HttpError(404, "Task not found.");
    if (detail.status === "running") throw new HttpError(409, "Wait for the active run to finish.");
    const [project] = await db.select().from(projects).where(eq(projects.id, detail.projectId)).limit(1);
    const sha = await pushTaskWorktree(project, detail);
    const preview = await taskGitPreview(project, detail);
    const done = preview.isClean;
    const pushedAt = new Date();
    if (done && detail.autoCleanup) {
      try {
        await cleanupTaskWorktree(project, detail);
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        await db.update(agentTasks).set({
          status: "review",
          pushedAt,
          lastError: `Git automation failed: ${message}`,
          updatedAt: new Date()
        }).where(eq(agentTasks.id, id));
        throw error;
      }
    }
    await db.update(agentTasks).set({
      status: done ? "done" : "review",
      pushedAt,
      lastError: done ? detail.lastError : "Branch pushed, but uncommitted changes remain in the worktree.",
      ...(done && detail.autoCleanup ? { worktreePath: null } : {}),
      updatedAt: new Date()
    }).where(eq(agentTasks.id, id));
    return sha;
  });
}

export async function cleanupAgentTask(id: string, force = false) {
  const initial = await getAgentTask(id);
  if (!initial) throw new HttpError(404, "Task not found.");
  await withAgentLifecycleLock([
    agentProjectLifecycleKey(initial.projectId),
    agentTaskLifecycleKey(id)
  ], async () => {
    const detail = await getAgentTask(id);
    if (!detail) throw new HttpError(404, "Task not found.");
    if (detail.status === "running") throw new HttpError(409, "Stop the active run before cleanup.");
    const [project] = await db.select().from(projects).where(eq(projects.id, detail.projectId)).limit(1);
    await cleanupTaskWorktree(project, detail, force);
    await db.update(agentTasks).set({ worktreePath: null, updatedAt: new Date() }).where(eq(agentTasks.id, id));
  });
}

export async function recoverInterruptedAgentRuns() {
  const running = await db.select().from(agentRuns).where(eq(agentRuns.status, "running"));
  if (!running.length) return 0;
  // New runs are host-native, but an upgrade can leave an owned v0.10 task
  // container behind. Remove only containers carrying Yanto's run labels
  // before releasing the persisted ownership lock.
  const listed = await runCommand("docker", [
    "ps", "-a", "--filter", "label=com.yanto.agent=true",
    "--format", "{{.ID}}\t{{.Label \"com.yanto.agent.run-id\"}}"
  ], { timeoutMs: 30_000, maxOutputBytes: 512 * 1024 });
  if (listed.exitCode !== 0) throw new Error(listed.output.trim() || "Unable to list interrupted legacy agent containers.");

  const containersByRun = new Map<string, string[]>();
  for (const line of listed.output.split("\n")) {
    const [containerId, runId] = line.trim().split("\t");
    if (!containerId || !runId) continue;
    const containers = containersByRun.get(runId) ?? [];
    containers.push(containerId);
    containersByRun.set(runId, containers);
  }

  let recovered = 0;
  for (const run of running) {
    let cleanupError: string | undefined;
    for (const containerId of containersByRun.get(run.id) ?? []) {
      const removed = await runCommand("docker", ["rm", "-f", containerId], { timeoutMs: 30_000, maxOutputBytes: 64 * 1024 });
      if (removed.exitCode !== 0 && !removed.output.toLowerCase().includes("no such container")) {
        cleanupError = removed.output.trim() || `Unable to remove interrupted legacy agent container ${containerId}.`;
        break;
      }
    }
    if (cleanupError) throw new Error(`Unable to recover interrupted agent run ${run.id}: ${cleanupError}`);

    const message = "Yanto restarted while the agent run was active.";
    const now = new Date();
    await db.transaction(async (tx) => {
      await tx.update(agentRuns).set({ status: "failed", error: message, finishedAt: now }).where(eq(agentRuns.id, run.id));
      await tx.update(agentTasks).set({ status: "done", lastError: message, finishedAt: now, updatedAt: now }).where(eq(agentTasks.id, run.taskId));
    });
    recovered += 1;
  }
  return recovered;
}

export async function drainActiveAgentRuns(timeoutMs = config.agentShutdownTimeoutMs) {
  const runs = [...activeRuns.values()];
  if (!runs.length) return { drained: true, activeRunCount: 0 };

  const drain = Promise.allSettled(runs.map(async (run) => {
    try {
      await run.stop();
    } finally {
      await run.completion;
    }
  })).then(() => true);
  let timer: NodeJS.Timeout | undefined;
  const timedOut = new Promise<false>((resolve) => {
    timer = setTimeout(() => resolve(false), timeoutMs);
    timer.unref();
  });
  const drained = await Promise.race([drain, timedOut]);
  if (timer) clearTimeout(timer);
  return { drained, activeRunCount: runs.length };
}
