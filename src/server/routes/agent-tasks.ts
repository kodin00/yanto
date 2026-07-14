import { Router } from "express";
import { z } from "zod";
import { requireOwner as requireAuth } from "../auth.js";
import { assertProjectPermission, filterByProjectPermission, isOwner, startStreamAuthorizationGuard } from "../authorization.js";
import { HttpError } from "../http-utils.js";
import { actor, asyncRoute, routeParam, sendStreamEvent, startEventStream } from "../http-utils.js";
import { recordAuditLog } from "../services/audit.js";
import { agentEventBus, type AgentLiveEvent } from "../services/agent-events.js";
import { agentTaskEvents, branchesForProject, cleanupAgentTask, commitAgentTask, createAgentTask, deleteAgentTask, getAgentTask, gitPreviewForTask, listAgentTasks, listAgentWorktrees, pushAgentTask, setAgentTaskArchived, startAgentTask, stopAgentTask, updateAgentTask } from "../services/agent-tasks.js";

const router = Router();
const taskInput = z.object({
  projectId: z.string().min(1), modelId: z.string().min(1), title: z.string().trim().min(1).max(200), prompt: z.string().trim().min(1).max(200_000),
  sourceBranch: z.string().trim().min(1).max(250), taskBranch: z.string().trim().min(1).max(250), resumeExistingBranch: z.boolean().optional().default(false),
  autoCommit: z.boolean().optional().default(false), autoPush: z.boolean().optional().default(false), autoCleanup: z.boolean().optional().default(false)
});

async function assertTaskPermission(req: Parameters<typeof actor>[0], taskId: string) {
  const task = await getAgentTask(taskId);
  if (!task) throw new HttpError(404, "Task not found.");
  assertProjectPermission(req, task.projectId, "tasks");
  return task;
}

router.get("/api/agent/tasks", requireAuth, asyncRoute(async (req, res) => {
  const archived = z.enum(["true", "false"]).optional().parse(req.query.archived) === "true";
  res.json(filterByProjectPermission(req, await listAgentTasks(archived), "tasks"));
}));
router.get("/api/agent/worktrees", requireAuth, asyncRoute(async (req, res) => {
  res.json(filterByProjectPermission(req, await listAgentWorktrees(), "tasks"));
}));
router.delete("/api/agent/worktrees/:taskId", requireAuth, asyncRoute(async (req, res) => {
  const taskId = routeParam(req, "taskId");
  const task = await assertTaskPermission(req, taskId);
  await cleanupAgentTask(taskId, req.query.force === "true");
  await recordAuditLog({ actor: actor(req), action: "agent_worktree.delete", entityType: "agent_task", entityId: taskId, projectId: task.projectId });
  res.status(204).end();
}));
router.post("/api/agent/tasks", requireAuth, asyncRoute(async (req, res) => {
  const body = taskInput.parse(req.body);
  assertProjectPermission(req, body.projectId, "tasks");
  const task = await createAgentTask(body);
  await recordAuditLog({ actor: actor(req), action: "agent_task.create", entityType: "agent_task", entityId: task.id, projectId: task.projectId });
  res.status(201).json(task);
}));
router.get("/api/agent/tasks/:id", requireAuth, asyncRoute(async (req, res) => {
  const task = await getAgentTask(routeParam(req, "id"));
  if (!task) { res.status(404).json({ message: "Task not found." }); return; }
  assertProjectPermission(req, task.projectId, "tasks");
  res.json(task);
}));
router.patch("/api/agent/tasks/:id", requireAuth, asyncRoute(async (req, res) => {
  const id = routeParam(req, "id");
  const existing = await assertTaskPermission(req, id);
  const task = await updateAgentTask(id, z.object({ title: z.string().trim().min(1).max(200).optional(), modelId: z.string().min(1).optional(), autoCommit: z.boolean().optional(), autoPush: z.boolean().optional(), autoCleanup: z.boolean().optional() }).parse(req.body));
  if (!task) { res.status(404).json({ message: "Task not found." }); return; }
  await recordAuditLog({ actor: actor(req), action: "agent_task.update", entityType: "agent_task", entityId: id, projectId: existing.projectId });
  res.json(task);
}));
router.patch("/api/agent/tasks/:id/archive", requireAuth, asyncRoute(async (req, res) => {
  const archived = z.object({ archived: z.boolean() }).parse(req.body).archived;
  const id = routeParam(req, "id");
  await assertTaskPermission(req, id);
  const task = await setAgentTaskArchived(id, archived);
  if (!task) { res.status(404).json({ message: "Task not found." }); return; }
  await recordAuditLog({ actor: actor(req), action: archived ? "agent_task.archive" : "agent_task.restore", entityType: "agent_task", entityId: task.id, projectId: task.projectId });
  res.json(task);
}));
router.delete("/api/agent/tasks/:id", requireAuth, asyncRoute(async (req, res) => {
  const id = routeParam(req, "id");
  const task = await assertTaskPermission(req, id);
  await deleteAgentTask(id, req.query.force === "true");
  await recordAuditLog({ actor: actor(req), action: "agent_task.delete", entityType: "agent_task", entityId: id, projectId: task.projectId });
  res.status(204).end();
}));
router.post("/api/agent/tasks/:id/run", requireAuth, asyncRoute(async (req, res) => {
  const body = z.object({ message: z.string().trim().min(1).max(200_000).optional() }).parse(req.body ?? {});
  const id = routeParam(req, "id");
  const task = await assertTaskPermission(req, id);
  const run = await startAgentTask(id, body.message);
  await recordAuditLog({ actor: actor(req), action: "agent_task.run", entityType: "agent_task", entityId: id, projectId: task.projectId, metadata: { runId: run.id } });
  res.status(202).json(run);
}));
router.post("/api/agent/tasks/:id/stop", requireAuth, asyncRoute(async (req, res) => { const id = routeParam(req, "id"); const task = await assertTaskPermission(req, id); await stopAgentTask(id); await recordAuditLog({ actor: actor(req), action: "agent_task.stop", entityType: "agent_task", entityId: id, projectId: task.projectId }); res.json({ ok: true }); }));
router.get("/api/projects/:id/agent-branches", requireAuth, asyncRoute(async (req, res) => { const projectId = routeParam(req, "id"); assertProjectPermission(req, projectId, "tasks"); res.json(await branchesForProject(projectId)); }));
router.get("/api/agent/tasks/:id/git", requireAuth, asyncRoute(async (req, res) => { const id = routeParam(req, "id"); await assertTaskPermission(req, id); res.json(await gitPreviewForTask(id)); }));
router.post("/api/agent/tasks/:id/commit", requireAuth, asyncRoute(async (req, res) => {
  const body = z.object({ message: z.string().trim().min(1).max(500), paths: z.array(z.string().max(1000)).max(500).optional() }).parse(req.body);
  const id = routeParam(req, "id");
  const task = await assertTaskPermission(req, id);
  const sha = await commitAgentTask(id, body.message, body.paths);
  await recordAuditLog({ actor: actor(req), action: "agent_task.commit", entityType: "agent_task", entityId: id, projectId: task.projectId, metadata: { sha } });
  res.json({ sha });
}));
router.post("/api/agent/tasks/:id/push", requireAuth, asyncRoute(async (req, res) => { const id = routeParam(req, "id"); const task = await assertTaskPermission(req, id); const sha = await pushAgentTask(id); await recordAuditLog({ actor: actor(req), action: "agent_task.push", entityType: "agent_task", entityId: id, projectId: task.projectId, metadata: { sha } }); res.json({ sha }); }));
router.delete("/api/agent/tasks/:id/worktree", requireAuth, asyncRoute(async (req, res) => { const id = routeParam(req, "id"); const task = await assertTaskPermission(req, id); await cleanupAgentTask(id, req.query.force === "true"); await recordAuditLog({ actor: actor(req), action: "agent_worktree.delete", entityType: "agent_task", entityId: id, projectId: task.projectId }); res.status(204).end(); }));

router.get("/api/agent/tasks/:id/events/stream", requireAuth, asyncRoute(async (req, res) => {
  const id = routeParam(req, "id");
  const buffered: AgentLiveEvent[] = [];
  let direct = false;
  let closed = false;
  let unsubscribe = () => {};
  let stopAuthorizationGuard = () => {};
  const cleanup = () => {
    if (closed) return;
    closed = true;
    stopAuthorizationGuard();
    unsubscribe();
  };
  const deliver = (event: AgentLiveEvent) => {
    if (closed) return;
    sendStreamEvent(res, event);
    if (event.done) { cleanup(); res.end(); }
  };

  unsubscribe = agentEventBus.subscribe(id, (event) => {
    if (direct) deliver(event);
    else buffered.push(event);
  });
  res.once("close", cleanup);

  try {
    const task = await getAgentTask(id);
    if (!task) { cleanup(); res.status(404).json({ message: "Task not found." }); return; }
    assertProjectPermission(req, task.projectId, "tasks");
    const runId = task.latestRun?.id ?? null;
    const events = await agentTaskEvents(id, runId ?? undefined);
    const sequence = events.reduce((latest, event) => Math.max(latest, event.sequence), 0);

    startEventStream(res);
    stopAuthorizationGuard = startStreamAuthorizationGuard(req, async () => {
      if (!isOwner(req)) throw new Error("Owner access was revoked.");
      const current = await getAgentTask(id);
      if (!current) throw new Error("Task no longer exists.");
      assertProjectPermission(req, current.projectId, "tasks");
    }, () => {
      if (closed) return;
      sendStreamEvent(res, { kind: "authorization_revoked", payload: { message: "Authorization was revoked." }, done: true });
      cleanup();
      res.end();
    }).stop;
    sendStreamEvent(res, {
      kind: "snapshot",
      payload: { task, events, runId, sequence, watermark: { runId, sequence } },
      done: task.status !== "running"
    });

    const delivered = new Set(events.map((event) => `${event.runId}:${event.sequence}`));
    while (!closed && buffered.length) {
      const pending = buffered.splice(0);
      for (const event of pending) {
        const key = `${event.runId}:${event.sequence}`;
        if (delivered.has(key) || (event.runId === runId && event.sequence <= sequence)) continue;
        delivered.add(key);
        deliver(event);
        if (closed) break;
      }
    }

    if (closed) return;
    if (task.status !== "running") { cleanup(); res.end(); return; }
    direct = true;
  } catch (error) {
    cleanup();
    throw error;
  }
}));

export default router;
