import { Router } from "express";
import { spawn } from "node:child_process";
import { requireAuth } from "../auth.js";
import { asyncRoute, actor, routeParam, sendStreamEvent, startEventStream } from "../http-utils.js";
import { recordAuditLog } from "../services/audit.js";
import { containerLogs, listContainers, listContainersSummary, restartContainer, startContainer, stopContainer } from "../services/docker.js";

const router = Router();

router.get(
  "/api/containers",
  requireAuth,
  asyncRoute(async (req, res) => {
    const skipStats = req.query.stats === "false";
    res.json(skipStats ? await listContainersSummary() : await listContainers());
  })
);

router.get(
  "/api/containers/:id/logs",
  requireAuth,
  asyncRoute(async (req, res) => {
    res.type("text/plain").send(await containerLogs(routeParam(req, "id")));
  })
);

router.get(
  "/api/containers/:id/logs/stream",
  requireAuth,
  asyncRoute(async (req, res) => {
    startEventStream(res);
    let closed = false;
    const child = spawn("docker", ["logs", "--tail", "500", "--follow", routeParam(req, "id")], {
      env: process.env,
      shell: false
    });

    const sendChunk = (buffer: Buffer) => {
      if (closed) return;
      sendStreamEvent(res, { chunk: buffer.toString() });
    };

    child.stdout.on("data", sendChunk);
    child.stderr.on("data", sendChunk);
    child.on("error", (error) => {
      if (closed) return;
      sendStreamEvent(res, { error: error.message, done: true });
      closed = true;
      res.end();
    });
    child.on("close", (exitCode) => {
      if (closed) return;
      sendStreamEvent(res, { chunk: `\nLog stream closed${exitCode ? ` with exit code ${exitCode}` : ""}.\n`, done: true });
      closed = true;
      res.end();
    });

    req.on("close", () => {
      closed = true;
      child.kill("SIGTERM");
    });
  })
);

router.post(
  "/api/containers/:id/stop",
  requireAuth,
  asyncRoute(async (req, res) => {
    const id = routeParam(req, "id");
    await stopContainer(id);
    await recordAuditLog({ actor: actor(req), action: "container.stop", entityType: "container", entityId: id });
    res.json({ ok: true });
  })
);

router.post(
  "/api/containers/:id/start",
  requireAuth,
  asyncRoute(async (req, res) => {
    const id = routeParam(req, "id");
    await startContainer(id);
    await recordAuditLog({ actor: actor(req), action: "container.start", entityType: "container", entityId: id });
    res.json({ ok: true });
  })
);

router.post(
  "/api/containers/:id/restart",
  requireAuth,
  asyncRoute(async (req, res) => {
    const id = routeParam(req, "id");
    await restartContainer(id);
    await recordAuditLog({ actor: actor(req), action: "container.restart", entityType: "container", entityId: id });
    res.json({ ok: true });
  })
);

export default router;
