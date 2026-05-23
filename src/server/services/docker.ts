import type { ContainerInfo } from "../../shared/types.js";
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

function parseLabels(labels: string | undefined) {
  const result = new Map<string, string>();
  for (const label of labels?.split(",") ?? []) {
    const separator = label.indexOf("=");
    if (separator <= 0) continue;
    result.set(label.slice(0, separator), label.slice(separator + 1));
  }
  return result;
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

export async function listContainers(): Promise<ContainerInfo[]> {
  const ps = await runCommand("docker", ["ps", "-a", "--format", "{{json .}}"]);
  if (ps.exitCode !== 0) {
    throw new Error(ps.output || "Unable to list Docker containers.");
  }

  const stats = await runCommand("docker", ["stats", "--no-stream", "--format", "{{json .}}"]);
  const statsById = new Map(parseJsonLines<DockerStatsLine>(stats.output).map((item) => [item.ID, item]));

  return parseJsonLines<DockerPsLine>(ps.output).map((container) => {
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
  });
}

export async function containerLogs(containerId: string) {
  const result = await runCommand("docker", ["logs", "--tail", "500", containerId]);
  if (result.exitCode !== 0) {
    throw new Error(result.output || "Unable to read container logs.");
  }
  return result.output;
}

async function assertContainerCanBeControlled(containerId: string) {
  const result = await runCommand("docker", ["inspect", "--format", "{{.Name}}", containerId]);
  if (result.exitCode !== 0) {
    throw new Error(result.output || "Unable to inspect container.");
  }
  const name = result.output.trim().replace(/^\//, "");
  if (/^yanto-(app|postgres)-\d+$/.test(name)) {
    throw new Error("Yanto app containers are protected from stop and restart actions.");
  }
}

export async function stopContainer(containerId: string) {
  await assertContainerCanBeControlled(containerId);
  const result = await runCommand("docker", ["stop", containerId]);
  if (result.exitCode !== 0) {
    throw new Error(result.output || "Unable to stop container.");
  }
}

export async function restartContainer(containerId: string) {
  await assertContainerCanBeControlled(containerId);
  const result = await runCommand("docker", ["restart", containerId]);
  if (result.exitCode !== 0) {
    throw new Error(result.output || "Unable to restart container.");
  }
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
    ["docker", ["system", "df"]],
    ["docker", ["builder", "du"]],
    ["docker", ["container", "ls", "-a", "--filter", "status=exited", "--format", "table {{.ID}}\\t{{.Names}}\\t{{.Status}}"]],
    ["docker", ["image", "ls", "--filter", "dangling=true", "--format", "table {{.ID}}\\t{{.Repository}}\\t{{.Tag}}\\t{{.Size}}"]],
    ["docker", ["network", "ls", "--filter", "dangling=true", "--format", "table {{.ID}}\\t{{.Name}}\\t{{.Driver}}"]]
  ] as const;

  let output = "";
  for (const [command, args] of commands) {
    const result = await runCommand(command, [...args]);
    output += `$ ${command} ${args.join(" ")}\n${result.output}\n`;
    if (result.exitCode !== 0) {
      throw new Error(output);
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
  return output;
}
