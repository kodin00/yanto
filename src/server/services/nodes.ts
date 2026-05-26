import { and, asc, desc, eq, sql } from "drizzle-orm";
import { db } from "../db/index.js";
import { deploymentNodes, deployments, projects, type DeploymentNodeRow } from "../db/schema.js";
import { config } from "../config.js";
import { createId, createWorkerToken, hashToken } from "./tokens.js";

export type NodeRole = "master" | "worker";
export type NodeStatus = "online" | "offline";

export type NodeInput = {
  name?: string;
  labels?: Record<string, unknown>;
  dockerVersion?: string | null;
};

function normalizeLabels(labels: Record<string, unknown> | undefined) {
  const normalized: Record<string, string> = {};
  for (const [key, value] of Object.entries(labels ?? {})) {
    const cleanKey = key.trim();
    if (!cleanKey) continue;
    normalized[cleanKey] = String(value);
  }
  return normalized;
}

export async function ensureLocalMasterNode() {
  const now = new Date();
  await db
    .insert(deploymentNodes)
    .values({
      id: config.localNodeId,
      name: "Master",
      role: "master",
      status: "online",
      lastSeenAt: now,
      labels: {},
      createdAt: now,
      updatedAt: now
    })
    .onConflictDoUpdate({
      target: deploymentNodes.id,
      set: { role: "master", status: "online", lastSeenAt: now, updatedAt: now }
    });
}

export async function listNodes() {
  return db
    .select({
      id: deploymentNodes.id,
      name: deploymentNodes.name,
      role: deploymentNodes.role,
      status: sql<string>`CASE WHEN ${deploymentNodes.role} = 'master' THEN 'online' WHEN ${deploymentNodes.lastSeenAt} < now() - interval '60 seconds' THEN 'offline' ELSE ${deploymentNodes.status} END`,
      lastSeenAt: deploymentNodes.lastSeenAt,
      dockerVersion: deploymentNodes.dockerVersion,
      labels: deploymentNodes.labels,
      createdAt: deploymentNodes.createdAt,
      updatedAt: deploymentNodes.updatedAt,
      projectCount: sql<number>`count(distinct ${projects.id})`,
      runningDeploymentCount: sql<number>`count(distinct ${deployments.id}) filter (where ${deployments.status} = 'running')`
    })
    .from(deploymentNodes)
    .leftJoin(projects, eq(projects.targetNodeId, deploymentNodes.id))
    .leftJoin(deployments, eq(deployments.nodeId, deploymentNodes.id))
    .groupBy(deploymentNodes.id)
    .orderBy(asc(deploymentNodes.role), asc(deploymentNodes.name));
}

export async function getNode(id: string) {
  const [node] = await db.select().from(deploymentNodes).where(eq(deploymentNodes.id, id)).limit(1);
  return node;
}

export async function assertDeployableNode(id: string) {
  const node = await getNode(id);
  if (!node) {
    throw new Error("Deployment node not found.");
  }
  return node;
}

export async function registerWorker(input: NodeInput) {
  const token = createWorkerToken();
  const now = new Date();
  const [node] = await db
    .insert(deploymentNodes)
    .values({
      id: createId("node"),
      name: input.name?.trim() || "Worker node",
      role: "worker",
      status: "online",
      lastSeenAt: now,
      dockerVersion: input.dockerVersion?.trim() || null,
      labels: normalizeLabels(input.labels),
      tokenHash: hashToken(token, config.workerTokenSecret),
      createdAt: now,
      updatedAt: now
    })
    .returning();
  return { node, token };
}

export async function nodeForWorkerToken(token: string) {
  const tokenHash = hashToken(token, config.workerTokenSecret);
  const [node] = await db.select().from(deploymentNodes).where(and(eq(deploymentNodes.role, "worker"), eq(deploymentNodes.tokenHash, tokenHash))).limit(1);
  return node;
}

export async function markNodeSeen(node: DeploymentNodeRow, input: NodeInput = {}) {
  const now = new Date();
  const [updated] = await db
    .update(deploymentNodes)
    .set({
      name: input.name?.trim() || node.name,
      status: "online",
      lastSeenAt: now,
      dockerVersion: input.dockerVersion?.trim() || node.dockerVersion,
      labels: input.labels ? normalizeLabels(input.labels) : node.labels,
      updatedAt: now
    })
    .where(eq(deploymentNodes.id, node.id))
    .returning();
  return updated;
}

export async function nextWorkerDeployment(nodeId: string) {
  const [row] = await db
    .select({ deployment: deployments, project: projects })
    .from(deployments)
    .innerJoin(projects, eq(projects.id, deployments.projectId))
    .where(and(eq(deployments.nodeId, nodeId), eq(deployments.status, "running")))
    .orderBy(asc(deployments.startedAt))
    .limit(1);
  return row;
}

export async function latestDeploymentForNode(nodeId: string) {
  const [deployment] = await db.select().from(deployments).where(eq(deployments.nodeId, nodeId)).orderBy(desc(deployments.startedAt)).limit(1);
  return deployment;
}
