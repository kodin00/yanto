import { and, asc, desc, eq, inArray, isNull, lte, or, sql } from "drizzle-orm";
import fs from "node:fs/promises";
import path from "node:path";
import { config } from "../config.js";
import { db } from "../db/index.js";
import { logger } from "../logger.js";
import {
  backupPolicies,
  backupPolicyDestinations,
  backupReplicas,
  backups,
  deploymentNodes,
  type BackupPolicyRow,
  type BackupRow
} from "../db/schema.js";
import { backupDestinationFromNode, backupsToPrune, fetchBackupReplica, removeRemoteBackupFile, rsyncBackupFileWithRetry } from "./backup-replication.js";
import { createPostgresBackup, deleteBackup, getBackup, listPostgresBackupTargets } from "./backups.js";
import { createId } from "./tokens.js";

export type BackupPolicyInput = {
  name: string;
  sourceNodeId: string;
  targetContainerId?: string | null;
  enabled?: boolean;
  hourlyAtMinute?: number;
  hourlyRetention?: number;
  dailyRetention?: number;
  destinationNodeIds?: string[];
};

export type WorkerBackupCompletion = {
  status: "success" | "failed";
  filePath?: string;
  fileSizeBytes?: number;
  checksum?: string;
  error?: string;
  replicas?: Array<{
    destinationNodeId: string;
    status: "success" | "failed";
    filePath?: string;
    checksum?: string;
    error?: string;
    attempts?: number;
  }>;
};

function safeFilenamePart(input: string) {
  return input.trim().toLowerCase().replace(/[^a-z0-9._-]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 72);
}

export function nextHourlyBackupRun(now: Date, minute: number) {
  const normalizedMinute = Math.max(0, Math.min(59, Math.trunc(minute)));
  const next = new Date(now);
  next.setUTCMinutes(normalizedMinute, 0, 0);
  if (next.getTime() <= now.getTime()) next.setUTCHours(next.getUTCHours() + 1);
  return next;
}

async function assertPolicyReferences(input: BackupPolicyInput) {
  const nodeIds = [...new Set([input.sourceNodeId, ...(input.destinationNodeIds ?? [])])];
  const nodes = nodeIds.length
    ? await db.select({ id: deploymentNodes.id }).from(deploymentNodes).where(inArray(deploymentNodes.id, nodeIds))
    : [];
  if (nodes.length !== nodeIds.length) throw new Error("One or more backup nodes do not exist.");
  if (!input.targetContainerId && input.sourceNodeId !== config.localNodeId) {
    throw new Error("The Yanto application database can only be backed up on the master node.");
  }
  if (input.targetContainerId) {
    const target = (await listPostgresBackupTargets()).find(
      (candidate) => candidate.nodeId === input.sourceNodeId && candidate.containerId === input.targetContainerId
    );
    if (!target) throw new Error("Postgres target does not exist on the selected source node.");
  }
}

async function replaceDestinations(
  tx: Parameters<Parameters<typeof db.transaction>[0]>[0],
  policyId: string,
  destinationNodeIds: string[]
) {
  await tx.delete(backupPolicyDestinations).where(eq(backupPolicyDestinations.policyId, policyId));
  const uniqueIds = [...new Set(destinationNodeIds)];
  if (uniqueIds.length) {
    await tx.insert(backupPolicyDestinations).values(
      uniqueIds.map((destinationNodeId) => ({ policyId, destinationNodeId }))
    );
  }
}

export async function listBackupPolicies() {
  const [policies, destinations] = await Promise.all([
    db.select().from(backupPolicies).orderBy(asc(backupPolicies.name)),
    db.select().from(backupPolicyDestinations).orderBy(asc(backupPolicyDestinations.createdAt))
  ]);
  return policies.map((policy) => ({
    ...policy,
    destinationNodeIds: destinations.filter((row) => row.policyId === policy.id).map((row) => row.destinationNodeId)
  }));
}

export async function getBackupPolicy(id: string) {
  const [policy] = await db.select().from(backupPolicies).where(eq(backupPolicies.id, id)).limit(1);
  if (!policy) return undefined;
  const destinations = await db.select().from(backupPolicyDestinations)
    .where(eq(backupPolicyDestinations.policyId, id));
  return { ...policy, destinationNodeIds: destinations.map((row) => row.destinationNodeId) };
}

export async function createBackupPolicy(input: BackupPolicyInput) {
  await assertPolicyReferences(input);
  const now = new Date();
  const hourlyAtMinute = input.hourlyAtMinute ?? 0;
  const policyId = createId("bpol");
  await db.transaction(async (tx) => {
    await tx.insert(backupPolicies).values({
      id: policyId,
      name: input.name.trim(),
      sourceNodeId: input.sourceNodeId,
      targetContainerId: input.targetContainerId || null,
      enabled: input.enabled ?? true,
      hourlyAtMinute,
      hourlyRetention: input.hourlyRetention ?? 24,
      dailyRetention: input.dailyRetention ?? 30,
      nextRunAt: input.enabled === false ? null : nextHourlyBackupRun(now, hourlyAtMinute),
      createdAt: now,
      updatedAt: now
    });
    await replaceDestinations(tx, policyId, input.destinationNodeIds ?? []);
  });
  return getBackupPolicy(policyId);
}

export async function updateBackupPolicy(id: string, input: Partial<BackupPolicyInput>) {
  const existing = await getBackupPolicy(id);
  if (!existing) return undefined;
  const merged: BackupPolicyInput = {
    name: input.name ?? existing.name,
    sourceNodeId: input.sourceNodeId ?? existing.sourceNodeId,
    targetContainerId: input.targetContainerId !== undefined ? input.targetContainerId : existing.targetContainerId,
    enabled: input.enabled ?? existing.enabled,
    hourlyAtMinute: input.hourlyAtMinute ?? existing.hourlyAtMinute,
    hourlyRetention: input.hourlyRetention ?? existing.hourlyRetention,
    dailyRetention: input.dailyRetention ?? existing.dailyRetention,
    destinationNodeIds: input.destinationNodeIds ?? existing.destinationNodeIds
  };
  await assertPolicyReferences(merged);
  const now = new Date();
  await db.transaction(async (tx) => {
    await tx.update(backupPolicies).set({
      name: merged.name.trim(),
      sourceNodeId: merged.sourceNodeId,
      targetContainerId: merged.targetContainerId || null,
      enabled: merged.enabled,
      hourlyAtMinute: merged.hourlyAtMinute,
      hourlyRetention: merged.hourlyRetention,
      dailyRetention: merged.dailyRetention,
      nextRunAt: merged.enabled ? nextHourlyBackupRun(now, merged.hourlyAtMinute ?? 0) : null,
      updatedAt: now
    }).where(eq(backupPolicies.id, id));
    if (input.destinationNodeIds !== undefined) await replaceDestinations(tx, id, merged.destinationNodeIds ?? []);
  });
  return getBackupPolicy(id);
}

export async function deleteBackupPolicy(id: string) {
  const [deleted] = await db.delete(backupPolicies).where(eq(backupPolicies.id, id)).returning();
  return deleted;
}

async function destinationsForPolicy(policyId: string) {
  return db.select({ node: deploymentNodes })
    .from(backupPolicyDestinations)
    .innerJoin(deploymentNodes, eq(deploymentNodes.id, backupPolicyDestinations.destinationNodeId))
    .where(eq(backupPolicyDestinations.policyId, policyId));
}

export async function ensureBackupReplicas(backupId: string, destinationNodeIds: string[]) {
  if (!destinationNodeIds.length) return [];
  await db.insert(backupReplicas).values([...new Set(destinationNodeIds)].map((destinationNodeId) => ({
    id: createId("brep"),
    backupId,
    destinationNodeId,
    status: "pending"
  }))).onConflictDoNothing();
  return listBackupReplicas(backupId);
}

export async function listBackupReplicas(backupId: string) {
  return db.select().from(backupReplicas).where(eq(backupReplicas.backupId, backupId))
    .orderBy(asc(backupReplicas.createdAt));
}

export async function materializeBackupFile(backupId: string) {
  const backup = await getBackup(backupId);
  if (!backup || backup.status !== "success") throw new Error("Backup not found.");
  try {
    await fs.access(backup.filePath);
    return backup.filePath;
  } catch {
    // Fall through to independently verified replicas.
  }
  const replicas = await db.select({ replica: backupReplicas, node: deploymentNodes })
    .from(backupReplicas)
    .innerJoin(deploymentNodes, eq(deploymentNodes.id, backupReplicas.destinationNodeId))
    .where(and(eq(backupReplicas.backupId, backupId), eq(backupReplicas.status, "success")))
    .orderBy(asc(backupReplicas.createdAt));
  let lastError: unknown;
  for (const { replica, node } of replicas) {
    if (!replica.filePath || !replica.checksum) continue;
    if (node.id === config.localNodeId) {
      try {
        await fs.access(replica.filePath);
        return replica.filePath;
      } catch (error) {
        lastError = error;
      }
    }
    try {
      const result = await fetchBackupReplica({
        remotePath: replica.filePath,
        targetPath: backup.filePath,
        destination: backupDestinationFromNode(node),
        expectedChecksum: backup.checksum ?? replica.checksum
      });
      return result.filePath;
    } catch (error) {
      lastError = error;
    }
  }
  throw lastError instanceof Error ? lastError : new Error("No verified backup copy is currently available.");
}

export async function replicateBackupReplica(replicaId: string) {
  const [row] = await db.select({ replica: backupReplicas, backup: backups, node: deploymentNodes })
    .from(backupReplicas)
    .innerJoin(backups, eq(backups.id, backupReplicas.backupId))
    .innerJoin(deploymentNodes, eq(deploymentNodes.id, backupReplicas.destinationNodeId))
    .where(eq(backupReplicas.id, replicaId)).limit(1);
  if (!row) throw new Error("Backup replica not found.");
  if (row.backup.status !== "success") throw new Error("Only successful backups can be replicated.");
  if (row.backup.sourceNodeId !== config.localNodeId) {
    throw new Error("A remote-source backup must be replicated by its source worker.");
  }

  const now = new Date();
  if (row.replica.destinationNodeId === row.backup.sourceNodeId) {
    const [updated] = await db.update(backupReplicas).set({
      status: "success",
      filePath: row.backup.filePath,
      checksum: row.backup.checksum,
      error: null,
      attempts: row.replica.attempts + 1,
      updatedAt: now,
      finishedAt: now
    }).where(eq(backupReplicas.id, replicaId)).returning();
    return updated;
  }

  const destination = backupDestinationFromNode(row.node);
  await db.update(backupReplicas).set({ status: "copying", error: null, updatedAt: now, finishedAt: null })
    .where(eq(backupReplicas.id, replicaId));
  try {
    const result = await rsyncBackupFileWithRetry({
      sourcePath: row.backup.filePath,
      backupId: row.backup.id,
      destination,
      expectedChecksum: row.backup.checksum ?? undefined,
      maxAttempts: 3,
      onAttempt: async (attempt) => {
        await db.update(backupReplicas).set({ attempts: row.replica.attempts + attempt, updatedAt: new Date() })
          .where(eq(backupReplicas.id, replicaId));
      }
    });
    const [updated] = await db.update(backupReplicas).set({
      status: "success",
      filePath: result.filePath,
      checksum: result.checksum,
      error: null,
      updatedAt: new Date(),
      finishedAt: new Date()
    }).where(eq(backupReplicas.id, replicaId)).returning();
    return updated;
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    await db.update(backupReplicas).set({ status: "failed", error: message, updatedAt: new Date(), finishedAt: new Date() })
      .where(eq(backupReplicas.id, replicaId));
    throw error;
  }
}

export async function replicateBackupToPolicyDestinations(backup: BackupRow) {
  if (!backup.policyId || backup.status !== "success") return [];
  const destinations = await destinationsForPolicy(backup.policyId);
  const replicas = await ensureBackupReplicas(backup.id, destinations.map(({ node }) => node.id));
  return Promise.allSettled(replicas.map((replica) => replicateBackupReplica(replica.id)));
}

async function queueWorkerBackup(policy: BackupPolicyRow) {
  const id = createId("bak");
  const filename = `${safeFilenamePart(policy.name) || "postgres"}-postgres-${new Date().toISOString().replace(/[:.]/g, "-")}.sql.gz`;
  const [backup] = await db.insert(backups).values({
    id,
    projectId: null,
    sourceNodeId: policy.sourceNodeId,
    policyId: policy.id,
    kind: "postgres-container",
    status: "pending",
    filename,
    filePath: path.join(config.backupsDir, filename),
    note: `Scheduled backup policy ${policy.name}`
  }).returning();
  const destinations = await destinationsForPolicy(policy.id);
  await ensureBackupReplicas(id, destinations.map(({ node }) => node.id));
  return backup;
}

export async function runBackupPolicy(id: string) {
  const policy = await getBackupPolicy(id);
  if (!policy) throw new Error("Backup policy not found.");
  const now = new Date();
  await db.update(backupPolicies).set({
    lastRunAt: now,
    nextRunAt: policy.enabled ? nextHourlyBackupRun(now, policy.hourlyAtMinute) : null,
    updatedAt: now
  }).where(eq(backupPolicies.id, id));

  if (policy.sourceNodeId !== config.localNodeId) return queueWorkerBackup(policy);
  const backup = await createPostgresBackup(policy.targetContainerId ?? undefined, {
    sourceNodeId: policy.sourceNodeId,
    policyId: policy.id
  });
  if (backup.status === "success") {
    await replicateBackupToPolicyDestinations(backup);
    await applyBackupRetention(policy.id);
  }
  return getBackup(backup.id);
}

export async function runDueBackupPolicies(now = new Date()) {
  const due = await db.select({ id: backupPolicies.id }).from(backupPolicies)
    .where(and(
      eq(backupPolicies.enabled, true),
      or(isNull(backupPolicies.nextRunAt), lte(backupPolicies.nextRunAt, now))
    ))
    .orderBy(asc(backupPolicies.nextRunAt));
  const results = [];
  for (const policy of due) {
    try {
      results.push({ policyId: policy.id, status: "fulfilled" as const, backup: await runBackupPolicy(policy.id) });
    } catch (error) {
      results.push({ policyId: policy.id, status: "rejected" as const, error: error instanceof Error ? error.message : String(error) });
    }
  }
  return results;
}

export async function nextWorkerBackupJob(nodeId: string) {
  const [candidate] = await db.select({ backup: backups, policy: backupPolicies })
    .from(backups)
    .innerJoin(backupPolicies, eq(backupPolicies.id, backups.policyId))
    .where(and(eq(backups.sourceNodeId, nodeId), eq(backups.status, "pending")))
    .orderBy(asc(backups.createdAt)).limit(1);
  if (!candidate) return null;
  const destinations = await destinationsForPolicy(candidate.policy.id);
  const destinationConfigs = destinations.map(({ node }) => node.id === nodeId
    ? { nodeId: node.id, local: true as const }
    : backupDestinationFromNode(node));
  const [claimed] = await db.update(backups).set({ status: "running", error: null })
    .where(and(eq(backups.id, candidate.backup.id), eq(backups.status, "pending"))).returning();
  if (!claimed) return null;
  return {
    backup: claimed,
    targetContainerId: candidate.policy.targetContainerId,
    destinations: destinationConfigs
  };
}

export async function completeWorkerBackup(backupId: string, sourceNodeId: string, input: WorkerBackupCompletion) {
  const backup = await getBackup(backupId);
  if (!backup || backup.sourceNodeId !== sourceNodeId || backup.status !== "running") {
    throw new Error("Running backup not found for worker.");
  }
  const now = new Date();
  await db.transaction(async (tx) => {
    await tx.update(backups).set({
      status: input.status,
      filePath: input.filePath ?? backup.filePath,
      fileSizeBytes: input.fileSizeBytes ?? null,
      checksum: input.checksum ?? null,
      error: input.status === "failed" ? input.error || "Worker backup failed." : null,
      finishedAt: now
    }).where(eq(backups.id, backupId));
    for (const result of input.replicas ?? []) {
      await tx.update(backupReplicas).set({
        status: result.status,
        filePath: result.filePath ?? null,
        checksum: result.checksum ?? null,
        error: result.status === "failed" ? result.error || "Worker replication failed." : null,
        attempts: Math.max(1, result.attempts ?? 1),
        updatedAt: now,
        finishedAt: now
      }).where(and(eq(backupReplicas.backupId, backupId), eq(backupReplicas.destinationNodeId, result.destinationNodeId)));
    }
  });
  if (input.status === "success" && backup.policyId) await applyBackupRetention(backup.policyId);
  return getBackup(backupId);
}

async function removeRemoteBackupCopies(backup: BackupRow) {
  const copies = await db.select({ replica: backupReplicas, node: deploymentNodes })
    .from(backupReplicas)
    .innerJoin(deploymentNodes, eq(deploymentNodes.id, backupReplicas.destinationNodeId))
    .where(and(eq(backupReplicas.backupId, backup.id), eq(backupReplicas.status, "success")));
  const remoteFiles = new Map<string, { filePath: string; node: typeof deploymentNodes.$inferSelect }>();
  for (const { replica, node } of copies) {
    if (replica.filePath && node.id !== config.localNodeId) {
      remoteFiles.set(`${node.id}:${replica.filePath}`, { filePath: replica.filePath, node });
    }
  }
  if (backup.sourceNodeId && backup.sourceNodeId !== config.localNodeId) {
    const [sourceNode] = await db.select().from(deploymentNodes).where(eq(deploymentNodes.id, backup.sourceNodeId)).limit(1);
    if (!sourceNode) throw new Error("Backup source node no longer exists.");
    remoteFiles.set(`${sourceNode.id}:${backup.filePath}`, { filePath: backup.filePath, node: sourceNode });
  }
  for (const remote of remoteFiles.values()) {
    await removeRemoteBackupFile({
      remotePath: remote.filePath,
      destination: backupDestinationFromNode(remote.node)
    });
  }
}

export async function deleteBackupEverywhere(id: string) {
  const backup = await getBackup(id);
  if (!backup) return undefined;
  if (backup.status === "running") throw new Error("Cannot remove a backup while it is still running.");
  await removeRemoteBackupCopies(backup);
  return deleteBackup(id);
}

export async function applyBackupRetention(policyId: string) {
  const policy = await getBackupPolicy(policyId);
  if (!policy) return [];
  const successful = await db.select({ id: backups.id, createdAt: backups.createdAt }).from(backups)
    .where(and(eq(backups.policyId, policyId), eq(backups.status, "success")))
    .orderBy(desc(backups.createdAt));
  const expired = backupsToPrune(successful, { hourly: policy.hourlyRetention, daily: policy.dailyRetention });
  const deleted: string[] = [];
  for (const expiredBackup of expired) {
    const backup = await getBackup(expiredBackup.id);
    if (!backup) continue;
    try {
      await removeRemoteBackupCopies(backup);
      await deleteBackup(backup.id);
      deleted.push(backup.id);
    } catch (error) {
      // Keep the database record so a later retention pass can retry cleanup.
      logger.warn("backup retention cleanup failed", {
        backupId: backup.id,
        error: error instanceof Error ? error.message : String(error)
      });
    }
  }
  return deleted;
}

export async function recoverInterruptedBackups() {
  const [workerRunning, copying] = await Promise.all([
    db.update(backups).set({ status: "pending", error: "Worker restarted before backup completion." })
      .where(and(eq(backups.status, "running"), sql`${backups.sourceNodeId} <> ${config.localNodeId}`)).returning({ id: backups.id }),
    db.update(backupReplicas).set({ status: "failed", error: "Backup replication was interrupted.", updatedAt: new Date(), finishedAt: new Date() })
      .where(eq(backupReplicas.status, "copying")).returning({ id: backupReplicas.id })
  ]);
  return { requeuedBackups: workerRunning.length, failedReplicas: copying.length };
}
