import type { ContainerInfo } from "../../shared/types.js";
import { HttpError } from "../http-utils.js";
import { runCommand } from "./commands.js";

type DockerPsLine = {
  ID: string;
  Names: string;
  Image: string;
  Status: string;
  State: string;
  Ports: string;
  CreatedAt?: string;
  Labels?: string;
};

type DockerStatsLine = {
  ID: string;
  CPUPerc: string;
  MemUsage: string;
  MemPerc: string;
};

function parseJsonLines<T>(input: string) {
  return input
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .flatMap((line) => {
      try {
        return [JSON.parse(line) as T];
      } catch {
        return [];
      }
    });
}

export function validateContainerId(id: string): string {
  if (!/^[a-zA-Z0-9][a-zA-Z0-9_.-]*$/.test(id)) {
    throw new HttpError(400, "Invalid container identifier.");
  }
  return id;
}

function parseLabels(labels: string | undefined) {
  const result = new Map<string, string>();
  for (const label of labels?.split(",") ?? []) {
    const separator = label.indexOf("=");
    if (separator <= 0) continue;
    result.set(label.slice(0, separator), label.slice(separator + 1));
  }
  return result;
}

function createdAtTime(value: string | null) {
  return value ? Date.parse(value) : 0;
}

export function normalizeDockerCreatedAt(value: string | undefined) {
  if (!value) return null;
  const parsed = Date.parse(value);
  if (!Number.isNaN(parsed)) {
    return new Date(parsed).toISOString();
  }

  const match = value.match(/^(\d{4}-\d{2}-\d{2}) (\d{2}:\d{2}:\d{2}) ([+-]\d{2})(\d{2})(?:\s+\S+)?$/);
  if (!match) return null;
  const [, date, time, offsetHour, offsetMinute] = match;
  const normalized = `${date}T${time}${offsetHour}:${offsetMinute}`;
  const normalizedParsed = Date.parse(normalized);
  return Number.isNaN(normalizedParsed) ? null : new Date(normalizedParsed).toISOString();
}

export function isPostgresContainerLike(container: Pick<ContainerInfo, "name" | "image"> & { composeService?: string | null }) {
  const haystack = [container.name, container.image, container.composeService ?? ""].join(" ").toLowerCase();
  return /\b(postgres|postgresql|postgis|timescale|timescaledb|pgvector)\b/.test(haystack) || haystack.includes("bitnami/postgresql");
}

let containerCache: ContainerInfo[] | null = null;
let containerCacheTime = 0;
let containerCacheRevision = 0;
let containerReadRequest: { revision: number; promise: Promise<ContainerInfo[]> } | null = null;
const CONTAINER_CACHE_TTL_MS = 5000;

export function invalidateContainerCache() {
  containerCache = null;
  containerCacheTime = 0;
  containerCacheRevision += 1;
}

export async function listContainers(): Promise<ContainerInfo[]> {
  const now = Date.now();
  if (containerCache && now - containerCacheTime < CONTAINER_CACHE_TTL_MS) {
    return containerCache;
  }

  const revision = containerCacheRevision;
  if (containerReadRequest?.revision === revision) return containerReadRequest.promise;

  const promise = (async () => {
    const ps = await runCommand("docker", ["ps", "-a", "--format", "{{json .}}"]);
    if (ps.exitCode !== 0) {
      throw new Error(ps.output || "Unable to list Docker containers.");
    }

    const stats = await runCommand("docker", ["stats", "--no-stream", "--format", "{{json .}}"]);
    const statsById = new Map(parseJsonLines<DockerStatsLine>(stats.output).map((item) => [item.ID, item]));

    const result = parseJsonLines<DockerPsLine>(ps.output)
      .map((container) => {
        const stat = statsById.get(container.ID);
        const labels = parseLabels(container.Labels);
        const composeService = labels.get("com.docker.compose.service") ?? null;
        const row = {
          id: container.ID,
          name: container.Names,
          image: container.Image,
          status: container.Status,
          state: container.State,
          ports: container.Ports,
          createdAt: normalizeDockerCreatedAt(container.CreatedAt),
          cpuPercent: stat?.CPUPerc ?? "0%",
          memoryUsage: stat?.MemUsage ?? "-",
          memoryPercent: stat?.MemPerc ?? "0%",
          composeProject: labels.get("com.docker.compose.project") ?? null,
          composeService
        };
        return {
          ...row,
          isPostgresCandidate: isPostgresContainerLike(row)
        };
      })
      .sort((a, b) => createdAtTime(b.createdAt) - createdAtTime(a.createdAt));

    if (containerCacheRevision !== revision) {
      // A container action invalidated this snapshot while Docker was reading
      // it. Re-enter through the current revision so existing callers do not
      // render a result that predates the completed mutation.
      return listContainers();
    }
    containerCache = result;
    containerCacheTime = Date.now();
    return result;
  })().finally(() => {
    if (containerReadRequest?.promise === promise) containerReadRequest = null;
  });

  containerReadRequest = { revision, promise };
  return promise;
}

export async function containerLogs(containerId: string) {
  validateContainerId(containerId);
  const result = await runCommand("docker", ["logs", "--tail", "500", containerId]);
  if (result.exitCode !== 0) {
    throw new Error(result.output || "Unable to read container logs.");
  }
  return result.output;
}

async function assertContainerCanBeControlled(containerId: string) {
  validateContainerId(containerId);
  const result = await runCommand("docker", ["inspect", "--format", "{{.Name}}", containerId]);
  if (result.exitCode !== 0) {
    throw new Error(result.output || "Unable to inspect container.");
  }
  const name = result.output.trim().replace(/^\//, "");
  if (/^yanto-(app|postgres)-\d+$/.test(name)) {
    throw new Error("Yanto app containers are protected from stop, restart, and exec actions.");
  }
}

export async function execInContainer(containerId: string, command: string) {
  await assertContainerCanBeControlled(containerId);
  const result = await runCommand("docker", ["exec", containerId, "sh", "-lc", command]);
  return {
    output: result.output,
    exitCode: result.exitCode,
    truncated: result.truncated,
    timedOut: result.timedOut
  };
}

export async function stopContainer(containerId: string) {
  await assertContainerCanBeControlled(containerId);
  const result = await runCommand("docker", ["stop", containerId]);
  if (result.exitCode !== 0) {
    throw new Error(result.output || "Unable to stop container.");
  }
  invalidateContainerCache();
}

export async function startContainer(containerId: string) {
  await assertContainerCanBeControlled(containerId);
  const result = await runCommand("docker", ["start", containerId]);
  if (result.exitCode !== 0) {
    throw new Error(result.output || "Unable to start container.");
  }
  invalidateContainerCache();
}

export async function restartContainer(containerId: string) {
  await assertContainerCanBeControlled(containerId);
  const result = await runCommand("docker", ["restart", containerId]);
  if (result.exitCode !== 0) {
    throw new Error(result.output || "Unable to restart container.");
  }
  invalidateContainerCache();
}

const cleanupCommands = [
  ["docker", ["builder", "prune", "-f"]],
  ["docker", ["image", "prune", "-f"]],
  ["docker", ["container", "prune", "-f"]],
  ["docker", ["network", "prune", "-f"]],
  ["sh", ["-lc", "command -v apt-get >/dev/null 2>&1 && apt-get clean || true"]]
] as const;

export async function previewDockerCleanup() {
  const commands = [
    { command: "docker", args: ["system", "df"], required: true },
    { command: "docker", args: ["builder", "du"], required: false },
    { command: "docker", args: ["container", "ls", "-a", "--filter", "status=exited", "--format", "table {{.ID}}\\t{{.Names}}\\t{{.Status}}"], required: false },
    { command: "docker", args: ["image", "ls", "--filter", "dangling=true", "--format", "table {{.ID}}\\t{{.Repository}}\\t{{.Tag}}\\t{{.Size}}"], required: false },
    { command: "docker", args: ["network", "ls", "--filter", "dangling=true", "--format", "table {{.ID}}\\t{{.Name}}\\t{{.Driver}}"], required: false }
  ] as const;

  let output = "";
  for (const { command, args, required } of commands) {
    const result = await runCommand(command, [...args]);
    output += `$ ${command} ${args.join(" ")}\n${result.output}\n`;
    if (result.exitCode !== 0) {
      if (required) {
        throw new Error(output);
      }
      output += `Preview detail command failed; continuing with available cleanup information.\n\n`;
    }
  }
  return output;
}

export async function cleanupDocker() {
  let output = "Yanto cleanup is limited to Docker builder/image/container/network prune and package cache cleanup. Running containers and named volumes are protected.\n\n";
  for (const [command, args] of cleanupCommands) {
    const result = await runCommand(command, [...args]);
    output += `$ ${command} ${args.join(" ")}\n${result.output}\n`;
    if (result.exitCode !== 0) {
      throw new Error(output);
    }
  }
  invalidateContainerCache();
  return output;
}
