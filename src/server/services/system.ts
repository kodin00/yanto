import os from "node:os";
import type { SystemUsage } from "../../shared/types.js";
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
