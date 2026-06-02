import { Router } from "express";
import { requireAuth } from "../auth.js";
import { asyncRoute, routeParam, sendStreamEvent, startEventStream } from "../http-utils.js";
import { findDeployment, latestDeployments } from "../services/deployments.js";
import { deploymentEvents } from "../services/deployment-events.js";

const router = Router();

router.get(
  "/api/deployments",
  requireAuth,
  asyncRoute(async (req, res) => {
    const requestedLimit = Number(req.query.limit ?? 500);
    const limit = Number.isFinite(requestedLimit) ? Math.min(Math.max(Math.trunc(requestedLimit), 1), 500) : 500;
    res.json(await latestDeployments(limit));
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

    // Send initial state
    const done = initial.status !== "running";
    sendStreamEvent(res, {
      logs: initial.logs,
      status: initial.status,
      done
    });

    if (done) {
      res.end();
      return;
    }

    // Subscribe to event bus instead of polling
    const unsubscribe = deploymentEvents.onLogUpdate(id, (event) => {
      sendStreamEvent(res, {
        logs: event.logs,
        status: event.status,
        done: event.done
      });
      if (event.done) {
        unsubscribe();
        res.end();
      }
    });

    req.on("close", () => {
      unsubscribe();
    });
  })
);

export default router;
