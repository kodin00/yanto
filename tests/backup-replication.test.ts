import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  backupDestinationFromNode,
  backupsToPrune,
  calculateBackupChecksum,
  fetchBackupReplica,
  rsyncBackupFile,
  rsyncBackupFileWithRetry,
  type BackupDestinationConfig
} from "../src/server/services/backup-replication.js";

const tempPaths: string[] = [];

async function tempFile(contents: string) {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), "yanto-backup-test-"));
  tempPaths.push(directory);
  const filePath = path.join(directory, "database.sql.gz");
  await fs.writeFile(filePath, contents);
  return filePath;
}

afterEach(async () => {
  await Promise.all(tempPaths.splice(0).map((filePath) => fs.rm(filePath, { recursive: true, force: true })));
});

const destination: BackupDestinationConfig = {
  nodeId: "node_home",
  host: "home.example.test",
  port: 22,
  username: "backups",
  directory: "/srv/yanto/backups",
  privateKeyPath: "/run/secrets/backup-key"
};

describe("backup replication", () => {
  it("uses an FRP SSH endpoint when a direct backup endpoint is absent", () => {
    expect(backupDestinationFromNode({
      id: "node_home",
      labels: {
        "frp.sshHost": "vps.example.test",
        "frp.sshPort": "22022",
        "backup.sshUser": "receiver",
        "backup.directory": "/data/backups",
        "backup.privateKeyPath": "/keys/home"
      }
    })).toEqual({
      nodeId: "node_home",
      host: "vps.example.test",
      port: 22022,
      username: "receiver",
      directory: "/data/backups",
      privateKeyPath: "/keys/home"
    });
  });

  it("copies with partial-transfer protection and verifies the remote checksum", async () => {
    const sourcePath = await tempFile("postgres backup");
    const checksum = await calculateBackupChecksum(sourcePath);
    const run = vi.fn()
      .mockResolvedValueOnce({ exitCode: 0, output: "" })
      .mockResolvedValueOnce({ exitCode: 0, output: "" })
      .mockResolvedValueOnce({ exitCode: 0, output: `${checksum}  database.sql.gz\n` });

    await expect(rsyncBackupFile({
      sourcePath,
      backupId: "bak_1",
      destination,
      expectedChecksum: checksum,
      run
    })).resolves.toEqual({
      filePath: "/srv/yanto/backups/bak_1/database.sql.gz",
      checksum,
      attempts: 1
    });
    expect(run).toHaveBeenCalledTimes(3);
    expect(run.mock.calls[1][0]).toBe("rsync");
    expect(run.mock.calls[1][1]).toEqual(expect.arrayContaining([
      "--partial",
      "--delay-updates",
      "--checksum",
      "--timeout=60"
    ]));
  });

  it("retries an interrupted transfer without creating another dump", async () => {
    const sourcePath = await tempFile("retry backup");
    const checksum = await calculateBackupChecksum(sourcePath);
    let transferAttempt = 0;
    const run = vi.fn(async (command: string) => {
      if (command === "rsync" && ++transferAttempt < 3) return { exitCode: 12, output: "connection dropped" };
      if (command === "ssh" && transferAttempt >= 3) return { exitCode: 0, output: `${checksum}  dump\n` };
      return { exitCode: 0, output: "" };
    });
    const attempts: number[] = [];

    await expect(rsyncBackupFileWithRetry({
      sourcePath,
      backupId: "bak_retry",
      destination,
      expectedChecksum: checksum,
      maxAttempts: 3,
      run,
      onAttempt: (attempt) => attempts.push(attempt)
    })).resolves.toMatchObject({ checksum, attempts: 3 });
    expect(attempts).toEqual([1, 2, 3]);
    expect(transferAttempt).toBe(3);
  });

  it("rejects a destination checksum mismatch", async () => {
    const sourcePath = await tempFile("corrupt me");
    const run = vi.fn()
      .mockResolvedValueOnce({ exitCode: 0, output: "" })
      .mockResolvedValueOnce({ exitCode: 0, output: "" })
      .mockResolvedValueOnce({ exitCode: 0, output: `${"0".repeat(64)}  database.sql.gz\n` });
    await expect(rsyncBackupFile({ sourcePath, backupId: "bak_bad", destination, run }))
      .rejects.toThrow("checksum verification failed");
  });

  it("fetches a verified replica into the requested local path", async () => {
    const expectedPath = await tempFile("restored replica");
    const checksum = await calculateBackupChecksum(expectedPath);
    const targetDirectory = await fs.mkdtemp(path.join(os.tmpdir(), "yanto-backup-fetch-"));
    tempPaths.push(targetDirectory);
    const targetPath = path.join(targetDirectory, "restored.sql.gz");
    const run = vi.fn(async (_command: string, args: string[]) => {
      await fs.copyFile(expectedPath, args.at(-1)!);
      return { exitCode: 0, output: "" };
    });
    await expect(fetchBackupReplica({
      remotePath: "/srv/yanto/backups/bak_1/database.sql.gz",
      targetPath,
      destination,
      expectedChecksum: checksum,
      run
    })).resolves.toEqual({ filePath: targetPath, checksum });
    await expect(fs.readFile(targetPath, "utf8")).resolves.toBe("restored replica");
  });
});

describe("backup retention", () => {
  it("keeps hourly and daily UTC generations plus the newest backup", () => {
    const backups = [
      ["newest", "2026-07-16T10:30:00.000Z"],
      ["same-hour", "2026-07-16T10:10:00.000Z"],
      ["previous-hour", "2026-07-16T09:10:00.000Z"],
      ["previous-day", "2026-07-15T08:00:00.000Z"],
      ["old", "2026-07-14T08:00:00.000Z"]
    ].map(([id, createdAt]) => ({ id, createdAt: new Date(createdAt) }));
    expect(backupsToPrune(backups, { hourly: 2, daily: 2 }).map(({ id }) => id)).toEqual(["same-hour", "old"]);
    expect(backupsToPrune(backups, { hourly: 0, daily: 0 }).map(({ id }) => id))
      .toEqual(["same-hour", "previous-hour", "previous-day", "old"]);
  });
});
