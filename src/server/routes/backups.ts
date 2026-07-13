import { Router } from "express";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import express from "express";
import { requireAuth } from "../auth.js";
import { config } from "../config.js";
import { HttpError, asyncRoute, actor, routeParam } from "../http-utils.js";
import { backupInput } from "../route-schemas.js";
import { recordAuditLog } from "../services/audit.js";
import { createPostgresBackup, deleteBackup, getBackup, listBackups, listPostgresBackupTargets, markBackupDownloaded, restorePostgresBackupTarget, uploadBackupToR2 } from "../services/backups.js";

const router = Router();

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
    res.json(await listBackups(limit));
  })
);

router.get(
  "/api/backups/postgres-targets",
  requireAuth,
  asyncRoute(async (_req, res) => {
    res.json(await listPostgresBackupTargets());
  })
);

router.post(
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

router.post(
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

router.post(
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

router.get(
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

router.delete(
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

export default router;
