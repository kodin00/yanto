import { desc, eq, sql } from "drizzle-orm";
import { spawn } from "node:child_process";
import { createWriteStream } from "node:fs";
import fs from "node:fs/promises";
import path from "node:path";
import { createGzip } from "node:zlib";
import { config } from "../config.js";
import { db } from "../db/index.js";
import { backups } from "../db/schema.js";
import { listContainers } from "./docker.js";
import { listProjects } from "./projects.js";
import { runCommand } from "./commands.js";
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

export async function listBackups(limit = 50) {
  return db.select().from(backups).orderBy(desc(backups.createdAt)).limit(limit);
}

export async function getBackup(id: string) {
  const [backup] = await db.select().from(backups).where(eq(backups.id, id)).limit(1);
  return backup;
}

export async function listPostgresBackupTargets() {
  const [containers, projectRows] = await Promise.all([listContainers(), listProjects()]);
  const projectsByFolder = new Map(projectRows.map((project) => [project.folderName, project]));
  const candidates = containers.filter((container) => container.isPostgresCandidate);

  return Promise.all(
    candidates.map(async (container) => {
      const env = await inspectContainerEnv(container.id);
      const databaseUser = env.get("POSTGRES_USER") || env.get("PGUSER") || "postgres";
      const databaseName = env.get("POSTGRES_DB") || env.get("PGDATABASE") || databaseUser;
      const project = container.composeProject ? projectsByFolder.get(container.composeProject) : undefined;
      return {
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

export async function createPostgresBackup(containerId?: string) {
  const id = createId("bak");
  const target = containerId ? (await listPostgresBackupTargets()).find((item) => item.containerId === containerId) : undefined;

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
    if (target) {
      await runContainerPgDumpGzip(target.containerId, filePath);
    } else {
      await runPgDumpGzip(filePath);
    }
    const stat = await fs.stat(filePath);
    const [finished] = await db
      .update(backups)
      .set({ status: "success", fileSizeBytes: stat.size, finishedAt: new Date(), error: null })
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

export async function deleteBackup(id: string) {
  const backup = await getBackup(id);
  if (!backup) {
    return undefined;
  }
  if (backup.status === "running") {
    throw new Error("Cannot remove a backup while it is still running.");
  }
  await fs.rm(backup.filePath, { force: true });
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
