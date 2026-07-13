import { Router } from "express";
import { requireAuth } from "../auth.js";
import { asyncRoute, routeParam, sendStreamEvent, startEventStream } from "../http-utils.js";
import { findDeployment, latestDeployments } from "../services/deployments.js";
import { deploymentEvents, type DeploymentLogEvent } from "../services/deployment-events.js";

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
    let ready = false;
    let closed = false;
    let waitingForDrain = false;
    let pendingEvent: DeploymentLogEvent | undefined;
    let unsubscribe = () => {};
    const cleanup = () => {
      if (closed) return;
      closed = true;
      pendingEvent = undefined;
      res.removeListener("drain", flushPending);
      unsubscribe();
    };
    const deliver = (event: DeploymentLogEvent) => {
      if (closed) return;
      if (!ready || waitingForDrain) {
        // Deployment updates contain the full retained log. Keeping only the
        // newest snapshot bounds memory without losing final state.
        pendingEvent = event;
        return;
      }
      waitingForDrain = !sendStreamEvent(res, {
        logs: event.logs,
        status: event.status,
        done: event.done
      });
      if (event.done) {
        cleanup();
        res.end();
      } else if (waitingForDrain) {
        res.once("drain", flushPending);
      }
    };
    function flushPending() {
      if (closed) return;
      waitingForDrain = false;
      const event = pendingEvent;
      pendingEvent = undefined;
      if (event) deliver(event);
    }

    // Subscribe before loading the snapshot so a terminal update cannot be
    // lost between the database read and listener registration.
    unsubscribe = deploymentEvents.onLogUpdate(id, deliver);
    res.once("close", cleanup);

    try {
      const initial = await findDeployment(id);
      if (closed) return;
      if (!initial) {
        cleanup();
        res.status(404).json({ message: "Deployment not found." });
        return;
      }

      startEventStream(res);
      ready = true;
      deliver({
        deploymentId: id,
        logs: initial.logs,
        status: initial.status,
        done: initial.status !== "running"
      });

      if (closed) return;
      const pending = pendingEvent;
      pendingEvent = undefined;
      if (pending) deliver(pending);
    } catch (error) {
      cleanup();
      throw error;
    }
  })
);

export default router;
