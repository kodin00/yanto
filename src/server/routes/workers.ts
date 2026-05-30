import { Router } from "express";
import type express from "express";
import { asyncRoute, routeParam } from "../http-utils.js";
import { workerDeploymentUpdateInput, workerHeartbeatInput, workerLogInput, workerRegisterInput } from "../route-schemas.js";
import { recordAuditLog } from "../services/audit.js";
import { appendDeploymentLog, findDeploymentForNode, finishDeployment, startDeployment, updateDeploymentMetadata } from "../services/deployments.js";
import { getProject } from "../services/projects.js";
import { githubBranchFromRef, githubPayloadFromRequestBody, githubWebhookPayloadInput, projectDeployBranch, verifyGithubSignature } from "../services/github-webhooks.js";
import { markNodeSeen, nextWorkerDeployment, nodeForWorkerToken, registerWorker } from "../services/nodes.js";
import { getWorkerJoinToken } from "../services/settings.js";
import { constantTimeEqual } from "../services/tokens.js";

type RawBodyRequest = express.Request & { rawBody?: Buffer };

const router = Router();

async function workerNode(req: express.Request) {
  const header = req.header("authorization") ?? "";
  const token = header.startsWith("Bearer ") ? header.slice("Bearer ".length) : "";
  if (!token) return null;
  return nodeForWorkerToken(token);
}

function requireWorker(handler: (req: express.Request, res: express.Response, node: NonNullable<Awaited<ReturnType<typeof workerNode>>>) => Promise<void>) {
  return asyncRoute(async (req, res) => {
    const node = await workerNode(req);
    if (!node) {
      res.status(401).json({ message: "Invalid worker token." });
      return;
    }
    await handler(req, res, node);
  });
}

router.post(
  "/api/workers/register",
  asyncRoute(async (req, res) => {
    const body = workerRegisterInput.parse(req.body ?? {});
    const joinToken = await getWorkerJoinToken();
    if (!joinToken || !constantTimeEqual(body.joinToken, joinToken)) {
      res.status(401).json({ message: "Invalid worker join token." });
      return;
    }
    const result = await registerWorker(body);
    await recordAuditLog({ actor: "worker", action: "node.register", entityType: "deployment_node", entityId: result.node.id, metadata: { name: result.node.name } });
    res.status(201).json({ node: result.node, token: result.token });
  })
);

router.post(
  "/api/workers/heartbeat",
  requireWorker(async (req, res, node) => {
    const body = workerHeartbeatInput.parse(req.body ?? {});
    const updated = await markNodeSeen(node, body);
    res.json({ ok: true, node: updated });
  })
);

router.get(
  "/api/workers/jobs/next",
  requireWorker(async (_req, res, node) => {
    await markNodeSeen(node);
    const job = await nextWorkerDeployment(node.id);
    res.json(job ?? null);
  })
);

router.post(
  "/api/workers/deployments/:id/logs",
  requireWorker(async (req, res, node) => {
    const id = routeParam(req, "id");
    const deployment = await findDeploymentForNode(id, node.id);
    if (!deployment) {
      res.status(404).json({ message: "Deployment not found for worker." });
      return;
    }
    const body = workerLogInput.parse(req.body ?? {});
    await appendDeploymentLog(id, body.chunk);
    res.json({ ok: true });
  })
);

router.patch(
  "/api/workers/deployments/:id",
  requireWorker(async (req, res, node) => {
    const id = routeParam(req, "id");
    const deployment = await findDeploymentForNode(id, node.id);
    if (!deployment) {
      res.status(404).json({ message: "Deployment not found for worker." });
      return;
    }
    const body = workerDeploymentUpdateInput.parse(req.body ?? {});
    if (body.commitSha !== undefined || body.commitMessage !== undefined || body.targetRef !== undefined) {
      await updateDeploymentMetadata(id, {
        commitSha: body.commitSha,
        commitMessage: body.commitMessage,
        targetRef: body.targetRef
      });
    }
    if (body.status) {
      await finishDeployment(id, body.status, body.exitCode ?? (body.status === "success" ? 0 : 1));
    }
    await markNodeSeen(node);
    res.json({ ok: true });
  })
);

router.post(
  "/deploy",
  asyncRoute(async (req, res) => {
    const id = String(req.query.id ?? "");
    const header = req.header("authorization") ?? "";
    const token = header.startsWith("Bearer ") ? header.slice("Bearer ".length) : "";
    const project = await getProject(id);
    if (!project || !token || !constantTimeEqual(project.deployToken, token)) {
      res.status(401).json({ message: "Invalid deployment token." });
      return;
    }
    if (!project.manualDeployEnabled) {
      res.status(403).json({ message: "Manual deployments are disabled for this project." });
      return;
    }
    const result = await startDeployment(id, "manual");
    await recordAuditLog({ action: "deployment.manual_api", entityType: "deployment", entityId: result.deployment.id, projectId: id, metadata: { reused: result.reused } });
    res.status(result.reused ? 200 : 202).json(result);
  })
);

router.post(
  "/webhooks/github",
  asyncRoute(async (req, res) => {
    const projectId = String(req.query.id ?? req.query.projectId ?? "");
    const project = await getProject(projectId);
    if (!project) {
      res.status(404).json({ message: "Project not found." });
      return;
    }

    if (!verifyGithubSignature(project.deployToken, (req as RawBodyRequest).rawBody, req.header("x-hub-signature-256") ?? undefined)) {
      res.status(401).json({ message: "Invalid GitHub signature." });
      return;
    }

    const event = req.header("x-github-event") ?? "";
    const delivery = req.header("x-github-delivery") ?? null;
    if (event === "ping") {
      res.json({ ok: true });
      return;
    }

    if (!project.githubWebhookEnabled) {
      res.status(202).json({ ok: true, ignored: "github_webhook_disabled" });
      return;
    }

    if (event !== "push") {
      res.status(202).json({ ok: true, ignored: "unsupported_event", event });
      return;
    }

    const payload = githubWebhookPayloadInput.parse(githubPayloadFromRequestBody(req.body));
    const branch = githubBranchFromRef(payload.ref);
    const expectedBranch = projectDeployBranch(project);
    const repository = payload.repository?.full_name ?? payload.repository?.name ?? null;

    if (payload.deleted) {
      res.status(202).json({ ok: true, ignored: "deleted_ref", branch, repository });
      return;
    }

    if (!branch || branch !== expectedBranch) {
      await recordAuditLog({
        actor: "github",
        action: "deployment.github_webhook.ignored",
        entityType: "project",
        entityId: project.id,
        projectId: project.id,
        metadata: { delivery, event, repository, branch, expectedBranch, ref: payload.ref ?? null }
      });
      res.status(202).json({ ok: true, ignored: "branch_mismatch", branch, expectedBranch, repository });
      return;
    }

    const targetRef = payload.after?.trim() || payload.ref;
    const result = await startDeployment(project.id, "github", { targetRef });
    await recordAuditLog({
      actor: "github",
      action: "deployment.github_webhook",
      entityType: "deployment",
      entityId: result.deployment.id,
      projectId: project.id,
      metadata: {
        delivery,
        event,
        repository,
        branch,
        defaultBranch: payload.repository?.default_branch ?? null,
        targetRef,
        reused: result.reused
      }
    });
    res.status(result.reused ? 200 : 202).json(result);
  })
);

export default router;
