import { desc, eq, inArray, sql } from "drizzle-orm";
import { spawn } from "node:child_process";
import { createReadStream, createWriteStream } from "node:fs";
import fs from "node:fs/promises";
import path from "node:path";
import { pipeline } from "node:stream/promises";
import { createGunzip, createGzip } from "node:zlib";
import { config } from "../config.js";
import { db } from "../db/index.js";
import { backupReplicas, backups, deploymentNodes, type BackupRow } from "../db/schema.js";
import { calculateBackupChecksum, rsyncBackupFileWithRetry, type BackupDestinationConfig } from "./backup-replication.js";
import { listContainers } from "./docker.js";
import { listProjects } from "./projects.js";
import { runCommand } from "./commands.js";
import { uploadFileToR2 } from "./r2.js";
import { createId } from "./tokens.js";

function backupTimestamp() {
  return new Date().toISOString().replace(/[:.]/g, "-");
}

function safeFilenamePart(input: string) {
  return input
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 72);
}

async function runGzipStream(command: string, args: string[], filePath: string) {
  await fs.mkdir(path.dirname(filePath), { recursive: true, mode: 0o700 });

  await new Promise<void>((resolve, reject) => {
    const child = spawn(command, args, {
      env: process.env,
      shell: false,
      stdio: ["ignore", "pipe", "pipe"]
    });
    const gzip = createGzip({ level: 9 });
    const output = createWriteStream(filePath, { mode: 0o600 });
    const errors: string[] = [];
    let outputFinished = false;
    let childExitCode: number | null | undefined;
    let settled = false;

    const fail = (error: Error) => {
      if (settled) return;
      settled = true;
      reject(error);
    };

    const maybeResolve = () => {
      if (settled || !outputFinished || childExitCode !== 0) return;
      settled = true;
      resolve();
    };

    child.stderr.on("data", (chunk: Buffer) => errors.push(chunk.toString()));
    child.on("error", fail);
    gzip.on("error", fail);
    output.on("error", fail);
    output.on("finish", () => {
      outputFinished = true;
      maybeResolve();
    });

    child.stdout.pipe(gzip).pipe(output);
    child.on("close", (exitCode) => {
      childExitCode = exitCode;
      if (exitCode !== 0) {
        fail(new Error(errors.join("").trim() || `${command} failed with exit code ${exitCode ?? 1}.`));
        return;
      }
      maybeResolve();
    });
  });
}

async function runPgDumpGzip(filePath: string) {
  await runGzipStream("pg_dump", [config.databaseUrl], filePath);
}

async function runContainerPgDumpGzip(containerId: string, filePath: string) {
  await runGzipStream(
    "docker",
    [
      "exec",
      containerId,
      "sh",
      "-lc",
      'PGUSER="${POSTGRES_USER:-${PGUSER:-postgres}}"; PGDATABASE="${POSTGRES_DB:-${PGDATABASE:-$PGUSER}}"; export PGPASSWORD="${POSTGRES_PASSWORD:-${PGPASSWORD:-}}"; exec pg_dump -U "$PGUSER" "$PGDATABASE"'
    ],
    filePath
  );
}

async function isCustomPgDump(dumpPath: string, originalFilename: string) {
  const lower = originalFilename.toLowerCase().replace(/\.gz$/, "");
  if (lower.endsWith(".dump") || lower.endsWith(".backup")) {
    return true;
  }
  const handle = await fs.open(dumpPath, "r");
  try {
    const buffer = Buffer.alloc(5);
    const result = await handle.read(buffer, 0, buffer.length, 0);
    return result.bytesRead === 5 && buffer.toString() === "PGDMP";
  } finally {
    await handle.close();
  }
}

async function runContainerRestore(containerId: string, dumpPath: string, originalFilename: string) {
  const customDump = await isCustomPgDump(dumpPath, originalFilename);
  const source = createReadStream(dumpPath);
  const sqlStream = originalFilename.toLowerCase().endsWith(".gz") ? source.pipe(createGunzip()) : source;
  const restoreCommand = customDump
    ? 'exec pg_restore -U "$PGUSER" -d "$PGDATABASE" --clean --if-exists --no-owner'
    : 'exec psql -U "$PGUSER" -d "$PGDATABASE" -v ON_ERROR_STOP=1';

  await new Promise<void>((resolve, reject) => {
    const child = spawn(
      "docker",
      [
        "exec",
        "-i",
        containerId,
        "sh",
        "-lc",
        [
          'PGUSER="${POSTGRES_USER:-${PGUSER:-postgres}}"',
          'PGDATABASE="${POSTGRES_DB:-${PGDATABASE:-$PGUSER}}"',
          'export PGPASSWORD="${POSTGRES_PASSWORD:-${PGPASSWORD:-}}"',
          'psql -U "$PGUSER" -d "$PGDATABASE" -v ON_ERROR_STOP=1 -c "DROP SCHEMA IF EXISTS public CASCADE; CREATE SCHEMA public;"',
          restoreCommand
        ].join("; ")
      ],
      {
        env: process.env,
        shell: false,
        stdio: ["pipe", "pipe", "pipe"]
      }
    );
    const output: string[] = [];
    let settled = false;

    const fail = (error: Error) => {
      if (settled) return;
      settled = true;
      child.kill("SIGTERM");
      reject(error);
    };

    child.stdout.on("data", (chunk: Buffer) => output.push(chunk.toString()));
    child.stderr.on("data", (chunk: Buffer) => output.push(chunk.toString()));
    child.on("error", fail);
    child.on("close", (exitCode) => {
      if (settled) return;
      settled = true;
      if (exitCode !== 0) {
        reject(new Error(output.join("").trim() || `Postgres restore failed with exit code ${exitCode ?? 1}.`));
        return;
      }
      resolve();
    });

    pipeline(sqlStream, child.stdin).catch(fail);
  });
}

async function inspectContainerEnv(containerId: string) {
  const result = await runCommand("docker", ["inspect", "--format", "{{json .Config.Env}}", containerId]);
  if (result.exitCode !== 0) {
    return new Map<string, string>();
  }
  try {
    const values = JSON.parse(result.output.trim()) as string[];
    return new Map(
      values.flatMap((value) => {
        const separator = value.indexOf("=");
        return separator > 0 ? [[value.slice(0, separator), value.slice(separator + 1)] as const] : [];
      })
    );
  } catch {
    return new Map<string, string>();
  }
}

export type PostgresBackupTarget = {
  nodeId: string;
  nodeName: string;
  containerId: string;
  containerName: string;
  image: string;
  status: string;
  state: string;
  composeProject: string | null;
  composeService: string | null;
  projectId: string | null;
  projectName: string | null;
  databaseName: string;
  databaseUser: string;
};

type ReportedPostgresTarget = Omit<PostgresBackupTarget, "nodeId" | "nodeName">;

function reportedPostgresTargets(node: { id: string; name: string; labels: Record<string, string> }) {
  const raw = node.labels?.["backup.postgresTargets"];
  if (!raw) return [];
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed)) return [];
    return parsed.flatMap((value): PostgresBackupTarget[] => {
      if (!value || typeof value !== "object") return [];
      const target = value as Partial<ReportedPostgresTarget>;
      if (
        typeof target.containerId !== "string" ||
        typeof target.containerName !== "string" ||
        typeof target.image !== "string" ||
        typeof target.status !== "string" ||
        typeof target.state !== "string" ||
        typeof target.databaseName !== "string" ||
        typeof target.databaseUser !== "string"
      ) return [];
      return [{
        nodeId: node.id,
        nodeName: node.name,
        containerId: target.containerId,
        containerName: target.containerName,
        image: target.image,
        status: target.status,
        state: target.state,
        composeProject: typeof target.composeProject === "string" ? target.composeProject : null,
        composeService: typeof target.composeService === "string" ? target.composeService : null,
        projectId: typeof target.projectId === "string" ? target.projectId : null,
        projectName: typeof target.projectName === "string" ? target.projectName : null,
        databaseName: target.databaseName,
        databaseUser: target.databaseUser
      }];
    });
  } catch {
    return [];
  }
}

async function enrichBackups(rows: BackupRow[]) {
  if (!rows.length) return [];
  const backupIds = rows.map((backup) => backup.id);
  const replicas = await db.select().from(backupReplicas).where(inArray(backupReplicas.backupId, backupIds));
  const nodeIds = [...new Set([
    ...rows.flatMap((backup) => backup.sourceNodeId ? [backup.sourceNodeId] : []),
    ...replicas.map((replica) => replica.destinationNodeId)
  ])];
  const nodes = nodeIds.length
    ? await db.select({ id: deploymentNodes.id, name: deploymentNodes.name }).from(deploymentNodes)
      .where(inArray(deploymentNodes.id, nodeIds))
    : [];
  const nodeNames = new Map(nodes.map((node) => [node.id, node.name]));
  return rows.map((backup) => ({
    ...backup,
    sourceNodeName: backup.sourceNodeId ? nodeNames.get(backup.sourceNodeId) ?? null : null,
    replicas: replicas.filter((replica) => replica.backupId === backup.id).map((replica) => ({
      ...replica,
      destinationNodeName: nodeNames.get(replica.destinationNodeId) ?? null
    }))
  }));
}

export async function listBackups(limit = 50) {
  const rows = await db.select().from(backups).orderBy(desc(backups.createdAt)).limit(limit);
  return enrichBackups(rows);
}

export async function listBackupsForProjects(projectIds: string[], limit = 50) {
  if (projectIds.length === 0) return [];
  const rows = await db.select().from(backups).where(inArray(backups.projectId, projectIds))
    .orderBy(desc(backups.createdAt)).limit(limit);
  return enrichBackups(rows);
}

export async function getBackup(id: string) {
  const [backup] = await db.select().from(backups).where(eq(backups.id, id)).limit(1);
  if (!backup) return undefined;
  const [enriched] = await enrichBackups([backup]);
  return enriched;
}

async function discoverLocalPostgresBackupTargets(projectRows: Awaited<ReturnType<typeof listProjects>> = []): Promise<PostgresBackupTarget[]> {
  const containers = await listContainers();
  const projectsByFolder = new Map(projectRows.map((project) => [project.folderName, project]));
  const candidates = containers.filter((container) => container.isPostgresCandidate);

  return Promise.all(
    candidates.map(async (container) => {
      const env = await inspectContainerEnv(container.id);
      const databaseUser = env.get("POSTGRES_USER") || env.get("PGUSER") || "postgres";
      const databaseName = env.get("POSTGRES_DB") || env.get("PGDATABASE") || databaseUser;
      const project = container.composeProject ? projectsByFolder.get(container.composeProject) : undefined;
      return {
        nodeId: config.localNodeId,
        nodeName: "Master",
        containerId: container.id,
        containerName: container.name,
        image: container.image,
        status: container.status,
        state: container.state,
        composeProject: container.composeProject ?? null,
        composeService: container.composeService ?? null,
        projectId: project?.id ?? null,
        projectName: project?.name ?? null,
        databaseName,
        databaseUser
      };
    })
  );
}

export async function listLocalPostgresBackupTargets(): Promise<PostgresBackupTarget[]> {
  return discoverLocalPostgresBackupTargets(await listProjects());
}

export async function listRuntimePostgresBackupTargets(): Promise<PostgresBackupTarget[]> {
  return discoverLocalPostgresBackupTargets();
}

export async function listPostgresBackupTargets(): Promise<PostgresBackupTarget[]> {
  const [localTargets, nodes, projectRows] = await Promise.all([
    listLocalPostgresBackupTargets(),
    db.select({ id: deploymentNodes.id, name: deploymentNodes.name, labels: deploymentNodes.labels }).from(deploymentNodes),
    listProjects()
  ]);
  const localNode = nodes.find((node) => node.id === config.localNodeId);
  const namedLocalTargets = localNode
    ? localTargets.map((target) => ({ ...target, nodeName: localNode.name }))
    : localTargets;
  const remoteTargets = nodes.filter((node) => node.id !== config.localNodeId).flatMap(reportedPostgresTargets)
    .map((target) => {
      const project = projectRows.find((candidate) =>
        candidate.targetNodeId === target.nodeId && candidate.folderName === target.composeProject
      );
      return project ? { ...target, projectId: project.id, projectName: project.name } : target;
    });
  return [
    ...namedLocalTargets,
    ...remoteTargets
  ];
}

export async function postgresTargetInventoryLabel() {
  const targets = await listRuntimePostgresBackupTargets();
  return JSON.stringify(targets.map((target) => ({
    containerId: target.containerId,
    containerName: target.containerName,
    image: target.image,
    status: target.status,
    state: target.state,
    composeProject: target.composeProject,
    composeService: target.composeService,
    projectId: target.projectId,
    projectName: target.projectName,
    databaseName: target.databaseName,
    databaseUser: target.databaseUser
  })));
}

export async function createPostgresBackupArtifact(input: {
  containerId?: string;
  backupId?: string;
  filename?: string;
}) {
  const target = input.containerId
    ? (await listRuntimePostgresBackupTargets()).find((item) => item.containerId === input.containerId)
    : undefined;
  if (input.containerId && !target) throw new Error("Container is not a recognized local Postgres backup target.");
  if (target && target.state !== "running") throw new Error(`${target.containerName} must be running before it can be dumped.`);

  const owner = target?.projectName ?? target?.composeProject ?? target?.containerName ?? "yanto";
  const generatedFilename = `${safeFilenamePart(owner) || "postgres"}-postgres-${backupTimestamp()}.sql.gz`;
  const filename = path.basename(input.filename || generatedFilename);
  const filePath = path.join(config.backupsDir, filename);
  if (path.dirname(filePath) !== config.backupsDir) throw new Error("Backup filename is invalid.");
  if (target) await runContainerPgDumpGzip(target.containerId, filePath);
  else await runPgDumpGzip(filePath);
  const [stat, checksum] = await Promise.all([fs.stat(filePath), calculateBackupChecksum(filePath)]);
  return { target, filename, filePath, fileSizeBytes: stat.size, checksum };
}

export type WorkerBackupRuntimeJob = {
  backup: { id: string; filename: string };
  targetContainerId: string | null;
  destinations: Array<
    | { nodeId: string; local: true }
    | (BackupDestinationConfig & { local?: false })
  >;
};

export async function executeWorkerBackupJob(job: WorkerBackupRuntimeJob, signal?: AbortSignal) {
  try {
    signal?.throwIfAborted();
    const artifact = await createPostgresBackupArtifact({
      backupId: job.backup.id,
      containerId: job.targetContainerId ?? undefined,
      filename: job.backup.filename
    });
    const replicas = await Promise.all(job.destinations.map(async (destination) => {
      if ("local" in destination && destination.local) {
        return {
          destinationNodeId: destination.nodeId,
          status: "success" as const,
          filePath: artifact.filePath,
          checksum: artifact.checksum,
          attempts: 1
        };
      }
      try {
        const result = await rsyncBackupFileWithRetry({
          sourcePath: artifact.filePath,
          backupId: job.backup.id,
          destination,
          expectedChecksum: artifact.checksum,
          maxAttempts: 3,
          signal
        });
        return {
          destinationNodeId: destination.nodeId,
          status: "success" as const,
          filePath: result.filePath,
          checksum: result.checksum,
          attempts: result.attempts
        };
      } catch (error) {
        return {
          destinationNodeId: destination.nodeId,
          status: "failed" as const,
          error: error instanceof Error ? error.message : String(error),
          attempts: 3
        };
      }
    }));
    return {
      status: "success" as const,
      filePath: artifact.filePath,
      fileSizeBytes: artifact.fileSizeBytes,
      checksum: artifact.checksum,
      replicas
    };
  } catch (error) {
    return {
      status: "failed" as const,
      error: error instanceof Error ? error.message : String(error),
      replicas: []
    };
  }
}

export async function createPostgresBackup(containerId?: string, options: { sourceNodeId?: string; policyId?: string } = {}) {
  const id = createId("bak");
  const sourceNodeId = options.sourceNodeId ?? config.localNodeId;
  if (sourceNodeId !== config.localNodeId) {
    throw new Error("Remote Postgres backups must be queued for the source worker.");
  }
  const target = containerId ? (await listLocalPostgresBackupTargets()).find((item) => item.containerId === containerId) : undefined;

  if (containerId && !target) {
    throw new Error("Container is not a recognized Postgres backup target.");
  }
  if (target && target.state !== "running") {
    throw new Error(`${target.containerName} must be running before it can be dumped.`);
  }

  const owner = target?.projectName ?? target?.composeProject ?? target?.containerName ?? "yanto";
  const filename = `${safeFilenamePart(owner) || "postgres"}-postgres-${backupTimestamp()}.sql.gz`;
  const filePath = path.join(config.backupsDir, filename);
  const createdAt = new Date();
  const note = target
    ? `Container ${target.containerName}; database ${target.databaseName}; user ${target.databaseUser}`
    : "Yanto application database";

  const [backup] = await db
    .insert(backups)
    .values({
      id,
      projectId: target?.projectId ?? null,
      sourceNodeId,
      policyId: options.policyId ?? null,
      kind: target ? "postgres-container" : "postgres",
      status: "running",
      filename,
      filePath,
      fileSizeBytes: null,
      error: null,
      note,
      createdAt
    })
    .returning();

  try {
    const artifact = await createPostgresBackupArtifact({ containerId, backupId: id, filename });
    const [finished] = await db
      .update(backups)
      .set({ status: "success", fileSizeBytes: artifact.fileSizeBytes, checksum: artifact.checksum, finishedAt: new Date(), error: null })
      .where(eq(backups.id, id))
      .returning();
    return finished;
  } catch (error) {
    await fs.rm(filePath, { force: true });
    const message = error instanceof Error ? error.message : String(error);
    const [failed] = await db
      .update(backups)
      .set({ status: "failed", finishedAt: new Date(), error: message })
      .where(eq(backups.id, id))
      .returning();
    return failed ?? { ...backup, status: "failed", error: message, finishedAt: new Date() };
  }
}

export async function restorePostgresBackupTarget(containerId: string, dumpPath: string, originalFilename: string) {
  const target = (await listLocalPostgresBackupTargets()).find((item) => item.containerId === containerId);
  if (!target) {
    throw new Error("Container is not a recognized Postgres backup target.");
  }
  if (target.state !== "running") {
    throw new Error(`${target.containerName} must be running before it can be restored.`);
  }
  await runContainerRestore(target.containerId, dumpPath, originalFilename);
  return target;
}

export async function uploadBackupToR2(id: string) {
  const backup = await getBackup(id);
  if (!backup || backup.status !== "success") {
    throw new Error("Backup not found.");
  }
  const result = await uploadFileToR2({
    filePath: backup.filePath,
    filename: backup.filename || `${backup.id}.sql.gz`,
    contentType: backup.filename.endsWith(".gz") ? "application/gzip" : "application/sql"
  });
  return { backup, result };
}

export async function deleteBackup(id: string) {
  const backup = await getBackup(id);
  if (!backup) {
    return undefined;
  }
  if (backup.status === "running") {
    throw new Error("Cannot remove a backup while it is still running.");
  }
  if (!backup.sourceNodeId || backup.sourceNodeId === config.localNodeId) {
    await fs.rm(backup.filePath, { force: true });
  }
  await db.delete(backups).where(eq(backups.id, id));
  return backup;
}

export async function markBackupDownloaded(id: string) {
  await db
    .update(backups)
    .set({
      downloadedAt: new Date(),
      downloadCount: sql`${backups.downloadCount} + 1`
    })
    .where(eq(backups.id, id));
}
