import os from "node:os";
import type { HealthStatus, SystemUsage } from "../../shared/types.js";
import { pool } from "../db/index.js";
import { runCommand } from "./commands.js";

const HEALTH_CHECK_TIMEOUT_MS = 3_000;

async function withHealthDeadline<T>(operation: Promise<T>, message: string, onTimeout?: () => void) {
  let timer: NodeJS.Timeout | undefined;
  try {
    return await Promise.race([
      operation,
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(() => {
          onTimeout?.();
          reject(new Error(message));
        }, HEALTH_CHECK_TIMEOUT_MS);
        timer.unref();
      })
    ]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}

function parseDfLine(line: string) {
  const parts = line.trim().split(/\s+/);
  if (parts.length < 6) {
    return null;
  }
  const [filesystem, size, used, available, percent, mount] = parts;
  return {
    filesystem,
    size: Number(size) * 1024,
    used: Number(used) * 1024,
    available: Number(available) * 1024,
    usedPercent: Number(percent.replace("%", "")),
    mount
  };
}

export async function healthStatus(): Promise<HealthStatus> {
  const dockerController = new AbortController();
  const [database, docker] = await Promise.all([
    withHealthDeadline(
      pool.query({ text: "SELECT 1", query_timeout: HEALTH_CHECK_TIMEOUT_MS } as { text: string }),
      `Database health check timed out after ${HEALTH_CHECK_TIMEOUT_MS}ms.`
    )
      .then(() => ({ ok: true }))
      .catch((error: unknown) => ({ ok: false, message: error instanceof Error ? error.message : String(error) })),
    withHealthDeadline(
      runCommand("docker", ["version", "--format", "{{.Server.Version}}"], {
        signal: dockerController.signal,
        timeoutMs: HEALTH_CHECK_TIMEOUT_MS,
        killProcessGroup: true,
        maxOutputBytes: 64 * 1024
      }),
      `Docker health check timed out after ${HEALTH_CHECK_TIMEOUT_MS}ms.`,
      () => dockerController.abort()
    )
      .then((result) => result.exitCode === 0
        ? { ok: true, message: result.output.trim() }
        : { ok: false, message: result.output.trim() })
      .catch((error: unknown) => ({ ok: false, message: error instanceof Error ? error.message : String(error) }))
  ]);
  const checks: HealthStatus["checks"] = { database, docker };

  return {
    ok: Object.values(checks).every((check) => check.ok),
    uptimeSeconds: Math.round(process.uptime()),
    checkedAt: new Date().toISOString(),
    checks
  };
}

export async function systemUsage(): Promise<SystemUsage> {
  const total = os.totalmem();
  const free = os.freemem();
  const used = total - free;
  const load = os.loadavg()[0] ?? 0;
  const cpuCount = os.cpus().length || 1;
  const cpuLoadPercent = Math.min(100, Math.round((load / cpuCount) * 100));

  const df = await runCommand("df", ["-kP", "/", "/projects"]);
  const storage = df.output
    .split("\n")
    .slice(1)
    .map(parseDfLine)
    .filter((item): item is NonNullable<typeof item> => Boolean(item));

  return {
    cpuLoadPercent,
    memory: {
      total,
      used,
      free,
      usedPercent: Math.round((used / total) * 100)
    },
    storage,
  };
}
