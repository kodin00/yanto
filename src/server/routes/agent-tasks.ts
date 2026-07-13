import { Router } from "express";
import { z } from "zod";
import { requireAuth } from "../auth.js";
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

router.get("/api/agent/tasks", requireAuth, asyncRoute(async (req, res) => {
  const archived = z.enum(["true", "false"]).optional().parse(req.query.archived) === "true";
  res.json(await listAgentTasks(archived));
}));
router.get("/api/agent/worktrees", requireAuth, asyncRoute(async (_req, res) => {
  res.json(await listAgentWorktrees());
}));
router.delete("/api/agent/worktrees/:taskId", requireAuth, asyncRoute(async (req, res) => {
  const taskId = routeParam(req, "taskId");
  await cleanupAgentTask(taskId, req.query.force === "true");
  await recordAuditLog({ actor: actor(req), action: "agent_worktree.delete", entityType: "agent_task", entityId: taskId });
  res.status(204).end();
}));
router.post("/api/agent/tasks", requireAuth, asyncRoute(async (req, res) => {
  const task = await createAgentTask(taskInput.parse(req.body));
  await recordAuditLog({ actor: actor(req), action: "agent_task.create", entityType: "agent_task", entityId: task.id, projectId: task.projectId });
  res.status(201).json(task);
}));
router.get("/api/agent/tasks/:id", requireAuth, asyncRoute(async (req, res) => {
  const task = await getAgentTask(routeParam(req, "id"));
  if (!task) { res.status(404).json({ message: "Task not found." }); return; }
  res.json(task);
}));
router.patch("/api/agent/tasks/:id", requireAuth, asyncRoute(async (req, res) => {
  const task = await updateAgentTask(routeParam(req, "id"), z.object({ title: z.string().trim().min(1).max(200).optional(), modelId: z.string().min(1).optional(), autoCommit: z.boolean().optional(), autoPush: z.boolean().optional(), autoCleanup: z.boolean().optional() }).parse(req.body));
  if (!task) { res.status(404).json({ message: "Task not found." }); return; }
  res.json(task);
}));
router.patch("/api/agent/tasks/:id/archive", requireAuth, asyncRoute(async (req, res) => {
  const archived = z.object({ archived: z.boolean() }).parse(req.body).archived;
  const task = await setAgentTaskArchived(routeParam(req, "id"), archived);
  if (!task) { res.status(404).json({ message: "Task not found." }); return; }
  await recordAuditLog({ actor: actor(req), action: archived ? "agent_task.archive" : "agent_task.restore", entityType: "agent_task", entityId: task.id, projectId: task.projectId });
  res.json(task);
}));
router.delete("/api/agent/tasks/:id", requireAuth, asyncRoute(async (req, res) => {
  const id = routeParam(req, "id");
  await deleteAgentTask(id, req.query.force === "true");
  await recordAuditLog({ actor: actor(req), action: "agent_task.delete", entityType: "agent_task", entityId: id });
  res.status(204).end();
}));
router.post("/api/agent/tasks/:id/run", requireAuth, asyncRoute(async (req, res) => {
  const body = z.object({ message: z.string().trim().min(1).max(200_000).optional() }).parse(req.body ?? {});
  const run = await startAgentTask(routeParam(req, "id"), body.message);
  res.status(202).json(run);
}));
router.post("/api/agent/tasks/:id/stop", requireAuth, asyncRoute(async (req, res) => { await stopAgentTask(routeParam(req, "id")); res.json({ ok: true }); }));
router.get("/api/projects/:id/agent-branches", requireAuth, asyncRoute(async (req, res) => { res.json(await branchesForProject(routeParam(req, "id"))); }));
router.get("/api/agent/tasks/:id/git", requireAuth, asyncRoute(async (req, res) => { res.json(await gitPreviewForTask(routeParam(req, "id"))); }));
router.post("/api/agent/tasks/:id/commit", requireAuth, asyncRoute(async (req, res) => {
  const body = z.object({ message: z.string().trim().min(1).max(500), paths: z.array(z.string().max(1000)).max(500).optional() }).parse(req.body);
  res.json({ sha: await commitAgentTask(routeParam(req, "id"), body.message, body.paths) });
}));
router.post("/api/agent/tasks/:id/push", requireAuth, asyncRoute(async (req, res) => { res.json({ sha: await pushAgentTask(routeParam(req, "id")) }); }));
router.delete("/api/agent/tasks/:id/worktree", requireAuth, asyncRoute(async (req, res) => { await cleanupAgentTask(routeParam(req, "id"), req.query.force === "true"); res.status(204).end(); }));

router.get("/api/agent/tasks/:id/events/stream", requireAuth, asyncRoute(async (req, res) => {
  const id = routeParam(req, "id");
  const buffered: AgentLiveEvent[] = [];
  let direct = false;
  let closed = false;
  let unsubscribe = () => {};
  const cleanup = () => {
    if (closed) return;
    closed = true;
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
    const runId = task.latestRun?.id ?? null;
    const events = await agentTaskEvents(id, runId ?? undefined);
    const sequence = events.reduce((latest, event) => Math.max(latest, event.sequence), 0);

    startEventStream(res);
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
