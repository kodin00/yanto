import { asc, desc, eq, inArray } from "drizzle-orm";
import { db } from "../db/index.js";
import { agentEvents, agentMessages, agentRuns, agentTasks, aiModels, aiProviders, projects, type AgentTaskRow } from "../db/schema.js";
import { config } from "../config.js";
import { HttpError } from "../http-utils.js";
import { logger } from "../logger.js";
import { createId } from "./tokens.js";
import { resolveProviderModel } from "./ai-providers.js";
import { AgentSandbox } from "./agent-tools.js";
import { runAgentProvider } from "./agent-provider-runner.js";
import { runCodexAccount } from "./codex-account-runner.js";
import { agentEventBus } from "./agent-events.js";
import { cleanupTaskWorktree, commitTaskWorktree, fetchProjectBranches, prepareTaskWorktree, pushTaskWorktree, taskGitPreview } from "./agent-worktrees.js";

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

const activeRuns = new Map<string, { runId: string; controller: AbortController; sandbox?: AgentSandbox }>();
const eventSequences = new Map<string, number>();

function taskView(row: {
  task: AgentTaskRow;
  projectName: string;
  modelName: string;
  providerName: string;
}, latestRun: typeof agentRuns.$inferSelect | null = null) {
  return { ...row.task, projectName: row.projectName, modelName: row.modelName, providerName: row.providerName, latestRun };
}

async function taskRows(taskId?: string) {
  const query = db.select({ task: agentTasks, projectName: projects.name, modelName: aiModels.displayName, providerName: aiProviders.name })
    .from(agentTasks)
    .innerJoin(projects, eq(agentTasks.projectId, projects.id))
    .innerJoin(aiModels, eq(agentTasks.modelId, aiModels.id))
    .innerJoin(aiProviders, eq(aiModels.providerId, aiProviders.id));
  return taskId ? query.where(eq(agentTasks.id, taskId)).limit(1) : query.orderBy(desc(agentTasks.createdAt));
}

export async function listAgentTasks() {
  const rows = await taskRows();
  const runs = rows.length ? await db.select().from(agentRuns).where(inArray(agentRuns.taskId, rows.map((row) => row.task.id))).orderBy(desc(agentRuns.startedAt)) : [];
  return rows.map((row) => taskView(row, runs.find((run) => run.taskId === row.task.id) ?? null));
}

export async function getAgentTask(id: string) {
  const [row] = await taskRows(id);
  if (!row) return undefined;
  const [messages, runs] = await Promise.all([
    db.select().from(agentMessages).where(eq(agentMessages.taskId, id)).orderBy(asc(agentMessages.createdAt)),
    db.select().from(agentRuns).where(eq(agentRuns.taskId, id)).orderBy(desc(agentRuns.startedAt))
  ]);
  return { ...taskView(row, runs[0] ?? null), messages, runs };
}

export async function createAgentTask(input: CreateAgentTaskInput) {
  const [project] = await db.select().from(projects).where(eq(projects.id, input.projectId)).limit(1);
  if (!project) throw new HttpError(404, "Project not found.");
  if (!project.gitUrl) throw new HttpError(400, "AI tasks require a Git-backed project.");
  await resolveProviderModel(input.modelId);
  if (!input.title.trim() || !input.prompt.trim()) throw new HttpError(400, "Task title and instructions are required.");
  if (input.sourceBranch.trim() === input.taskBranch.trim()) throw new HttpError(400, "Task branch must differ from the source branch.");
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
  const current = await getAgentTask(id);
  if (!current) return undefined;
  if (current.status === "running") throw new HttpError(409, "Stop the active run before changing task settings.");
  const autoCommit = input.autoCommit ?? current.autoCommit;
  const autoPush = input.autoPush ?? current.autoPush;
  const autoCleanup = input.autoCleanup ?? current.autoCleanup;
  if (autoPush && !autoCommit) throw new HttpError(400, "Auto-push requires auto-commit.");
  if (autoCleanup && !autoPush) throw new HttpError(400, "Auto-cleanup requires auto-push.");
  if (input.modelId) await resolveProviderModel(input.modelId);
  await db.update(agentTasks).set({
    ...(input.title !== undefined ? { title: input.title.trim() } : {}),
    ...(input.modelId !== undefined ? { modelId: input.modelId } : {}),
    autoCommit, autoPush, autoCleanup, updatedAt: new Date()
  }).where(eq(agentTasks.id, id));
  return getAgentTask(id);
}

export async function deleteAgentTask(id: string, force = false) {
  const detail = await getAgentTask(id);
  if (!detail) return;
  if (detail.status === "running") throw new HttpError(409, "Stop the active run before deleting the task.");
  const [project] = await db.select().from(projects).where(eq(projects.id, detail.projectId)).limit(1);
  if (project && detail.worktreePath) await cleanupTaskWorktree(project, detail, force);
  await db.delete(agentTasks).where(eq(agentTasks.id, id));
}

async function recordEvent(taskId: string, runId: string, kind: string, payload: Record<string, unknown>, done = false) {
  const sequence = (eventSequences.get(runId) ?? 0) + 1;
  eventSequences.set(runId, sequence);
  if (kind !== "assistant_delta") {
    await db.insert(agentEvents).values({ id: createId("age"), runId, sequence, kind, payload, createdAt: new Date() });
  }
  agentEventBus.publish({ taskId, runId, sequence, kind, payload, done });
}

async function finishRun(taskId: string, runId: string, status: "succeeded" | "failed" | "canceled", assistantText: string, error?: string) {
  await db.update(agentRuns).set({ status, assistantText, error: error ?? null, finishedAt: new Date() }).where(eq(agentRuns.id, runId));
  await db.update(agentTasks).set({ status: "review", lastError: error ?? null, finishedAt: new Date(), updatedAt: new Date() }).where(eq(agentTasks.id, taskId));
  if (assistantText.trim()) await db.insert(agentMessages).values({ id: createId("agm"), taskId, runId, role: "assistant", content: assistantText.trim(), createdAt: new Date() });
  await recordEvent(taskId, runId, "run_finished", { status, error: error ?? null, assistantText });
}

async function executeAgentRun(taskId: string, runId: string, controller: AbortController) {
  let sandbox: AgentSandbox | undefined;
  let assistantText = "";
  let providerCompleted = false;
  const timeout = setTimeout(() => controller.abort(new Error("Agent run timed out.")), config.agentRunTimeoutMs);
  timeout.unref();
  try {
    const detail = await getAgentTask(taskId);
    if (!detail) throw new Error("Task not found.");
    const [project] = await db.select().from(projects).where(eq(projects.id, detail.projectId)).limit(1);
    if (!project) throw new Error("Project not found.");
    const resolved = await resolveProviderModel(detail.modelId);
    const prepared = await prepareTaskWorktree(project, detail);
    await db.update(agentTasks).set({ worktreePath: prepared.worktreePath, sourceSha: prepared.sourceSha, updatedAt: new Date() }).where(eq(agentTasks.id, taskId));
    await recordEvent(taskId, runId, "worktree_ready", { path: prepared.worktreePath, sourceSha: prepared.sourceSha, branch: detail.taskBranch });

    const messages = detail.messages.slice(-50).map((message) => ({ role: message.role === "assistant" ? "assistant" as const : "user" as const, content: message.content }));
    if (resolved.provider.protocol === "codex_account") {
      activeRuns.set(taskId, { runId, controller });
      await recordEvent(taskId, runId, "sandbox_started", { image: config.agentDefaultImage, runtime: "codex" });
      const lastUserMessage = [...detail.messages].reverse().find((message) => message.role === "user")?.content ?? detail.prompt;
      const result = await runCodexAccount({
        runId, worktreePath: prepared.worktreePath, prompt: lastUserMessage, model: resolved.model.modelId,
        threadId: detail.codexThreadId, signal: controller.signal,
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
      sandbox = new AgentSandbox(runId, prepared.worktreePath, project.agentImage.trim() || config.agentDefaultImage);
      activeRuns.set(taskId, { runId, controller, sandbox });
      await sandbox.start();
      await recordEvent(taskId, runId, "sandbox_started", { image: project.agentImage.trim() || config.agentDefaultImage });
      assistantText = await runAgentProvider({
        protocol: resolved.provider.protocol as never,
        baseUrl: resolved.provider.baseUrl,
        apiKey: resolved.apiKey,
        model: resolved.model.modelId,
        messages,
        sandbox,
        signal: controller.signal,
        event: (kind, payload) => recordEvent(taskId, runId, kind, payload)
      });
    }
    await finishRun(taskId, runId, "succeeded", assistantText);
    providerCompleted = true;

    const refreshed = await getAgentTask(taskId);
    if (!refreshed) return;
    if (refreshed.autoCommit) {
      const preview = await taskGitPreview(project, refreshed);
      if (!preview.isClean) {
        const sha = await commitTaskWorktree(project, refreshed, `task: ${refreshed.title}`);
        await recordEvent(taskId, runId, "git_committed", { sha, message: `task: ${refreshed.title}` });
      }
      if (refreshed.autoPush) {
        const sha = await pushTaskWorktree(project, refreshed);
        await db.update(agentTasks).set({ status: "done", pushedAt: new Date(), updatedAt: new Date() }).where(eq(agentTasks.id, taskId));
        await recordEvent(taskId, runId, "git_pushed", { sha, branch: refreshed.taskBranch });
        if (refreshed.autoCleanup) {
          await cleanupTaskWorktree(project, refreshed);
          await db.update(agentTasks).set({ worktreePath: null, updatedAt: new Date() }).where(eq(agentTasks.id, taskId));
          await recordEvent(taskId, runId, "worktree_cleaned", {});
        }
      }
    }
    const completedTask = await getAgentTask(taskId);
    await recordEvent(taskId, runId, "task_finished", { status: completedTask?.status ?? "review" }, true);
  } catch (error) {
    const canceled = controller.signal.aborted;
    const message = error instanceof Error ? error.message : String(error);
    if (providerCompleted) {
      await db.update(agentTasks).set({ status: "review", lastError: `Git automation failed: ${message}`, updatedAt: new Date() }).where(eq(agentTasks.id, taskId));
      await recordEvent(taskId, runId, "git_automation_failed", { error: message }, true);
    } else {
      await finishRun(taskId, runId, canceled ? "canceled" : "failed", assistantText, message);
      await recordEvent(taskId, runId, "task_finished", { status: "review", error: message }, true);
    }
    logger.error("agent run failed", { taskId, runId, canceled, error: message });
  } finally {
    clearTimeout(timeout);
    await sandbox?.stop();
    activeRuns.delete(taskId);
    eventSequences.delete(runId);
  }
}

export async function startAgentTask(id: string, followUp?: string) {
  const task = await getAgentTask(id);
  if (!task) throw new HttpError(404, "Task not found.");
  if (activeRuns.has(id) || task.status === "running") throw new HttpError(409, "Task already has an active run.");
  if (activeRuns.size >= config.agentMaxConcurrentRuns) throw new HttpError(429, "Agent run capacity is full. Try again after another task finishes.");
  if (followUp?.trim()) await db.insert(agentMessages).values({ id: createId("agm"), taskId: id, role: "user", content: followUp.trim(), createdAt: new Date() });
  const resolved = await resolveProviderModel(task.modelId);
  const [run] = await db.insert(agentRuns).values({
    id: createId("agr"), taskId: id, status: "running", providerProtocol: resolved.provider.protocol,
    modelName: resolved.model.modelId, assistantText: "", startedAt: new Date()
  }).returning();
  await db.update(agentTasks).set({ status: "running", lastError: null, startedAt: new Date(), finishedAt: null, updatedAt: new Date() }).where(eq(agentTasks.id, id));
  const controller = new AbortController();
  activeRuns.set(id, { runId: run.id, controller });
  await recordEvent(id, run.id, "run_started", { model: run.modelName, protocol: run.providerProtocol });
  void executeAgentRun(id, run.id, controller);
  return run;
}

export async function stopAgentTask(id: string) {
  const active = activeRuns.get(id);
  if (!active) throw new HttpError(409, "Task has no active run.");
  active.controller.abort(new Error("Stopped by user."));
  await active.sandbox?.stop();
}

export async function agentTaskEvents(id: string) {
  const runs = await db.select({ id: agentRuns.id }).from(agentRuns).where(eq(agentRuns.taskId, id));
  if (!runs.length) return [];
  return db.select().from(agentEvents).where(inArray(agentEvents.runId, runs.map((run) => run.id))).orderBy(asc(agentEvents.createdAt), asc(agentEvents.sequence));
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
  const detail = await getAgentTask(id);
  if (!detail) throw new HttpError(404, "Task not found.");
  if (detail.status === "running") throw new HttpError(409, "Wait for the active run to finish.");
  const [project] = await db.select().from(projects).where(eq(projects.id, detail.projectId)).limit(1);
  return commitTaskWorktree(project, detail, message, paths);
}

export async function pushAgentTask(id: string) {
  const detail = await getAgentTask(id);
  if (!detail) throw new HttpError(404, "Task not found.");
  if (detail.status === "running") throw new HttpError(409, "Wait for the active run to finish.");
  const [project] = await db.select().from(projects).where(eq(projects.id, detail.projectId)).limit(1);
  const sha = await pushTaskWorktree(project, detail);
  const preview = await taskGitPreview(project, detail);
  const done = preview.isClean;
  await db.update(agentTasks).set({ status: done ? "done" : "review", pushedAt: new Date(), lastError: done ? null : "Branch pushed, but uncommitted changes remain in the worktree.", updatedAt: new Date() }).where(eq(agentTasks.id, id));
  if (done && detail.autoCleanup) {
    await cleanupTaskWorktree(project, detail);
    await db.update(agentTasks).set({ worktreePath: null, updatedAt: new Date() }).where(eq(agentTasks.id, id));
  }
  return sha;
}

export async function cleanupAgentTask(id: string, force = false) {
  const detail = await getAgentTask(id);
  if (!detail) throw new HttpError(404, "Task not found.");
  if (detail.status === "running") throw new HttpError(409, "Stop the active run before cleanup.");
  const [project] = await db.select().from(projects).where(eq(projects.id, detail.projectId)).limit(1);
  await cleanupTaskWorktree(project, detail, force);
  await db.update(agentTasks).set({ worktreePath: null, updatedAt: new Date() }).where(eq(agentTasks.id, id));
}

export async function recoverInterruptedAgentRuns() {
  const running = await db.select().from(agentRuns).where(eq(agentRuns.status, "running"));
  if (!running.length) return 0;
  const now = new Date();
  await db.update(agentRuns).set({ status: "failed", error: "Yanto restarted while the agent run was active.", finishedAt: now }).where(eq(agentRuns.status, "running"));
  await db.update(agentTasks).set({ status: "review", lastError: "Yanto restarted while the agent run was active.", finishedAt: now, updatedAt: now }).where(eq(agentTasks.status, "running"));
  return running.length;
}
