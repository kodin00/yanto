import { Router } from "express";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import express from "express";
import { requireAuth } from "../auth.js";
import { accessibleProjectIds, assertProjectPermission, filterByProjectPermission, isOwner, listAccessibleContainers, projectForContainer, revalidateRequestPrincipal } from "../authorization.js";
import { config } from "../config.js";
import { HttpError, asyncRoute, actor, routeParam } from "../http-utils.js";
import { backupInput, backupPolicyInput, backupPolicyUpdateInput } from "../route-schemas.js";
import { recordAuditLog } from "../services/audit.js";
import { createBackupPolicy, deleteBackupEverywhere, deleteBackupPolicy, getBackupPolicy, listBackupPolicies, listBackupReplicas, materializeBackupFile, replicateBackupReplica, runBackupPolicy, updateBackupPolicy } from "../services/backup-policies.js";
import { createPostgresBackup, getBackup, listBackups, listBackupsForProjects, listPostgresBackupTargets, markBackupDownloaded, restorePostgresBackupTarget, uploadBackupToR2 } from "../services/backups.js";

const router = Router();

function requireBackupOwner(req: express.Request) {
  if (!isOwner(req)) throw new HttpError(403, "Only the owner can manage multi-node backup policies.");
}

async function saveRequestBodyToTempFile(req: express.Request) {
  await fs.promises.mkdir(config.backupsDir, { recursive: true, mode: 0o700 });
  const filenameHeader = req.header("x-filename") ?? "uploaded-dump.sql";
  let decodedFilename = filenameHeader;
  try {
    decodedFilename = decodeURIComponent(filenameHeader);
  } catch {
    // Treat malformed percent escapes as literal filename characters.
  }
  const originalFilename = (path.basename(decodedFilename).replace(/[^\w .-]+/g, "-").slice(0, 160) || "uploaded-dump.sql");
  const filePath = path.join(config.backupsDir, `.restore-${crypto.randomUUID()}-${originalFilename}`);
  const contentLength = Number(req.header("content-length") ?? "");
  if (Number.isFinite(contentLength) && contentLength > config.backupUploadMaxBytes) {
    req.resume();
    throw new HttpError(413, `Dump upload is larger than ${config.backupUploadMaxBytes} bytes.`);
  }
  let received = 0;

  try {
    await new Promise<void>((resolve, reject) => {
      const output = fs.createWriteStream(filePath, { flags: "wx", mode: 0o600 });
      let failed = false;
      const fail = (error: Error) => {
        if (failed) return;
        failed = true;
        req.unpipe(output);
        output.destroy();
        // Drain the remaining request body so the server can return a useful
        // 413 response without retaining the rejected upload in memory.
        req.resume();
        reject(error);
      };

      req.on("data", (chunk: Buffer) => {
        received += chunk.byteLength;
        if (received > config.backupUploadMaxBytes) {
          fail(new HttpError(413, `Dump upload is larger than ${config.backupUploadMaxBytes} bytes.`));
        }
      });
      req.once("aborted", () => fail(new Error("Dump upload was interrupted.")));
      req.once("error", fail);
      output.once("error", fail);
      output.once("finish", () => {
        if (!failed) resolve();
      });
      req.pipe(output);
    });
  } catch (error) {
    await fs.promises.rm(filePath, { force: true });
    throw error;
  }

  if (received === 0) {
    await fs.promises.rm(filePath, { force: true });
    throw new Error("Upload a Postgres dump file first.");
  }

  return { filePath, originalFilename, size: received };
}

router.get(
  "/api/backups",
  requireAuth,
  asyncRoute(async (req, res) => {
    const limit = Math.min(200, Math.max(1, Number(req.query.limit ?? 50)));
    const projectIds = accessibleProjectIds(req);
    if (projectIds === null) {
      res.json(await listBackups(limit));
      return;
    }
    const allowedIds = filterByProjectPermission(req, [...projectIds].map((projectId) => ({ projectId })), "backups").map((row) => row.projectId);
    res.json(await listBackupsForProjects(allowedIds, limit));
  })
);

router.get(
  "/api/backups/postgres-targets",
  requireAuth,
  asyncRoute(async (req, res) => {
    const targets = await listPostgresBackupTargets();
    if (isOwner(req)) {
      res.json(targets);
      return;
    }
    const accessibleContainerIds = new Set((await listAccessibleContainers(req)).map((container) => container.id));
    res.json(filterByProjectPermission(req, targets, "backups").filter(
      (target) => target.nodeId !== config.localNodeId || accessibleContainerIds.has(target.containerId)
    ));
  })
);

router.get(
  "/api/backups/policies",
  requireAuth,
  asyncRoute(async (req, res) => {
    requireBackupOwner(req);
    res.json(await listBackupPolicies());
  })
);

router.post(
  "/api/backups/policies",
  requireAuth,
  asyncRoute(async (req, res) => {
    requireBackupOwner(req);
    const policy = await createBackupPolicy(backupPolicyInput.parse(req.body ?? {}));
    await recordAuditLog({
      actor: actor(req),
      action: "backup.policy.create",
      entityType: "backup_policy",
      entityId: policy?.id,
      metadata: { sourceNodeId: policy?.sourceNodeId, destinationNodeIds: policy?.destinationNodeIds ?? [] }
    });
    res.status(201).json(policy);
  })
);

router.patch(
  "/api/backups/policies/:id",
  requireAuth,
  asyncRoute(async (req, res) => {
    requireBackupOwner(req);
    const id = routeParam(req, "id");
    const policy = await updateBackupPolicy(id, backupPolicyUpdateInput.parse(req.body ?? {}));
    if (!policy) {
      res.status(404).json({ message: "Backup policy not found." });
      return;
    }
    await recordAuditLog({ actor: actor(req), action: "backup.policy.update", entityType: "backup_policy", entityId: id });
    res.json(policy);
  })
);

router.delete(
  "/api/backups/policies/:id",
  requireAuth,
  asyncRoute(async (req, res) => {
    requireBackupOwner(req);
    const id = routeParam(req, "id");
    const existing = await getBackupPolicy(id);
    if (!existing) {
      res.status(404).json({ message: "Backup policy not found." });
      return;
    }
    await deleteBackupPolicy(id);
    await recordAuditLog({ actor: actor(req), action: "backup.policy.delete", entityType: "backup_policy", entityId: id });
    res.status(204).end();
  })
);

router.post(
  "/api/backups/policies/:id/run",
  requireAuth,
  asyncRoute(async (req, res) => {
    requireBackupOwner(req);
    const id = routeParam(req, "id");
    if (!await getBackupPolicy(id)) {
      res.status(404).json({ message: "Backup policy not found." });
      return;
    }
    const backup = await runBackupPolicy(id);
    await recordAuditLog({
      actor: actor(req),
      action: "backup.policy.run",
      entityType: "backup_policy",
      entityId: id,
      metadata: { backupId: backup?.id, status: backup?.status }
    });
    res.status(202).json(backup);
  })
);

router.get(
  "/api/backups/:id/replicas",
  requireAuth,
  asyncRoute(async (req, res) => {
    requireBackupOwner(req);
    const id = routeParam(req, "id");
    if (!await getBackup(id)) {
      res.status(404).json({ message: "Backup not found." });
      return;
    }
    res.json(await listBackupReplicas(id));
  })
);

router.post(
  "/api/backups/replicas/:id/retry",
  requireAuth,
  asyncRoute(async (req, res) => {
    requireBackupOwner(req);
    const replica = await replicateBackupReplica(routeParam(req, "id"));
    await recordAuditLog({
      actor: actor(req),
      action: "backup.replica.retry",
      entityType: "backup_replica",
      entityId: replica.id,
      metadata: { backupId: replica.backupId, destinationNodeId: replica.destinationNodeId, status: replica.status }
    });
    res.json(replica);
  })
);

router.post(
  "/api/backups",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = backupInput.parse(req.body ?? {});
    if (body.sourceNodeId && body.sourceNodeId !== config.localNodeId) {
      throw new HttpError(400, "Use an automatic backup policy for a remote Postgres target.");
    }
    if (body.containerId) await projectForContainer(req, body.containerId, "backups");
    else if (!isOwner(req)) throw new HttpError(403, "Only the owner can back up the Yanto database.");
    const backup = await createPostgresBackup(body.containerId, { sourceNodeId: body.sourceNodeId });
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

router.post(
  "/api/backups/postgres-targets/:containerId/restore",
  requireAuth,
  asyncRoute(async (req, res) => {
    await projectForContainer(req, routeParam(req, "containerId"), "backups");
    const upload = await saveRequestBodyToTempFile(req);
    try {
      const containerId = routeParam(req, "containerId");
      await revalidateRequestPrincipal(req);
      await projectForContainer(req, containerId, "backups");
      const target = await restorePostgresBackupTarget(containerId, upload.filePath, upload.originalFilename);
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

router.post(
  "/api/backups/:id/upload-r2",
  requireAuth,
  asyncRoute(async (req, res) => {
    const id = routeParam(req, "id");
    const existing = await getBackup(id);
    if (!existing?.projectId && !isOwner(req)) throw new HttpError(404, "Backup not found.");
    if (existing?.projectId) assertProjectPermission(req, existing.projectId, "backups");
    await materializeBackupFile(id);
    const { backup, result } = await uploadBackupToR2(id);
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

router.post(
  "/api/backups/:id/restore/:containerId",
  requireAuth,
  asyncRoute(async (req, res) => {
    const backup = await getBackup(routeParam(req, "id"));
    if (!backup || backup.status !== "success") {
      res.status(404).json({ message: "Backup not found." });
      return;
    }
    if (!backup.projectId && !isOwner(req)) throw new HttpError(404, "Backup not found.");
    if (backup.projectId) assertProjectPermission(req, backup.projectId, "backups");
    const containerId = routeParam(req, "containerId");
    await projectForContainer(req, containerId, "backups");
    const filePath = await materializeBackupFile(backup.id);
    await revalidateRequestPrincipal(req);
    await projectForContainer(req, containerId, "backups");
    const target = await restorePostgresBackupTarget(containerId, filePath, backup.filename);
    await recordAuditLog({
      actor: actor(req),
      action: "backup.restore",
      entityType: "backup",
      entityId: backup.id,
      projectId: target.projectId,
      metadata: { filename: backup.filename, targetContainerId: target.containerId, database: target.databaseName }
    });
    res.json({ ok: true, target, backupId: backup.id });
  })
);

router.get(
  "/api/backups/:id/download",
  requireAuth,
  asyncRoute(async (req, res) => {
    const backup = await getBackup(routeParam(req, "id"));
    if (!backup || backup.status !== "success") {
      res.status(404).json({ message: "Backup not found." });
      return;
    }
    if (!backup.projectId && !isOwner(req)) throw new HttpError(404, "Backup not found.");
    if (backup.projectId) assertProjectPermission(req, backup.projectId, "backups");
    const filePath = await materializeBackupFile(backup.id);
    await markBackupDownloaded(backup.id);
    await recordAuditLog({ actor: actor(req), action: "backup.download", entityType: "backup", entityId: backup.id, projectId: backup.projectId, metadata: { filename: backup.filename } });
    res.download(filePath, backup.filename);
  })
);

router.delete(
  "/api/backups/:id",
  requireAuth,
  asyncRoute(async (req, res) => {
    const id = routeParam(req, "id");
    const existing = await getBackup(id);
    if (!existing?.projectId && !isOwner(req)) throw new HttpError(404, "Backup not found.");
    if (existing?.projectId) assertProjectPermission(req, existing.projectId, "backups");
    const backup = await deleteBackupEverywhere(id);
    if (!backup) {
      res.status(404).json({ message: "Backup not found." });
      return;
    }
    await recordAuditLog({ actor: actor(req), action: "backup.delete", entityType: "backup", entityId: backup.id, projectId: backup.projectId, metadata: { filename: backup.filename } });
    res.status(204).end();
  })
);

export default router;
