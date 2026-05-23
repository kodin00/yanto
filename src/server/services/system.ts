import os from "node:os";
import type { HealthStatus, SystemUsage } from "../../shared/types.js";
import { pool } from "../db/index.js";
import { runCommand } from "./commands.js";

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
  const checks: HealthStatus["checks"] = {
    database: { ok: false },
    docker: { ok: false }
  };

  try {
    await pool.query("SELECT 1");
    checks.database = { ok: true };
  } catch (error) {
    checks.database = { ok: false, message: error instanceof Error ? error.message : String(error) };
  }

  const docker = await runCommand("docker", ["version", "--format", "{{.Server.Version}}"]);
  checks.docker = docker.exitCode === 0 ? { ok: true, message: docker.output.trim() } : { ok: false, message: docker.output.trim() };

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
