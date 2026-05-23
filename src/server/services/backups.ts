import { desc, eq, sql } from "drizzle-orm";
import { spawn } from "node:child_process";
import { createWriteStream } from "node:fs";
import fs from "node:fs/promises";
import path from "node:path";
import { createGzip } from "node:zlib";
import { config } from "../config.js";
import { db } from "../db/index.js";
import { backups } from "../db/schema.js";
import { createId } from "./tokens.js";

function backupTimestamp() {
  return new Date().toISOString().replace(/[:.]/g, "-");
}

async function runPgDumpGzip(filePath: string) {
  await fs.mkdir(path.dirname(filePath), { recursive: true, mode: 0o700 });

  await new Promise<void>((resolve, reject) => {
    const pgDump = spawn("pg_dump", [config.databaseUrl], {
      env: process.env,
      shell: false,
      stdio: ["ignore", "pipe", "pipe"]
    });
    const gzip = createGzip({ level: 9 });
    const output = createWriteStream(filePath, { mode: 0o600 });
    const errors: string[] = [];

    pgDump.stderr.on("data", (chunk: Buffer) => errors.push(chunk.toString()));
    pgDump.on("error", reject);
    gzip.on("error", reject);
    output.on("error", reject);
    output.on("finish", resolve);

    pgDump.stdout.pipe(gzip).pipe(output);
    pgDump.on("close", (exitCode) => {
      if (exitCode !== 0) {
        reject(new Error(errors.join("").trim() || `pg_dump failed with exit code ${exitCode ?? 1}.`));
      }
    });
  });
}

export async function listBackups(limit = 50) {
  return db.select().from(backups).orderBy(desc(backups.createdAt)).limit(limit);
}

export async function getBackup(id: string) {
  const [backup] = await db.select().from(backups).where(eq(backups.id, id)).limit(1);
  return backup;
}

export async function createPostgresBackup() {
  const id = createId("bak");
  const filename = `yanto-postgres-${backupTimestamp()}.sql.gz`;
  const filePath = path.join(config.backupsDir, filename);
  const createdAt = new Date();

  const [backup] = await db
    .insert(backups)
    .values({
      id,
      projectId: null,
      kind: "postgres",
      status: "running",
      filename,
      filePath,
      fileSizeBytes: null,
      error: null,
      note: null,
      createdAt
    })
    .returning();

  try {
    await runPgDumpGzip(filePath);
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

export async function markBackupDownloaded(id: string) {
  await db
    .update(backups)
    .set({
      downloadedAt: new Date(),
      downloadCount: sql`${backups.downloadCount} + 1`
    })
    .where(eq(backups.id, id));
}
