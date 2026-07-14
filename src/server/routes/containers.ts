import { Router } from "express";
import { spawn } from "node:child_process";
import { requireAuth } from "../auth.js";
import { listAccessibleContainers, projectForContainer, startStreamAuthorizationGuard } from "../authorization.js";
import { asyncRoute, actor, routeParam, sendStreamEvent, startEventStream } from "../http-utils.js";
import { recordAuditLog } from "../services/audit.js";
import { containerLogs, restartContainer, startContainer, stopContainer, validateContainerId } from "../services/docker.js";

const router = Router();

router.get(
  "/api/containers",
  requireAuth,
  asyncRoute(async (req, res) => {
    res.json(await listAccessibleContainers(req));
  })
);

router.get(
  "/api/containers/:id/logs",
  requireAuth,
  asyncRoute(async (req, res) => {
    const id = routeParam(req, "id");
    await projectForContainer(req, id);
    res.type("text/plain").send(await containerLogs(id));
  })
);

router.get(
  "/api/containers/:id/logs/stream",
  requireAuth,
  asyncRoute(async (req, res) => {
    const containerId = validateContainerId(routeParam(req, "id"));
    await projectForContainer(req, containerId);
    startEventStream(res);
    let closed = false;
    let waitingForDrain = false;
    let stopAuthorizationGuard = () => {};
    const child = spawn("docker", ["logs", "--tail", "500", "--follow", containerId], {
      env: process.env,
      shell: false
    });

    const resumeStreams = () => {
      waitingForDrain = false;
      if (closed) return;
      child.stdout.resume();
      child.stderr.resume();
    };

    const pauseForBackpressure = () => {
      if (waitingForDrain || closed) return;
      waitingForDrain = true;
      child.stdout.pause();
      child.stderr.pause();
      res.once("drain", resumeStreams);
    };

    const cleanup = () => {
      if (closed) return;
      closed = true;
      res.off("drain", resumeStreams);
      child.stdout.pause();
      child.stderr.pause();
      stopAuthorizationGuard();
      if (!child.killed) child.kill("SIGTERM");
    };

    const sendChunk = (buffer: Buffer) => {
      if (closed) return;
      if (!sendStreamEvent(res, { chunk: buffer.toString() })) pauseForBackpressure();
    };

    stopAuthorizationGuard = startStreamAuthorizationGuard(req, async () => {
      await projectForContainer(req, containerId);
    }, () => {
      if (closed) return;
      sendStreamEvent(res, { error: "Authorization was revoked.", done: true });
      cleanup();
      res.end();
    }).stop;

    child.stdout.on("data", sendChunk);
    child.stderr.on("data", sendChunk);
    child.on("error", (error) => {
      if (closed) return;
      sendStreamEvent(res, { error: error.message, done: true });
      cleanup();
      res.end();
    });
    child.on("close", (exitCode) => {
      if (closed) return;
      sendStreamEvent(res, { chunk: `\nLog stream closed${exitCode ? ` with exit code ${exitCode}` : ""}.\n`, done: true });
      closed = true;
      stopAuthorizationGuard();
      res.off("drain", resumeStreams);
      res.end();
    });

    res.once("close", cleanup);
  })
);

router.post(
  "/api/containers/:id/stop",
  requireAuth,
  asyncRoute(async (req, res) => {
    const id = routeParam(req, "id");
    const project = await projectForContainer(req, id, "runtime");
    await stopContainer(id);
    await recordAuditLog({ actor: actor(req), action: "container.stop", entityType: "container", entityId: id, projectId: project?.id });
    res.json({ ok: true });
  })
);

router.post(
  "/api/containers/:id/start",
  requireAuth,
  asyncRoute(async (req, res) => {
    const id = routeParam(req, "id");
    const project = await projectForContainer(req, id, "runtime");
    await startContainer(id);
    await recordAuditLog({ actor: actor(req), action: "container.start", entityType: "container", entityId: id, projectId: project?.id });
    res.json({ ok: true });
  })
);

router.post(
  "/api/containers/:id/restart",
  requireAuth,
  asyncRoute(async (req, res) => {
    const id = routeParam(req, "id");
    const project = await projectForContainer(req, id, "runtime");
    await restartContainer(id);
    await recordAuditLog({ actor: actor(req), action: "container.restart", entityType: "container", entityId: id, projectId: project?.id });
    res.json({ ok: true });
  })
);

export default router;
