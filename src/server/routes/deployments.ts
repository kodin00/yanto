import { Router } from "express";
import { requireAuth } from "../auth.js";
import { asyncRoute, routeParam, sendStreamEvent, startEventStream } from "../http-utils.js";
import { findDeployment, latestDeployments } from "../services/deployments.js";

const router = Router();

router.get(
  "/api/deployments",
  requireAuth,
  asyncRoute(async (_req, res) => {
    res.json(await latestDeployments());
  })
);

router.get(
  "/api/deployments/:id",
  requireAuth,
  asyncRoute(async (req, res) => {
    const deployment = await findDeployment(routeParam(req, "id"));
    if (!deployment) {
      res.status(404).json({ message: "Deployment not found." });
      return;
    }
    res.json(deployment);
  })
);

router.get(
  "/api/deployments/:id/logs",
  requireAuth,
  asyncRoute(async (req, res) => {
    const deployment = await findDeployment(routeParam(req, "id"));
    if (!deployment) {
      res.status(404).json({ message: "Deployment not found." });
      return;
    }
    res.type("text/plain").send(deployment.logs);
  })
);

router.get(
  "/api/deployments/:id/logs/stream",
  requireAuth,
  asyncRoute(async (req, res) => {
    const id = routeParam(req, "id");
    const initial = await findDeployment(id);
    if (!initial) {
      res.status(404).json({ message: "Deployment not found." });
      return;
    }

    startEventStream(res);
    let previousLogs = "";
    let closed = false;
    let timer: NodeJS.Timeout | null = null;

    const pushLatest = async () => {
      if (closed) return;
      const deployment = await findDeployment(id);
      if (!deployment) {
        sendStreamEvent(res, { logs: previousLogs, status: "missing", done: true });
        res.end();
        return;
      }
      if (deployment.logs !== previousLogs || deployment.status !== "running") {
        previousLogs = deployment.logs;
        sendStreamEvent(res, {
          logs: deployment.logs,
          status: deployment.status,
          done: deployment.status !== "running"
        });
      }
      if (deployment.status !== "running") {
        closed = true;
        if (timer) {
          clearInterval(timer);
        }
        res.end();
      }
    };

    await pushLatest();
    if (!closed) {
      timer = setInterval(() => {
        void pushLatest().catch((error) => {
          sendStreamEvent(res, { error: error instanceof Error ? error.message : "Unable to stream deployment logs.", done: true });
          res.end();
        });
      }, 700);
    }

    req.on("close", () => {
      closed = true;
      if (timer) {
        clearInterval(timer);
      }
    });
  })
);

export default router;
