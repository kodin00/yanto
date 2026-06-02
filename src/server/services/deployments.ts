import { and, desc, eq, sql } from "drizzle-orm";
import { db } from "../db/index.js";
import { deploymentNodes, deployments, projects, type DeploymentRow, type ProjectRow } from "../db/schema.js";
import { config } from "../config.js";
import { logger } from "../logger.js";
import { runProjectDeployment, type DeploymentMetadata, type PendingDeploymentEnv } from "./deployment-runner.js";
import { createId } from "./tokens.js";
import { deploymentEvents } from "./deployment-events.js";

const activeDeployments = new Map<string, DeploymentRow>();

function now() {
  return new Date();
}

export async function appendDeploymentLog(deploymentId: string, chunk: string) {
  const nextLogs = sql`${deployments.logs} || ${chunk}`;
  const truncationNotice = "[... older deployment logs truncated ...]\n";
  const retainedChars = Math.max(0, config.deploymentLogMaxChars - truncationNotice.length);
  const [updated] = await db
    .update(deployments)
    .set({
      logs:
        config.deploymentLogMaxChars > 0
          ? sql`CASE WHEN length(${nextLogs}) > ${config.deploymentLogMaxChars} THEN ${truncationNotice} || right(${nextLogs}, ${retainedChars}) ELSE ${nextLogs} END`
          : nextLogs
    })
    .where(eq(deployments.id, deploymentId))
    .returning({ logs: deployments.logs, status: deployments.status });

  if (updated) {
    deploymentEvents.emitLogUpdate({
      deploymentId,
      logs: updated.logs,
      status: updated.status,
      done: updated.status !== "running"
    });
  }
}

export async function updateDeploymentMetadata(deploymentId: string, metadata: DeploymentMetadata) {
  await db
    .update(deployments)
    .set({
      commitSha: metadata.commitSha ?? null,
      commitMessage: metadata.commitMessage ?? null,
      targetRef: metadata.targetRef ?? null
    })
    .where(eq(deployments.id, deploymentId));
}

export async function finishDeployment(deploymentId: string, status: "success" | "failed", exitCode: number) {
  await db
    .update(deployments)
    .set({ status, exitCode, finishedAt: now() })
    .where(eq(deployments.id, deploymentId));

  const updated = await findDeployment(deploymentId);
  if (updated) {
    deploymentEvents.emitLogUpdate({
      deploymentId,
      logs: updated.logs,
      status: updated.status,
      done: true
    });
  }
}

async function runLocalDeployment(project: ProjectRow, deployment: DeploymentRow, pendingEnv?: PendingDeploymentEnv) {
  try {
    await runProjectDeployment(project, deployment, {
      appendLog: (chunk) => appendDeploymentLog(deployment.id, chunk),
      updateMetadata: (metadata) => updateDeploymentMetadata(deployment.id, metadata)
    }, pendingEnv);
    await finishDeployment(deployment.id, "success", 0);
    logger.info("deployment succeeded", { projectId: project.id, deploymentId: deployment.id, nodeId: deployment.nodeId });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    await appendDeploymentLog(deployment.id, `Deployment failed: ${message}\n`);
    await finishDeployment(deployment.id, "failed", 1);
    logger.error("deployment failed", { projectId: project.id, deploymentId: deployment.id, nodeId: deployment.nodeId, error: message });
  } finally {
    activeDeployments.delete(project.id);
  }
}

export type StartDeploymentOptions = {
  targetRef?: string;
  rollbackFromDeploymentId?: string;
  pendingEnv?: PendingDeploymentEnv;
};

export async function startDeployment(projectId: string, trigger: "manual" | "webhook" | "github" | "rollback", options: StartDeploymentOptions = {}) {
  const active = activeDeployments.get(projectId);
  if (active) {
    return { deployment: active, reused: true };
  }

  const result = await db.transaction(async (tx) => {
    await tx.execute(sql`SELECT pg_advisory_xact_lock(hashtext(${`deployment:${projectId}`}))`);

    const [runningDeployment] = await tx
      .select()
      .from(deployments)
      .where(and(eq(deployments.projectId, projectId), eq(deployments.status, "running")))
      .limit(1);
    if (runningDeployment) {
      return { deployment: runningDeployment, project: null, node: null, reused: true };
    }

    const [project] = await tx.select().from(projects).where(eq(projects.id, projectId)).limit(1);
    if (!project) {
      throw new Error("Project not found.");
    }

    const [node] = await tx.select().from(deploymentNodes).where(eq(deploymentNodes.id, project.targetNodeId)).limit(1);
    if (!node) {
      throw new Error("Project target node not found.");
    }
    if (options.pendingEnv && !(node.role === "master" && node.id === config.localNodeId)) {
      throw new Error("Writing environment variables during deployment is only supported on the local master node.");
    }

    const [deployment] = await tx
      .insert(deployments)
      .values({
        id: createId("dep"),
        projectId,
        nodeId: node.id,
        status: "running",
        trigger,
        targetRef: options.targetRef?.trim() || null,
        rollbackFromDeploymentId: options.rollbackFromDeploymentId?.trim() || null,
        logs: "",
        startedAt: now()
      })
      .returning();

    return { deployment, project, node, reused: false };
  });

  if (!result.reused && result.project && result.node) {
    if (result.node.role === "master" && result.node.id === config.localNodeId) {
      activeDeployments.set(projectId, result.deployment);
      void runLocalDeployment(result.project, result.deployment, options.pendingEnv);
    } else {
      await appendDeploymentLog(result.deployment.id, `Queued deployment for node ${result.node.name} (${result.node.id}).\n`);
    }
  }
  return { deployment: result.deployment, reused: result.reused };
}

export async function latestDeployments(limit = 500) {
  return db
    .select({
      id: deployments.id,
      projectId: deployments.projectId,
      projectName: projects.name,
      nodeId: deployments.nodeId,
      nodeName: deploymentNodes.name,
      status: deployments.status,
      trigger: deployments.trigger,
      targetRef: deployments.targetRef,
      commitSha: deployments.commitSha,
      commitMessage: deployments.commitMessage,
      rollbackFromDeploymentId: deployments.rollbackFromDeploymentId,
      exitCode: deployments.exitCode,
      startedAt: deployments.startedAt,
      finishedAt: deployments.finishedAt
    })
    .from(deployments)
    .leftJoin(projects, eq(projects.id, deployments.projectId))
    .leftJoin(deploymentNodes, eq(deploymentNodes.id, deployments.nodeId))
    .orderBy(desc(deployments.startedAt))
    .limit(limit);
}

export async function findDeployment(id: string) {
  const [deployment] = await db.select().from(deployments).where(eq(deployments.id, id)).limit(1);
  return deployment;
}

export async function findDeploymentForNode(deploymentId: string, nodeId: string) {
  const [deployment] = await db.select().from(deployments).where(and(eq(deployments.id, deploymentId), eq(deployments.nodeId, nodeId))).limit(1);
  return deployment;
}

export async function rollbackTargetForProject(projectId: string, deploymentId?: string, targetRef?: string) {
  if (targetRef?.trim()) {
    return { targetRef: targetRef.trim(), rollbackFromDeploymentId: deploymentId ?? null };
  }

  if (deploymentId) {
    const [deployment] = await db.select().from(deployments).where(and(eq(deployments.id, deploymentId), eq(deployments.projectId, projectId))).limit(1);
    const ref = deployment?.commitSha ?? deployment?.targetRef;
    if (!ref) {
      throw new Error("Selected deployment does not have a recorded Git commit.");
    }
    return { targetRef: ref, rollbackFromDeploymentId: deployment.id };
  }

  const rows = await db
    .select()
    .from(deployments)
    .where(and(eq(deployments.projectId, projectId), eq(deployments.status, "success")))
    .orderBy(desc(deployments.startedAt))
    .limit(20);
  const distinct = rows.filter((deployment, index) => {
    const ref = deployment.commitSha ?? deployment.targetRef;
    return ref && rows.findIndex((candidate) => (candidate.commitSha ?? candidate.targetRef) === ref) === index;
  });
  const target = distinct[1];
  const ref = target?.commitSha ?? target?.targetRef;
  if (!target || !ref) {
    throw new Error("No previous successful Git deployment is available to roll back to.");
  }
  return { targetRef: ref, rollbackFromDeploymentId: target.id };
}

export async function activeDeploymentFor(projectId: string) {
  const [deployment] = await db
    .select()
    .from(deployments)
    .where(and(eq(deployments.projectId, projectId), eq(deployments.status, "running")))
    .limit(1);
  return deployment;
}

export async function recoverInterruptedDeployments() {
  const interruptedAt = now();
  const notice = `\nDeployment marked failed because Yanto restarted at ${interruptedAt.toISOString()} before it finished.\n`;
  const rows = await db
    .update(deployments)
    .set({
      status: "failed",
      exitCode: 1,
      finishedAt: interruptedAt,
      logs: sql`${deployments.logs} || ${notice}`
    })
    .where(eq(deployments.status, "running"))
    .returning();

  if (rows.length) {
    logger.warn("recovered interrupted deployments", { count: rows.length });
  }
  return rows.length;
}
