import cookieParser from "cookie-parser";
import express from "express";
import fs from "node:fs";
import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { z } from "zod";
import { clearSessionCookie, currentUser, requireAuth, setSessionCookie, verifyAdminPassword } from "./auth.js";
import { config, warnOnUnsafeDefaults } from "./config.js";
import { db, migrate, pool } from "./db/index.js";
import { projects } from "./db/schema.js";
import { asyncRoute, routeParam, sendStreamEvent, startEventStream } from "./http-utils.js";
import { logger } from "./logger.js";
import {
  backupInput,
  deploymentInput,
  envInput,
  envVariablesInput,
  projectInput,
  r2SettingsInput,
  rollbackInput,
  workerDeploymentUpdateInput,
  workerHeartbeatInput,
  workerLogInput,
  workerRegisterInput
} from "./route-schemas.js";
import { listAuditLogs, recordAuditLog } from "./services/audit.js";
import { createPostgresBackup, deleteBackup, getBackup, listBackups, listPostgresBackupTargets, markBackupDownloaded, restorePostgresBackupTarget, uploadBackupToR2 } from "./services/backups.js";
import { cleanupDocker, containerLogs, listContainers, previewDockerCleanup, restartContainer, stopContainer } from "./services/docker.js";
import {
  appendDeploymentLog,
  findDeployment,
  findDeploymentForNode,
  finishDeployment,
  latestDeployments,
  recoverInterruptedDeployments,
  rollbackTargetForProject,
  startDeployment,
  updateDeploymentMetadata
} from "./services/deployments.js";
import { ensureLocalMasterNode, listNodes, markNodeSeen, nextWorkerDeployment, nodeForWorkerToken, registerWorker } from "./services/nodes.js";
import { previewEnvContent, previewProjectEnv, readProjectEnv, readProjectEnvVariables, writeProjectEnv, writeProjectEnvVariables } from "./services/project-env.js";
import { restartProjectCompose, stopProjectCompose } from "./services/project-runtime.js";
import { createProject, deleteProject, getProject, listProjectsWithContainerCounts, updateProject } from "./services/projects.js";
import { managedSshKeyStatus, saveManagedSshPrivateKey } from "./services/ssh.js";
import { healthStatus, systemUsage } from "./services/system.js";
import { publicR2Settings, saveR2Settings } from "./services/settings.js";
import { constantTimeEqual } from "./services/tokens.js";
import { githubBranchFromRef, githubPayloadFromRequestBody, githubWebhookPayloadInput, projectDeployBranch, verifyGithubSignature } from "./services/github-webhooks.js";

const app = express();

type RawBodyRequest = express.Request & { rawBody?: Buffer };

function captureRawBody(req: express.Request, _res: express.Response, buffer: Buffer) {
  (req as RawBodyRequest).rawBody = Buffer.from(buffer);
}

app.use(express.json({ limit: "1mb", verify: captureRawBody }));
app.use(express.urlencoded({ extended: false, limit: "1mb", verify: captureRawBody }));
app.use(cookieParser());

function actor(req: express.Request) {
  return currentUser(req)?.username ?? "admin";
}

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

async function saveRequestBodyToTempFile(req: express.Request) {
  await fs.promises.mkdir(config.backupsDir, { recursive: true, mode: 0o700 });
  const filenameHeader = req.header("x-filename") ?? "uploaded-dump.sql";
  const originalFilename = path.basename(decodeURIComponent(filenameHeader)).replace(/[^\w .-]+/g, "-") || "uploaded-dump.sql";
  const filePath = path.join(config.backupsDir, `.restore-${Date.now()}-${Math.random().toString(16).slice(2)}-${originalFilename}`);
  let received = 0;

  await new Promise<void>((resolve, reject) => {
    const output = fs.createWriteStream(filePath, { mode: 0o600 });
    const fail = (error: Error) => {
      req.destroy();
      output.destroy();
      reject(error);
    };

    req.on("data", (chunk: Buffer) => {
      received += chunk.byteLength;
      if (received > config.backupUploadMaxBytes) {
        fail(new Error(`Dump upload is larger than ${config.backupUploadMaxBytes} bytes.`));
      }
    });
    req.on("error", fail);
    output.on("error", fail);
    output.on("finish", resolve);
    req.pipe(output);
  });

  if (received === 0) {
    await fs.promises.rm(filePath, { force: true });
    throw new Error("Upload a Postgres dump file first.");
  }

  return { filePath, originalFilename, size: received };
}

app.post(
  "/api/auth/login",
  asyncRoute(async (req, res) => {
    const body = z.object({ username: z.string(), password: z.string() }).parse(req.body);
    const ok = await verifyAdminPassword(body.username, body.password);
    if (!ok) {
      await recordAuditLog({ actor: body.username || "unknown", action: "auth.login.failed", entityType: "auth", metadata: { username: body.username } });
      res.status(401).json({ message: "Invalid username or password." });
      return;
    }
    setSessionCookie(res);
    await recordAuditLog({ actor: config.adminUsername, action: "auth.login.success", entityType: "auth", metadata: { username: config.adminUsername } });
    res.json({ username: config.adminUsername });
  })
);

app.post("/api/auth/logout", asyncRoute(async (req, res) => {
  await recordAuditLog({ actor: actor(req), action: "auth.logout", entityType: "auth" });
  clearSessionCookie(res);
  res.json({ ok: true });
}));

app.get("/api/auth/me", (req, res) => {
  const user = currentUser(req);
  if (!user) {
    res.status(401).json({ message: "Authentication required." });
    return;
  }
  res.json({ username: user.username });
});

app.get(
  "/api/health",
  asyncRoute(async (_req, res) => {
    const health = await healthStatus();
    res.status(health.ok ? 200 : 503).json(health);
  })
);

app.post(
  "/api/workers/register",
  asyncRoute(async (req, res) => {
    const body = workerRegisterInput.parse(req.body ?? {});
    if (!config.workerJoinToken || !constantTimeEqual(body.joinToken, config.workerJoinToken)) {
      res.status(401).json({ message: "Invalid worker join token." });
      return;
    }
    const result = await registerWorker(body);
    await recordAuditLog({ actor: "worker", action: "node.register", entityType: "deployment_node", entityId: result.node.id, metadata: { name: result.node.name } });
    res.status(201).json({ node: result.node, token: result.token });
  })
);

app.post(
  "/api/workers/heartbeat",
  requireWorker(async (req, res, node) => {
    const body = workerHeartbeatInput.parse(req.body ?? {});
    const updated = await markNodeSeen(node, body);
    res.json({ ok: true, node: updated });
  })
);

app.get(
  "/api/workers/jobs/next",
  requireWorker(async (_req, res, node) => {
    await markNodeSeen(node);
    const job = await nextWorkerDeployment(node.id);
    res.json(job ?? null);
  })
);

app.post(
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

app.patch(
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

app.get(
  "/api/nodes",
  requireAuth,
  asyncRoute(async (_req, res) => {
    res.json(await listNodes());
  })
);

app.post(
  "/api/nodes/join-token",
  requireAuth,
  asyncRoute(async (req, res) => {
    if (!config.workerJoinToken) {
      res.status(400).json({ message: "Set WORKER_JOIN_TOKEN on the master before connecting workers." });
      return;
    }
    const command = `curl -fsSL https://raw.githubusercontent.com/kodin00/yanto/master/scripts/install.sh | sudo bash -s -- worker --master ${config.appBaseUrl} --join-token ${config.workerJoinToken}`;
    await recordAuditLog({ actor: actor(req), action: "node.join_token.view", entityType: "deployment_node" });
    res.json({ token: config.workerJoinToken, command });
  })
);

app.get(
  "/api/projects",
  requireAuth,
  asyncRoute(async (_req, res) => {
    res.json(await listProjectsWithContainerCounts());
  })
);

app.post(
  "/api/projects",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = projectInput.parse(req.body);
    const project = await createProject(body);
    await recordAuditLog({ actor: actor(req), action: "project.create", entityType: "project", entityId: project.id, projectId: project.id });
    res.status(201).json(project);
  })
);

app.patch(
  "/api/projects/:id",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = projectInput.partial().parse(req.body);
    const project = await updateProject(routeParam(req, "id"), body);
    if (!project) {
      res.status(404).json({ message: "Project not found." });
      return;
    }
    await recordAuditLog({ actor: actor(req), action: "project.update", entityType: "project", entityId: project.id, projectId: project.id });
    res.json(project);
  })
);

app.delete(
  "/api/projects/:id",
  requireAuth,
  asyncRoute(async (req, res) => {
    const id = routeParam(req, "id");
    await deleteProject(id);
    await recordAuditLog({ actor: actor(req), action: "project.delete", entityType: "project", entityId: id });
    res.status(204).end();
  })
);

app.post(
  "/api/projects/:id/deploy",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = deploymentInput.parse(req.body ?? {});
    const projectId = routeParam(req, "id");
    const project = await getProject(projectId);
    if (!project) {
      res.status(404).json({ message: "Project not found." });
      return;
    }
    if (!project.manualDeployEnabled) {
      res.status(403).json({ message: "Manual deployments are disabled for this project." });
      return;
    }
    const result = await startDeployment(projectId, "manual", body);
    await recordAuditLog({
      actor: actor(req),
      action: "deployment.start",
      entityType: "deployment",
      entityId: result.deployment.id,
      projectId,
      metadata: { trigger: "manual", targetRef: body.targetRef ?? null, reused: result.reused }
    });
    res.status(result.reused ? 200 : 202).json(result);
  })
);

app.post(
  "/api/projects/:id/rollback",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = rollbackInput.parse(req.body ?? {});
    const projectId = routeParam(req, "id");
    const target = await rollbackTargetForProject(projectId, body.deploymentId, body.targetRef);
    const result = await startDeployment(projectId, "rollback", {
      targetRef: target.targetRef,
      rollbackFromDeploymentId: target.rollbackFromDeploymentId ?? undefined
    });
    await recordAuditLog({
      actor: actor(req),
      action: "deployment.rollback",
      entityType: "deployment",
      entityId: result.deployment.id,
      projectId,
      metadata: { targetRef: target.targetRef, rollbackFromDeploymentId: target.rollbackFromDeploymentId, reused: result.reused }
    });
    res.status(result.reused ? 200 : 202).json(result);
  })
);

app.get(
  "/api/projects/:id/env",
  requireAuth,
  asyncRoute(async (req, res) => {
    const project = await getProject(routeParam(req, "id"));
    if (!project) {
      res.status(404).json({ message: "Project not found." });
      return;
    }
    res.json(await readProjectEnvVariables(project, typeof req.query.envFile === "string" ? req.query.envFile : undefined));
  })
);

app.get(
  "/api/projects/:id/env/content",
  requireAuth,
  asyncRoute(async (req, res) => {
    const project = await getProject(routeParam(req, "id"));
    if (!project) {
      res.status(404).json({ message: "Project not found." });
      return;
    }
    res.json(await readProjectEnv(project, typeof req.query.envFile === "string" ? req.query.envFile : undefined));
  })
);

app.get(
  "/api/projects/:id/env/preview",
  requireAuth,
  asyncRoute(async (req, res) => {
    const project = await getProject(routeParam(req, "id"));
    if (!project) {
      res.status(404).json({ message: "Project not found." });
      return;
    }
    res.json(await previewProjectEnv(project, typeof req.query.envFile === "string" ? req.query.envFile : undefined));
  })
);

app.post(
  "/api/projects/:id/env/preview",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = envInput.parse(req.body);
    res.json(previewEnvContent(body.content, body.envFile ?? ".env"));
  })
);

app.put(
  "/api/projects/:id/env",
  requireAuth,
  asyncRoute(async (req, res) => {
    const project = await getProject(routeParam(req, "id"));
    if (!project) {
      res.status(404).json({ message: "Project not found." });
      return;
    }
    const body = envInput.parse(req.body);
    const preview = await writeProjectEnv(project, body.content, body.envFile);
    await recordAuditLog({
      actor: actor(req),
      action: "project.env.write",
      entityType: "project",
      entityId: project.id,
      projectId: project.id,
      metadata: { envFile: preview.envFile, entryCount: preview.entryCount }
    });
    res.json(preview);
  })
);

app.patch(
  "/api/projects/:id/env",
  requireAuth,
  asyncRoute(async (req, res) => {
    const project = await getProject(routeParam(req, "id"));
    if (!project) {
      res.status(404).json({ message: "Project not found." });
      return;
    }
    const body = envVariablesInput.parse(req.body);
    const preview = await writeProjectEnvVariables(project, body.variables, body.envFile);
    await recordAuditLog({
      actor: actor(req),
      action: "project.env.write",
      entityType: "project",
      entityId: project.id,
      projectId: project.id,
      metadata: { envFile: preview.envFile, entryCount: preview.entryCount }
    });
    res.json({ ok: true, preview });
  })
);

app.post(
  "/api/projects/:id/stop",
  requireAuth,
  asyncRoute(async (req, res) => {
    const project = await getProject(routeParam(req, "id"));
    if (!project) {
      res.status(404).json({ message: "Project not found." });
      return;
    }
    if (project.targetNodeId !== config.localNodeId) {
      res.status(400).json({ message: "Stopping projects on worker nodes is not supported yet." });
      return;
    }
    const logs = await stopProjectCompose(project);
    await recordAuditLog({ actor: actor(req), action: "project.compose.stop", entityType: "project", entityId: project.id, projectId: project.id });
    res.json({ ok: true, logs });
  })
);

app.post(
  "/api/projects/:id/restart",
  requireAuth,
  asyncRoute(async (req, res) => {
    const project = await getProject(routeParam(req, "id"));
    if (!project) {
      res.status(404).json({ message: "Project not found." });
      return;
    }
    if (project.targetNodeId !== config.localNodeId) {
      res.status(400).json({ message: "Restarting projects on worker nodes is not supported yet." });
      return;
    }
    const logs = await restartProjectCompose(project);
    await recordAuditLog({ actor: actor(req), action: "project.compose.restart", entityType: "project", entityId: project.id, projectId: project.id });
    res.json({ ok: true, logs });
  })
);

app.get(
  "/api/backups",
  requireAuth,
  asyncRoute(async (req, res) => {
    const limit = Math.min(200, Math.max(1, Number(req.query.limit ?? 50)));
    res.json(await listBackups(limit));
  })
);

app.get(
  "/api/backups/postgres-targets",
  requireAuth,
  asyncRoute(async (_req, res) => {
    res.json(await listPostgresBackupTargets());
  })
);

app.post(
  "/api/backups",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = backupInput.parse(req.body ?? {});
    const backup = await createPostgresBackup(body.containerId);
    await recordAuditLog({
      actor: actor(req),
      action: "backup.create",
      entityType: "backup",
      entityId: backup.id,
      projectId: backup.projectId,
      metadata: { kind: backup.kind, status: backup.status, fileSizeBytes: backup.fileSizeBytes, note: backup.note }
    });
    res.status(201).json(backup);
  })
);

app.post(
  "/api/backups/postgres-targets/:containerId/restore",
  requireAuth,
  asyncRoute(async (req, res) => {
    const upload = await saveRequestBodyToTempFile(req);
    try {
      const target = await restorePostgresBackupTarget(routeParam(req, "containerId"), upload.filePath, upload.originalFilename);
      await recordAuditLog({
        actor: actor(req),
        action: "backup.restore",
        entityType: "container",
        entityId: target.containerId,
        projectId: target.projectId,
        metadata: { filename: upload.originalFilename, size: upload.size, database: target.databaseName }
      });
      res.json({ ok: true, target });
    } finally {
      await fs.promises.rm(upload.filePath, { force: true });
    }
  })
);

app.post(
  "/api/backups/:id/upload-r2",
  requireAuth,
  asyncRoute(async (req, res) => {
    const { backup, result } = await uploadBackupToR2(routeParam(req, "id"));
    await recordAuditLog({
      actor: actor(req),
      action: "backup.r2_upload",
      entityType: "backup",
      entityId: backup.id,
      projectId: backup.projectId,
      metadata: { bucket: result.bucket, key: result.key, size: result.size }
    });
    res.json(result);
  })
);

app.get(
  "/api/backups/:id/download",
  requireAuth,
  asyncRoute(async (req, res) => {
    const backup = await getBackup(routeParam(req, "id"));
    if (!backup || backup.status !== "success") {
      res.status(404).json({ message: "Backup not found." });
      return;
    }
    await markBackupDownloaded(backup.id);
    await recordAuditLog({ actor: actor(req), action: "backup.download", entityType: "backup", entityId: backup.id, metadata: { filename: backup.filename } });
    res.download(backup.filePath, backup.filename);
  })
);

app.delete(
  "/api/backups/:id",
  requireAuth,
  asyncRoute(async (req, res) => {
    const backup = await deleteBackup(routeParam(req, "id"));
    if (!backup) {
      res.status(404).json({ message: "Backup not found." });
      return;
    }
    await recordAuditLog({ actor: actor(req), action: "backup.delete", entityType: "backup", entityId: backup.id, projectId: backup.projectId, metadata: { filename: backup.filename } });
    res.status(204).end();
  })
);

app.post(
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

app.post(
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

app.get(
  "/api/deployments",
  requireAuth,
  asyncRoute(async (_req, res) => {
    res.json(await latestDeployments());
  })
);

app.get(
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

app.get(
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

app.get(
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

app.get(
  "/api/containers",
  requireAuth,
  asyncRoute(async (_req, res) => {
    res.json(await listContainers());
  })
);

app.get(
  "/api/containers/:id/logs",
  requireAuth,
  asyncRoute(async (req, res) => {
    res.type("text/plain").send(await containerLogs(routeParam(req, "id")));
  })
);

app.get(
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

app.post(
  "/api/containers/:id/stop",
  requireAuth,
  asyncRoute(async (req, res) => {
    const id = routeParam(req, "id");
    await stopContainer(id);
    await recordAuditLog({ actor: actor(req), action: "container.stop", entityType: "container", entityId: id });
    res.json({ ok: true });
  })
);

app.post(
  "/api/containers/:id/restart",
  requireAuth,
  asyncRoute(async (req, res) => {
    const id = routeParam(req, "id");
    await restartContainer(id);
    await recordAuditLog({ actor: actor(req), action: "container.restart", entityType: "container", entityId: id });
    res.json({ ok: true });
  })
);

app.get(
  "/api/audit-logs",
  requireAuth,
  asyncRoute(async (req, res) => {
    const limit = Math.min(500, Math.max(1, Number(req.query.limit ?? 100)));
    res.json(await listAuditLogs(limit, typeof req.query.projectId === "string" ? req.query.projectId : undefined));
  })
);

app.get(
  "/api/system/usage",
  requireAuth,
  asyncRoute(async (_req, res) => {
    res.json(await systemUsage());
  })
);

app.get(
  "/api/system/logs",
  requireAuth,
  asyncRoute(async (_req, res) => {
    res.type("text/plain").send(logger.history() || "No system log entries recorded yet.");
  })
);

app.get(
  "/api/system/cleanup/preview",
  requireAuth,
  asyncRoute(async (_req, res) => {
    res.json({ logs: await previewDockerCleanup() });
  })
);

app.post(
  "/api/system/cleanup",
  requireAuth,
  asyncRoute(async (req, res) => {
    const logs = await cleanupDocker();
    await recordAuditLog({ actor: actor(req), action: "system.cleanup", entityType: "system", metadata: { protected: true } });
    res.json({ logs });
  })
);

app.get(
  "/api/settings",
  requireAuth,
  asyncRoute(async (_req, res) => {
    const [count, sshKey, r2] = await Promise.all([db.select().from(projects), managedSshKeyStatus(), publicR2Settings()]);
    res.json({
      projectsRoot: config.projectsRoot,
      hostProjectsRoot: config.hostProjectsRoot,
      sshKeysDir: config.sshKeysDir,
      appBaseUrl: config.appBaseUrl,
      projectCount: count.length,
      sshKey,
      r2
    });
  })
);

app.post(
  "/api/settings/r2",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = r2SettingsInput.parse(req.body ?? {});
    const r2 = await saveR2Settings(body);
    await recordAuditLog({ actor: actor(req), action: "settings.r2.save", entityType: "settings", metadata: { enabled: r2.enabled, bucket: r2.bucket, prefix: r2.prefix } });
    res.json({ ok: true, r2 });
  })
);

app.post(
  "/api/settings/ssh-key",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = z.object({ privateKey: z.string().min(1) }).parse(req.body);
    const sshKey = await saveManagedSshPrivateKey(body.privateKey);
    await recordAuditLog({ actor: actor(req), action: "settings.ssh_key.save", entityType: "settings" });
    res.json({ ok: true, sshKey });
  })
);

app.use((error: unknown, _req: express.Request, res: express.Response, next: express.NextFunction) => {
  void next;
  const message = error instanceof Error ? error.message : "Unexpected server error.";
  logger.error("request failed", { error: message });
  res.status(400).json({ message });
});

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const clientDir =
  [path.resolve(__dirname, "../client"), path.resolve(__dirname, "../../client"), path.resolve(__dirname, "../../dist/client")].find((candidate) =>
    fs.existsSync(path.join(candidate, "index.html"))
  ) ?? path.resolve(__dirname, "../client");
app.use(express.static(clientDir));
app.get(/.*/, (_req, res) => {
  res.sendFile(path.join(clientDir, "index.html"));
});

async function main() {
  warnOnUnsafeDefaults();
  await migrate();
  await ensureLocalMasterNode();
  await recoverInterruptedDeployments();
  app.listen(config.port, () => {
    logger.info("server started", { port: config.port });
  });
}

process.on("SIGINT", async () => {
  await pool.end();
  process.exit(0);
});

main().catch((error) => {
  logger.error("server failed to start", { error: error instanceof Error ? error.message : String(error) });
  process.exit(1);
});
