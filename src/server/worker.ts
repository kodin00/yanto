import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { config, warnOnUnsafeDefaults } from "./config.js";
import { logger } from "./logger.js";
import type { DeploymentRow } from "./db/schema.js";
import { runProjectDeployment, type DeploymentMetadata, type DeploymentProject } from "./services/deployment-runner.js";
import { runCommand } from "./services/commands.js";
import { requestWorkerJson, waitForWorkerPoll } from "./services/worker-runtime.js";

type WorkerJob = {
  deployment: DeploymentRow;
  project: DeploymentProject;
} | null;

function masterUrl(pathname: string) {
  if (!config.masterUrl) {
    throw new Error("YANTO_MASTER_URL is required in worker mode.");
  }
  return `${config.masterUrl.replace(/\/$/, "")}${pathname}`;
}

async function request<T>(pathname: string, options: RequestInit = {}, token?: string): Promise<T> {
  return requestWorkerJson<T>(masterUrl(pathname), {
    ...options,
    headers: {
      "Content-Type": "application/json",
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
      ...options.headers
    }
  });
}

let dockerVersionCache: { value: string | null; expiresAt: number } | undefined;
let dockerVersionRequest: Promise<string | null> | undefined;

async function dockerVersion(signal: AbortSignal) {
  if (dockerVersionCache && dockerVersionCache.expiresAt > Date.now()) return dockerVersionCache.value;
  if (dockerVersionRequest) return dockerVersionRequest;

  dockerVersionRequest = (async () => {
    const result = await runCommand("docker", ["version", "--format", "{{.Server.Version}}"], {
      timeoutMs: 10_000,
      signal,
      killProcessGroup: true
    });
    signal.throwIfAborted();
    const value = result.exitCode === 0 ? result.output.trim() : null;
    dockerVersionCache = {
      value,
      expiresAt: Date.now() + (value ? 5 * 60_000 : 30_000)
    };
    return value;
  })().finally(() => {
    dockerVersionRequest = undefined;
  });
  return dockerVersionRequest;
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

async function workerIdentity(signal: AbortSignal) {
  return {
    name: config.workerName || os.hostname(),
    dockerVersion: await dockerVersion(signal),
    labels: {
      hostname: os.hostname(),
      platform: os.platform(),
      arch: os.arch()
    }
  };
}

async function tokenForWorker(signal: AbortSignal) {
  const stored = await readStoredToken();
  if (stored) return stored;
  if (!config.workerJoinToken) {
    throw new Error("YANTO_WORKER_TOKEN or WORKER_JOIN_TOKEN is required in worker mode.");
  }
  const registration = await request<{ token: string }>("/api/workers/register", {
    method: "POST",
    body: JSON.stringify({ joinToken: config.workerJoinToken, ...(await workerIdentity(signal)) }),
    signal
  });
  await writeStoredToken(registration.token);
  return registration.token;
}

async function heartbeat(token: string, signal: AbortSignal) {
  await request<{ ok: true }>("/api/workers/heartbeat", {
    method: "POST",
    body: JSON.stringify(await workerIdentity(signal)),
    signal
  }, token);
}

async function nextJob(token: string, signal: AbortSignal) {
  return request<WorkerJob>("/api/workers/jobs/next", { method: "GET", signal }, token);
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

async function finish(
  token: string,
  deploymentId: string,
  status: "success" | "failed",
  exitCode: number,
  metadata: DeploymentMetadata = {},
  signal?: AbortSignal
) {
  await request<{ ok: true }>(`/api/workers/deployments/${deploymentId}`, {
    method: "PATCH",
    body: JSON.stringify({ status, exitCode, ...metadata }),
    signal
  }, token);
}

async function runJob(token: string, job: NonNullable<WorkerJob>, signal: AbortSignal) {
  try {
    await appendLog(token, job.deployment.id, `Worker ${config.workerName || os.hostname()} accepted deployment.\n`);
    await runProjectDeployment(job.project, job.deployment, {
      appendLog: (chunk) => appendLog(token, job.deployment.id, chunk),
      updateMetadata: (metadata) => updateMetadata(token, job.deployment.id, metadata),
      signal
    });
    await finish(token, job.deployment.id, "success", 0);
    logger.info("worker deployment succeeded", { deploymentId: job.deployment.id, projectId: job.project.id });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (signal.aborted) {
      // Prioritize releasing the master's running-deployment lock during a
      // shutdown, but do not let an unreachable master delay process exit.
      await finish(token, job.deployment.id, "failed", 1, {}, AbortSignal.timeout(5_000)).catch(() => undefined);
      logger.warn("worker deployment canceled during shutdown", {
        deploymentId: job.deployment.id,
        projectId: job.project.id,
        error: message
      });
      return;
    }
    await appendLog(token, job.deployment.id, `Deployment failed: ${message}\n`).catch(() => undefined);
    await finish(token, job.deployment.id, "failed", 1).catch(() => undefined);
    logger.error("worker deployment failed", { deploymentId: job.deployment.id, projectId: job.project.id, error: message });
  }
}

async function controlLoop(token: string, signal: AbortSignal) {
  let consecutiveErrors = 0;
  const maxBackoffMs = 60_000;
  while (!signal.aborted) {
    try {
      await heartbeat(token, signal);
      consecutiveErrors = 0;
    } catch (error) {
      if (signal.aborted) break;
      consecutiveErrors++;
      logger.error("worker control loop failed", { error: error instanceof Error ? error.message : String(error), consecutiveErrors });
    }
    const backoff = consecutiveErrors > 0
      ? Math.min(config.workerPollIntervalMs * Math.pow(2, consecutiveErrors - 1), maxBackoffMs)
      : config.workerPollIntervalMs;
    if (!(await waitForWorkerPoll(backoff, signal))) break;
  }
}

async function deploymentLoop(token: string, signal: AbortSignal) {
  let consecutiveErrors = 0;
  while (!signal.aborted) {
    try {
      const job = await nextJob(token, signal);
      if (job) await runJob(token, job, signal);
      consecutiveErrors = 0;
    } catch (error) {
      if (signal.aborted) break;
      consecutiveErrors++;
      logger.error("worker deployment loop failed", { error: error instanceof Error ? error.message : String(error), consecutiveErrors });
    }
    const delay = consecutiveErrors ? Math.min(config.workerPollIntervalMs * 2 ** (consecutiveErrors - 1), 60_000) : config.workerPollIntervalMs;
    if (!(await waitForWorkerPoll(delay, signal))) break;
  }
}

async function main() {
  warnOnUnsafeDefaults();
  const stopController = new AbortController();
  const shutdown = (signal: string) => {
    if (stopController.signal.aborted) return;
    logger.info("worker shutdown starting", { signal });
    stopController.abort(new Error(`Worker received ${signal}.`));
  };
  const onSigint = () => shutdown("SIGINT");
  const onSigterm = () => shutdown("SIGTERM");
  process.once("SIGINT", onSigint);
  process.once("SIGTERM", onSigterm);
  try {
    const token = await tokenForWorker(stopController.signal);
    if (stopController.signal.aborted) return;
    logger.info("worker started", { masterUrl: config.masterUrl, name: config.workerName || os.hostname() });
    await Promise.all([
      controlLoop(token, stopController.signal),
      deploymentLoop(token, stopController.signal)
    ]);
    logger.info("worker stopped");
  } catch (error) {
    if (!stopController.signal.aborted) throw error;
  } finally {
    process.removeListener("SIGINT", onSigint);
    process.removeListener("SIGTERM", onSigterm);
  }
}

main().catch((error) => {
  logger.error("worker failed to start", { error: error instanceof Error ? error.message : String(error) });
  process.exit(1);
});
