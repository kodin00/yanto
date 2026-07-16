import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import fs from "node:fs/promises";
import path from "node:path";
import { config } from "../config.js";
import { runCommand, type CommandResult } from "./commands.js";

export type BackupDestinationConfig = {
  nodeId: string;
  host: string;
  port: number;
  username: string;
  directory: string;
  privateKeyPath: string;
};

export type BackupReplicaResult = {
  filePath: string;
  checksum: string;
  attempts: number;
};

type NodeWithBackupLabels = {
  id: string;
  labels: Record<string, string>;
};

type CommandRunner = (
  command: string,
  args: string[],
  options?: Parameters<typeof runCommand>[2]
) => Promise<CommandResult>;

function positivePort(value: string | undefined) {
  if (!value) return undefined;
  const port = Number(value);
  return Number.isInteger(port) && port >= 1 && port <= 65_535 ? port : undefined;
}

function assertSafeHost(value: string) {
  if (!value || /[\s/?#@]/.test(value)) throw new Error("Backup SSH host is invalid.");
}

function assertSafeUsername(value: string) {
  if (!/^[a-z_][a-z0-9_-]*$/i.test(value)) throw new Error("Backup SSH username is invalid.");
}

function assertSafeRemoteDirectory(value: string) {
  if (!value.startsWith("/") || /[\0\r\n]/.test(value)) {
    throw new Error("Backup destination directory must be an absolute path without line breaks.");
  }
}

function shellQuote(value: string) {
  return `'${value.replace(/'/g, `'"'"'`)}'`;
}

function sshArgs(destination: BackupDestinationConfig) {
  return [
    "-p",
    String(destination.port),
    "-i",
    destination.privateKeyPath,
    "-o",
    `StrictHostKeyChecking=${config.sshStrictHostKeyChecking}`,
    "-o",
    "BatchMode=yes",
    "-o",
    "ConnectTimeout=15"
  ];
}

function rsyncRemoteShell(destination: BackupDestinationConfig) {
  return ["ssh", ...sshArgs(destination)].map(shellQuote).join(" ");
}

function remoteAddress(destination: BackupDestinationConfig) {
  const host = destination.host.includes(":") && !destination.host.startsWith("[")
    ? `[${destination.host}]`
    : destination.host;
  return `${destination.username}@${host}`;
}

export function backupDestinationFromNode(node: NodeWithBackupLabels): BackupDestinationConfig {
  const labels = node.labels ?? {};
  const host = labels["backup.sshHost"]?.trim() || labels["frp.sshHost"]?.trim();
  const port = positivePort(labels["backup.sshPort"]?.trim() || labels["frp.sshPort"]?.trim()) ?? 22;
  const username = labels["backup.sshUser"]?.trim();
  const directory = labels["backup.directory"]?.trim();
  const privateKeyPath = labels["backup.privateKeyPath"]?.trim() || config.managedSshPrivateKeyPath;

  if (!host) throw new Error(`Node ${node.id} does not have backup.sshHost or frp.sshHost configured.`);
  if (!username) throw new Error(`Node ${node.id} does not have backup.sshUser configured.`);
  if (!directory) throw new Error(`Node ${node.id} does not have backup.directory configured.`);
  assertSafeHost(host);
  assertSafeUsername(username);
  assertSafeRemoteDirectory(directory);

  return { nodeId: node.id, host, port, username, directory, privateKeyPath };
}

export async function calculateBackupChecksum(filePath: string) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(filePath)) hash.update(chunk as Buffer);
  return hash.digest("hex");
}

function checked(result: CommandResult, operation: string) {
  if (result.exitCode === 0) return result.output;
  const detail = result.output.trim().slice(-1_000);
  throw new Error(`${operation} failed${detail ? `: ${detail}` : ` with exit code ${result.exitCode}`}`);
}

export async function rsyncBackupFile(input: {
  sourcePath: string;
  backupId: string;
  destination: BackupDestinationConfig;
  expectedChecksum?: string;
  signal?: AbortSignal;
  run?: CommandRunner;
}): Promise<BackupReplicaResult> {
  const run = input.run ?? runCommand;
  const filename = path.basename(input.sourcePath);
  if (!filename || filename === "." || filename === "..") throw new Error("Backup filename is invalid.");
  const destinationDirectory = path.posix.join(input.destination.directory, input.backupId);
  const destinationPath = path.posix.join(destinationDirectory, filename);
  const remote = remoteAddress(input.destination);
  const commonOptions = { signal: input.signal, timeoutMs: 15 * 60_000, killProcessGroup: true };

  const mkdir = await run(
    "ssh",
    [...sshArgs(input.destination), "--", remote, `umask 077 && mkdir -p -- ${shellQuote(destinationDirectory)}`],
    commonOptions
  );
  checked(mkdir, `Preparing backup directory on ${input.destination.nodeId}`);

  const transfer = await run(
    "rsync",
    [
      "--archive",
      "--partial",
      "--delay-updates",
      "--checksum",
      "--timeout=60",
      "--chmod=F600,D700",
      "-e",
      rsyncRemoteShell(input.destination),
      "--",
      input.sourcePath,
      `${remote}:${shellQuote(destinationPath)}`
    ],
    commonOptions
  );
  checked(transfer, `Copying backup to ${input.destination.nodeId}`);

  const verify = await run(
    "ssh",
    [...sshArgs(input.destination), "--", remote, `sha256sum -- ${shellQuote(destinationPath)}`],
    commonOptions
  );
  const output = checked(verify, `Verifying backup on ${input.destination.nodeId}`);
  const remoteChecksum = output.trim().split(/\s+/)[0]?.toLowerCase();
  const expectedChecksum = (input.expectedChecksum ?? await calculateBackupChecksum(input.sourcePath)).toLowerCase();
  if (!/^[a-f0-9]{64}$/.test(remoteChecksum ?? "") || remoteChecksum !== expectedChecksum) {
    throw new Error(`Backup checksum verification failed on ${input.destination.nodeId}.`);
  }

  return { filePath: destinationPath, checksum: remoteChecksum, attempts: 1 };
}

export async function rsyncBackupFileWithRetry(input: Parameters<typeof rsyncBackupFile>[0] & {
  maxAttempts?: number;
  onAttempt?: (attempt: number) => void | Promise<void>;
}): Promise<BackupReplicaResult> {
  const maxAttempts = Math.max(1, Math.min(10, input.maxAttempts ?? 3));
  let lastError: unknown;
  for (let attempt = 1; attempt <= maxAttempts; attempt++) {
    input.signal?.throwIfAborted();
    await input.onAttempt?.(attempt);
    try {
      const result = await rsyncBackupFile(input);
      return { ...result, attempts: attempt };
    } catch (error) {
      lastError = error;
      if (attempt === maxAttempts || input.signal?.aborted) break;
      await new Promise<void>((resolve, reject) => {
        const finish = () => {
          input.signal?.removeEventListener("abort", abort);
          resolve();
        };
        const timer = setTimeout(finish, Math.min(250 * 2 ** (attempt - 1), 2_000));
        const abort = () => {
          clearTimeout(timer);
          input.signal?.removeEventListener("abort", abort);
          reject(input.signal?.reason ?? new Error("Backup replication was canceled."));
        };
        input.signal?.addEventListener("abort", abort, { once: true });
        timer.unref();
      });
    }
  }
  throw lastError instanceof Error ? lastError : new Error(String(lastError ?? "Backup replication failed."));
}

export async function fetchBackupReplica(input: {
  remotePath: string;
  targetPath: string;
  destination: BackupDestinationConfig;
  expectedChecksum: string;
  signal?: AbortSignal;
  run?: CommandRunner;
}) {
  if (!path.posix.isAbsolute(input.remotePath) || /[\0\r\n]/.test(input.remotePath)) {
    throw new Error("Remote backup path is invalid.");
  }
  const run = input.run ?? runCommand;
  const remote = remoteAddress(input.destination);
  const partialPath = `${input.targetPath}.partial-${process.pid}-${Date.now()}`;
  await fs.mkdir(path.dirname(input.targetPath), { recursive: true, mode: 0o700 });
  try {
    const transfer = await run(
      "rsync",
      [
        "--archive",
        "--partial",
        "--checksum",
        "--timeout=60",
        "--chmod=F600",
        "-e",
        rsyncRemoteShell(input.destination),
        "--",
        `${remote}:${shellQuote(input.remotePath)}`,
        partialPath
      ],
      { signal: input.signal, timeoutMs: 15 * 60_000, killProcessGroup: true }
    );
    checked(transfer, `Fetching backup from ${input.destination.nodeId}`);
    const checksum = await calculateBackupChecksum(partialPath);
    if (checksum.toLowerCase() !== input.expectedChecksum.toLowerCase()) {
      throw new Error(`Fetched backup checksum verification failed for ${input.destination.nodeId}.`);
    }
    await fs.chmod(partialPath, 0o600);
    await fs.rename(partialPath, input.targetPath);
    return { filePath: input.targetPath, checksum };
  } catch (error) {
    await fs.rm(partialPath, { force: true });
    throw error;
  }
}

export async function removeRemoteBackupFile(input: {
  remotePath: string;
  destination: BackupDestinationConfig;
  signal?: AbortSignal;
  run?: CommandRunner;
}) {
  if (!path.posix.isAbsolute(input.remotePath) || /[\0\r\n]/.test(input.remotePath)) {
    throw new Error("Remote backup path is invalid.");
  }
  if (!input.remotePath.startsWith(`${input.destination.directory.replace(/\/$/, "")}/`)) {
    throw new Error("Refusing to remove a file outside the configured backup directory.");
  }
  const run = input.run ?? runCommand;
  const result = await run(
    "ssh",
    [
      ...sshArgs(input.destination),
      "--",
      remoteAddress(input.destination),
      `rm -f -- ${shellQuote(input.remotePath)} && (rmdir -- ${shellQuote(path.posix.dirname(input.remotePath))} 2>/dev/null || true)`
    ],
    { signal: input.signal, timeoutMs: 60_000, killProcessGroup: true }
  );
  checked(result, `Removing expired backup from ${input.destination.nodeId}`);
}

export type RetentionBackup = { id: string; createdAt: Date };

export function backupsToPrune(
  values: RetentionBackup[],
  retention: { hourly?: number; daily?: number } = {}
) {
  const hourlyLimit = Math.max(0, retention.hourly ?? 24);
  const dailyLimit = Math.max(0, retention.daily ?? 30);
  const sorted = [...values].sort((left, right) => right.createdAt.getTime() - left.createdAt.getTime());
  const kept = new Set<string>();
  const hours = new Set<string>();
  const days = new Set<string>();

  for (const backup of sorted) {
    const iso = backup.createdAt.toISOString();
    const hour = iso.slice(0, 13);
    const day = iso.slice(0, 10);
    if (hours.size < hourlyLimit && !hours.has(hour)) {
      hours.add(hour);
      kept.add(backup.id);
    }
    if (days.size < dailyLimit && !days.has(day)) {
      days.add(day);
      kept.add(backup.id);
    }
  }

  // The newest successful generation is always kept, even with zero retention.
  if (sorted[0]) kept.add(sorted[0].id);
  return sorted.filter((backup) => !kept.has(backup.id));
}
