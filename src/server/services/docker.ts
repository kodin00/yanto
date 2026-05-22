import type { ContainerInfo } from "../../shared/types.js";
import { runCommand } from "./commands.js";

type DockerPsLine = {
  ID: string;
  Names: string;
  Image: string;
  Status: string;
  State: string;
  Ports: string;
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

export async function listContainers(): Promise<ContainerInfo[]> {
  const ps = await runCommand("docker", ["ps", "-a", "--format", "{{json .}}"]);
  if (ps.exitCode !== 0) {
    throw new Error(ps.output || "Unable to list Docker containers.");
  }

  const stats = await runCommand("docker", ["stats", "--no-stream", "--format", "{{json .}}"]);
  const statsById = new Map(parseJsonLines<DockerStatsLine>(stats.output).map((item) => [item.ID, item]));

  return parseJsonLines<DockerPsLine>(ps.output).map((container) => {
    const stat = statsById.get(container.ID);
    return {
      id: container.ID,
      name: container.Names,
      image: container.Image,
      status: container.Status,
      state: container.State,
      ports: container.Ports,
      cpuPercent: stat?.CPUPerc ?? "0%",
      memoryUsage: stat?.MemUsage ?? "-",
      memoryPercent: stat?.MemPerc ?? "0%"
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

export async function stopContainer(containerId: string) {
  const result = await runCommand("docker", ["stop", containerId]);
  if (result.exitCode !== 0) {
    throw new Error(result.output || "Unable to stop container.");
  }
}

export async function restartContainer(containerId: string) {
  const result = await runCommand("docker", ["restart", containerId]);
  if (result.exitCode !== 0) {
    throw new Error(result.output || "Unable to restart container.");
  }
}

export async function cleanupDocker() {
  const commands = [
    ["docker", ["builder", "prune", "-f"]],
    ["docker", ["image", "prune", "-f"]],
    ["docker", ["container", "prune", "-f"]],
    ["docker", ["network", "prune", "-f"]],
    ["sh", ["-lc", "command -v apt-get >/dev/null 2>&1 && apt-get clean || true"]]
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
