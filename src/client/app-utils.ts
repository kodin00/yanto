import type { ContainerInfo, Deployment, Project } from "../shared/types";
import type { ProjectEnvVariable } from "./lib/api";

export const pageSize = 10;

export function bytes(value: number) {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let current = value;
  let unit = 0;
  while (current >= 1024 && unit < units.length - 1) {
    current /= 1024;
    unit += 1;
  }
  return `${current.toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`;
}

export function dateTime(value: string | null) {
  if (!value) return "-";
  return new Date(value).toLocaleString();
}

export function durationSince(value: string | null) {
  if (!value) return "-";
  const started = new Date(value).getTime();
  if (Number.isNaN(started)) return "-";
  const seconds = Math.max(0, Math.floor((Date.now() - started) / 1000));
  const units = [
    ["d", 86400],
    ["h", 3600],
    ["m", 60]
  ] as const;
  const parts: string[] = [];
  let remaining = seconds;
  for (const [label, unitSeconds] of units) {
    const amount = Math.floor(remaining / unitSeconds);
    if (amount) {
      parts.push(`${amount}${label}`);
      remaining %= unitSeconds;
    }
    if (parts.length === 2) break;
  }
  if (!parts.length) return `${seconds}s`;
  return parts.join(" ");
}

export function durationBetween(startedAt: string | null, finishedAt: string | null) {
  if (!startedAt) return "-";
  const start = new Date(startedAt).getTime();
  const end = finishedAt ? new Date(finishedAt).getTime() : Date.now();
  if (Number.isNaN(start) || Number.isNaN(end)) return "-";
  const seconds = Math.max(0, Math.floor((end - start) / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  const remainingSeconds = seconds % 60;
  if (minutes < 60) return remainingSeconds ? `${minutes}m ${remainingSeconds}s` : `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  const remainingMinutes = minutes % 60;
  return remainingMinutes ? `${hours}h ${remainingMinutes}m` : `${hours}h`;
}

export function endpoint(project: Project, baseUrl: string) {
  return `${baseUrl.replace(/\/$/, "")}/deploy?id=${project.id}`;
}

export function githubWebhookEndpoint(project: Project, baseUrl: string) {
  return `${baseUrl.replace(/\/$/, "")}/webhooks/github?id=${project.id}`;
}

export function githubRepoNameFromUrl(input: string) {
  const value = input.trim();
  if (!value) return "";

  const sshMatch = value.match(/^git@github\.com:([^/\s]+)\/([^/\s]+?)(?:\.git)?$/i);
  if (sshMatch?.[2]) return sshMatch[2];

  try {
    const url = new URL(value);
    if (url.hostname.toLowerCase() !== "github.com") return "";
    const parts = url.pathname.split("/").filter(Boolean);
    const repo = parts[1];
    return repo ? repo.replace(/\.git$/i, "") : "";
  } catch {
    return "";
  }
}

export function slugifyFolderName(input: string) {
  return input
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 64);
}

export function pageItems<T>(items: T[], page: number) {
  return items.slice((page - 1) * pageSize, page * pageSize);
}

export function totalPages(items: unknown[]) {
  return Math.max(1, Math.ceil(items.length / pageSize));
}

export function usedMemoryMb(memoryUsage: string) {
  const used = memoryUsage.split("/")[0]?.trim();
  if (!used || used === "-") return "-";
  const match = used.match(/^([\d.]+)\s*([KMGT]?i?B)$/i);
  if (!match) return used;
  const value = Number(match[1] ?? 0);
  const unit = (match[2] ?? "").toLowerCase();
  const multipliers: Record<string, number> = {
    b: 1 / 1024 / 1024,
    kb: 1 / 1024,
    kib: 1 / 1024,
    mb: 1,
    mib: 1,
    gb: 1024,
    gib: 1024,
    tb: 1024 * 1024,
    tib: 1024 * 1024
  };
  const mb = value * (multipliers[unit] ?? 1);
  return `${mb >= 10 ? mb.toFixed(0) : mb.toFixed(1)} MB`;
}

export function isProtectedYantoContainer(container: ContainerInfo) {
  return /^yanto-(app|postgres)-\d+$/.test(container.name);
}

function internalTcpPort(ports: string) {
  const published = ports.match(/->(\d+)\/tcp\b/);
  if (published?.[1]) return published[1];

  const internal = ports.match(/(?:^|,\s*)(\d+)\/tcp\b/);
  return internal?.[1] ?? "";
}

export function cloudflareServiceUrl(project: Project, containers: ContainerInfo[]) {
  const candidates = containers
    .filter((container) => container.composeProject === project.folderName && container.state === "running" && !container.isPostgresCandidate)
    .sort((a, b) => {
      const aApp = a.composeService === "app" || a.name.includes("-app-");
      const bApp = b.composeService === "app" || b.name.includes("-app-");
      return Number(bApp) - Number(aApp);
    });

  const target = candidates[0];
  if (!target) return "";

  const port = internalTcpPort(target.ports);
  return port ? `http://${target.name}:${port}` : "";
}

export function normalizeEnvRows(rows: ProjectEnvVariable[]) {
  return rows.map((row) => ({ key: row.key, value: row.value ?? "" })).sort((a, b) => a.key.localeCompare(b.key));
}

export function deploymentChanges(deployment: Deployment) {
  const extra = deployment as Deployment & { changes?: string | string[] | null; commitSha?: string | null };
  if (Array.isArray(extra.changes)) return extra.changes.join(", ");
  if (extra.changes) return extra.changes;
  if (extra.commitSha) return extra.commitSha.slice(0, 12);
  return deployment.exitCode === null ? "Running" : `Exit ${deployment.exitCode}`;
}
