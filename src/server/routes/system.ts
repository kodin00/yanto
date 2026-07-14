import { Router } from "express";
import { requireAuth, requireOwner } from "../auth.js";
import { accessibleProjectIds, assertProjectAccess } from "../authorization.js";
import { config } from "../config.js";
import { asyncRoute, actor } from "../http-utils.js";
import { recordAuditLog } from "../services/audit.js";
import { listAuditLogs, listAuditLogsForProjects } from "../services/audit.js";
import { cleanupDocker, previewDockerCleanup } from "../services/docker.js";
import { ensureWorkerJoinToken } from "../services/settings.js";
import { listNodes } from "../services/nodes.js";
import { healthStatus, systemUsage } from "../services/system.js";
import { logger } from "../logger.js";

const router = Router();

router.get(
  "/api/health",
  asyncRoute(async (_req, res) => {
    const health = await healthStatus();
    res.status(health.ok ? 200 : 503).json({
      ...health,
      checks: Object.fromEntries(Object.entries(health.checks).map(([name, check]) => [name, { ok: check.ok }]))
    });
  })
);

router.get(
  "/api/nodes",
  requireOwner,
  asyncRoute(async (_req, res) => {
    res.json(await listNodes());
  })
);

router.post(
  "/api/nodes/join-token",
  requireOwner,
  asyncRoute(async (req, res) => {
    const token = await ensureWorkerJoinToken();
    const command = `curl -fsSL https://raw.githubusercontent.com/kodin00/yanto/master/scripts/install.sh | sudo bash -s -- worker --master ${config.appBaseUrl} --join-token ${token}`;
    await recordAuditLog({ actor: actor(req), action: "node.join_token.view", entityType: "deployment_node" });
    res.json({ token, command });
  })
);

router.get(
  "/api/audit-logs",
  requireAuth,
  asyncRoute(async (req, res) => {
    const limit = Math.min(500, Math.max(1, Number(req.query.limit ?? 100)));
    const projectId = typeof req.query.projectId === "string" ? req.query.projectId : undefined;
    if (projectId) assertProjectAccess(req, projectId);
    const projectIds = accessibleProjectIds(req);
    res.json(projectIds === null || projectId
      ? await listAuditLogs(limit, projectId)
      : await listAuditLogsForProjects([...projectIds], limit));
  })
);

router.get(
  "/api/system/usage",
  requireOwner,
  asyncRoute(async (_req, res) => {
    res.json(await systemUsage());
  })
);

router.get(
  "/api/system/logs",
  requireOwner,
  asyncRoute(async (_req, res) => {
    res.type("text/plain").send(logger.history() || "No system log entries recorded yet.");
  })
);

router.get(
  "/api/system/cleanup/preview",
  requireOwner,
  asyncRoute(async (_req, res) => {
    res.json({ logs: await previewDockerCleanup() });
  })
);

router.post(
  "/api/system/cleanup",
  requireOwner,
  asyncRoute(async (req, res) => {
    const logs = await cleanupDocker();
    await recordAuditLog({ actor: actor(req), action: "system.cleanup", entityType: "system", metadata: { protected: true } });
    res.json({ logs });
  })
);

export default router;
