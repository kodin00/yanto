import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { config, warnOnUnsafeDefaults } from "./config.js";
import { logger } from "./logger.js";
import type { DeploymentRow, ProjectRow } from "./db/schema.js";
import { runProjectDeployment, type DeploymentMetadata } from "./services/deployment-runner.js";
import { runCommand } from "./services/commands.js";

type WorkerJob = {
  deployment: DeploymentRow;
  project: ProjectRow;
} | null;

function sleep(ms: number) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function masterUrl(pathname: string) {
  if (!config.masterUrl) {
    throw new Error("YANTO_MASTER_URL is required in worker mode.");
  }
  return `${config.masterUrl.replace(/\/$/, "")}${pathname}`;
}

async function request<T>(pathname: string, options: RequestInit = {}, token?: string): Promise<T> {
  const response = await fetch(masterUrl(pathname), {
    ...options,
    headers: {
      "Content-Type": "application/json",
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
      ...options.headers
    }
  });
  if (!response.ok) {
    const body = (await response.json().catch(() => ({ message: response.statusText }))) as { message?: string };
    throw new Error(body.message ?? `Worker request failed with ${response.status}.`);
  }
  if (response.status === 204) {
    return undefined as T;
  }
  return response.json() as Promise<T>;
}

async function dockerVersion() {
  const result = await runCommand("docker", ["version", "--format", "{{.Server.Version}}"], { timeoutMs: 10000 });
  return result.exitCode === 0 ? result.output.trim() : null;
}

async function readStoredToken() {
  if (config.workerToken) return config.workerToken;
  try {
    return (await fs.readFile(config.workerTokenPath, "utf8")).trim();
  } catch {
    return "";
  }
}

async function writeStoredToken(token: string) {
  await fs.mkdir(path.dirname(config.workerTokenPath), { recursive: true, mode: 0o700 });
  await fs.writeFile(config.workerTokenPath, `${token}\n`, { mode: 0o600 });
}

async function workerIdentity() {
  return {
    name: config.workerName || os.hostname(),
    dockerVersion: await dockerVersion(),
    labels: {
      hostname: os.hostname(),
      platform: os.platform(),
      arch: os.arch()
    }
  };
}

async function tokenForWorker() {
  const stored = await readStoredToken();
  if (stored) return stored;
  if (!config.workerJoinToken) {
    throw new Error("YANTO_WORKER_TOKEN or WORKER_JOIN_TOKEN is required in worker mode.");
  }
  const registration = await request<{ token: string }>("/api/workers/register", {
    method: "POST",
    body: JSON.stringify({ joinToken: config.workerJoinToken, ...(await workerIdentity()) })
  });
  await writeStoredToken(registration.token);
  return registration.token;
}

async function heartbeat(token: string) {
  await request<{ ok: true }>("/api/workers/heartbeat", {
    method: "POST",
    body: JSON.stringify(await workerIdentity())
  }, token);
}

async function nextJob(token: string) {
  return request<WorkerJob>("/api/workers/jobs/next", { method: "GET" }, token);
}

async function appendLog(token: string, deploymentId: string, chunk: string) {
  await request<{ ok: true }>(`/api/workers/deployments/${deploymentId}/logs`, {
    method: "POST",
    body: JSON.stringify({ chunk })
  }, token);
}

async function updateMetadata(token: string, deploymentId: string, metadata: DeploymentMetadata) {
  await request<{ ok: true }>(`/api/workers/deployments/${deploymentId}`, {
    method: "PATCH",
    body: JSON.stringify(metadata)
  }, token);
}

async function finish(token: string, deploymentId: string, status: "success" | "failed", exitCode: number, metadata: DeploymentMetadata = {}) {
  await request<{ ok: true }>(`/api/workers/deployments/${deploymentId}`, {
    method: "PATCH",
    body: JSON.stringify({ status, exitCode, ...metadata })
  }, token);
}

async function runJob(token: string, job: NonNullable<WorkerJob>) {
  try {
    await appendLog(token, job.deployment.id, `Worker ${config.workerName || os.hostname()} accepted deployment.\n`);
    await runProjectDeployment(job.project, job.deployment, {
      appendLog: (chunk) => appendLog(token, job.deployment.id, chunk),
      updateMetadata: (metadata) => updateMetadata(token, job.deployment.id, metadata)
    });
    await finish(token, job.deployment.id, "success", 0);
    logger.info("worker deployment succeeded", { deploymentId: job.deployment.id, projectId: job.project.id });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    await appendLog(token, job.deployment.id, `Deployment failed: ${message}\n`).catch(() => undefined);
    await finish(token, job.deployment.id, "failed", 1).catch(() => undefined);
    logger.error("worker deployment failed", { deploymentId: job.deployment.id, projectId: job.project.id, error: message });
  }
}

async function main() {
  warnOnUnsafeDefaults();
  const token = await tokenForWorker();
  logger.info("worker started", { masterUrl: config.masterUrl, name: config.workerName || os.hostname() });
  let consecutiveErrors = 0;
  const maxBackoffMs = 60_000;
  while (true) {
    try {
      await heartbeat(token);
      const job = await nextJob(token);
      if (job) {
        await runJob(token, job);
      }
      consecutiveErrors = 0;
    } catch (error) {
      consecutiveErrors++;
      logger.error("worker loop failed", { error: error instanceof Error ? error.message : String(error), consecutiveErrors });
    }
    const backoff = consecutiveErrors > 0
      ? Math.min(config.workerPollIntervalMs * Math.pow(2, consecutiveErrors - 1), maxBackoffMs)
      : config.workerPollIntervalMs;
    await sleep(backoff);
  }
}

main().catch((error) => {
  logger.error("worker failed to start", { error: error instanceof Error ? error.message : String(error) });
  process.exit(1);
});
