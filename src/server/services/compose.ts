import fs from "node:fs/promises";
import path from "node:path";
import YAML from "yaml";
import type { ProjectRow } from "../db/schema.js";
import { normalizeComposeFile, pathExists } from "./paths.js";
import { runCommand } from "./commands.js";
import { listContainers } from "./docker.js";

type ComposeDocument = {
  services?: Record<string, unknown>;
};

const restartOverrideFile = ".yanto.restart.override.yml";
const portRangeLimit = 512;

export type ComposePublishedPort = {
  service: string;
  port: number;
  protocol: "tcp" | "udp";
  source: string;
};

export type ComposePortConflict = ComposePublishedPort & {
  conflictWith: string;
};

type ComposePortCheckOptions = {
  ignoreComposeProject?: string;
};

export function autoStartOverrideFile() {
  return restartOverrideFile;
}

export function buildAutoStartOverride(composeContent: string) {
  const parsed = YAML.parse(composeContent) as ComposeDocument | null;
  const services = parsed?.services;
  if (!services || typeof services !== "object" || Array.isArray(services)) {
    throw new Error("Compose file must include a services object before auto start can be enabled.");
  }

  const serviceNames = Object.keys(services);
  if (!serviceNames.length) {
    throw new Error("Compose file must include at least one service before auto start can be enabled.");
  }

  return YAML.stringify({
    services: Object.fromEntries(serviceNames.map((serviceName) => [serviceName, { restart: "unless-stopped" }]))
  });
}

export function buildDockerImageCompose(image: string) {
  return YAML.stringify({ services: { app: { image } } });
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function normalizeProtocol(value: unknown): "tcp" | "udp" {
  return String(value ?? "tcp").toLowerCase() === "udp" ? "udp" : "tcp";
}

function expandPortRange(value: unknown): number[] {
  const text = String(value ?? "").trim();
  if (!/^\d+(?:-\d+)?$/.test(text)) return [];
  const [startText, endText] = text.split("-");
  const start = Number(startText);
  const end = Number(endText ?? startText);
  if (!Number.isInteger(start) || !Number.isInteger(end) || start < 1 || end > 65535 || end < start || end - start > portRangeLimit) return [];
  return Array.from({ length: end - start + 1 }, (_, index) => start + index);
}

function splitShortPortSyntax(value: string) {
  const parts: string[] = [];
  let current = "";
  let bracketDepth = 0;
  for (const char of value) {
    if (char === "[") bracketDepth += 1;
    if (char === "]") bracketDepth = Math.max(0, bracketDepth - 1);
    if (char === ":" && bracketDepth === 0) {
      parts.push(current);
      current = "";
      continue;
    }
    current += char;
  }
  parts.push(current);
  return parts;
}

function extractShortPublishedPorts(service: string, value: string): ComposePublishedPort[] {
  const [portSpec, protocolText] = value.trim().split("/");
  if (!portSpec || portSpec.includes("$")) return [];
  const parts = splitShortPortSyntax(portSpec);
  if (parts.length < 2) return [];
  const hostPortSpec = parts.length === 2 ? parts[0] : parts[parts.length - 2];
  return expandPortRange(hostPortSpec).map((port) => ({
    service,
    port,
    protocol: normalizeProtocol(protocolText),
    source: value
  }));
}

function extractLongPublishedPorts(service: string, value: Record<string, unknown>): ComposePublishedPort[] {
  if (value.published === undefined || value.published === null || String(value.published).includes("$")) return [];
  const source = YAML.stringify(value).trim().replace(/\n+/g, " ");
  return expandPortRange(value.published).map((port) => ({
    service,
    port,
    protocol: normalizeProtocol(value.protocol),
    source
  }));
}

export function extractPublishedComposePorts(composeContent: string): ComposePublishedPort[] {
  const parsed = YAML.parse(composeContent) as unknown;
  if (!isRecord(parsed) || !isRecord(parsed.services)) return [];

  const published: ComposePublishedPort[] = [];
  for (const [service, config] of Object.entries(parsed.services)) {
    if (!isRecord(config) || !Array.isArray(config.ports)) continue;
    for (const port of config.ports) {
      if (typeof port === "string" || typeof port === "number") {
        published.push(...extractShortPublishedPorts(service, String(port)));
      } else if (isRecord(port)) {
        published.push(...extractLongPublishedPorts(service, port));
      }
    }
  }
  return published;
}

function portKey(port: Pick<ComposePublishedPort, "port" | "protocol">) {
  return `${port.protocol}:${port.port}`;
}

function parseDockerPublishedPorts(ports: string) {
  const result: { port: number; protocol: "tcp" | "udp" }[] = [];
  for (const segment of ports.split(",")) {
    const protocol = normalizeProtocol(segment.match(/\/(tcp|udp)\b/i)?.[1]);
    const match = segment.trim().match(/(?:^|:)(\d+(?:-\d+)?)\s*->/);
    if (!match) continue;
    for (const port of expandPortRange(match[1])) {
      result.push({ port, protocol });
    }
  }
  return result;
}

async function listListeningTcpPorts() {
  const result = await runCommand("sh", ["-lc", "command -v lsof >/dev/null 2>&1 && lsof -nP -iTCP -sTCP:LISTEN || true"]);
  const ports = new Map<string, string>();
  for (const line of result.output.split("\n")) {
    const port = line.match(/\bTCP\s+\S+:(\d+)\s+\(LISTEN\)/)?.[1];
    if (!port) continue;
    const command = line.trim().split(/\s+/)[0] || "process";
    ports.set(`tcp:${port}`, `${command} process`);
  }
  return ports;
}

async function usedPublishedHostPorts(options: ComposePortCheckOptions = {}) {
  const used = new Map<string, string[]>();
  const ignoredKeys = new Set<string>();
  const add = (key: string, owner: string) => used.set(key, [...(used.get(key) ?? []), owner]);

  for (const container of await listContainers()) {
    if (container.state !== "running") continue;
    for (const port of parseDockerPublishedPorts(container.ports)) {
      if (options.ignoreComposeProject && container.composeProject === options.ignoreComposeProject) {
        ignoredKeys.add(portKey(port));
        continue;
      }
      add(portKey(port), `container ${container.name}`);
    }
  }

  const hostPorts = await listListeningTcpPorts().catch(() => new Map<string, string>());
  for (const [key, owner] of hostPorts) {
    if (!ignoredKeys.has(key)) add(key, owner);
  }

  return used;
}

export async function findComposePortConflicts(composeContent: string, options: ComposePortCheckOptions = {}): Promise<ComposePortConflict[]> {
  const published = extractPublishedComposePorts(composeContent);
  if (!published.length) return [];

  const conflicts: ComposePortConflict[] = [];
  const seen = new Map<string, ComposePublishedPort>();
  for (const port of published) {
    const key = portKey(port);
    const previous = seen.get(key);
    if (previous) {
      conflicts.push({ ...port, conflictWith: `service ${previous.service}` });
    } else {
      seen.set(key, port);
    }
  }

  const usedPorts = await usedPublishedHostPorts(options);
  for (const port of published) {
    const owners = usedPorts.get(portKey(port));
    if (!owners?.length) continue;
    conflicts.push({ ...port, conflictWith: owners.join(", ") });
  }

  return conflicts;
}

export function formatComposePortConflictMessage(conflicts: ComposePortConflict[]) {
  const details = conflicts
    .slice(0, 8)
    .map((conflict) => `${conflict.protocol.toUpperCase()} ${conflict.port} from service ${conflict.service} conflicts with ${conflict.conflictWith}`)
    .join("; ");
  const suffix = conflicts.length > 8 ? `; and ${conflicts.length - 8} more` : "";
  return `Docker compose port conflict detected: ${details}${suffix}. Change the published host port or stop the conflicting service.`;
}

export async function assertComposePortsAvailable(composeContent: string, options: ComposePortCheckOptions = {}) {
  const conflicts = await findComposePortConflicts(composeContent, options);
  if (conflicts.length) {
    throw new Error(formatComposePortConflictMessage(conflicts));
  }
}

export async function readProjectCompose(project: ProjectRow) {
  const composeFile = normalizeComposeFile(project.composeFile);
  const target = path.join(project.localPath, composeFile);
  if (!(await pathExists(target))) {
    if (project.dockerImage) {
      return { composeFile, content: buildDockerImageCompose(project.dockerImage), exists: true };
    }
    return { composeFile, content: "", exists: false };
  }

  return {
    composeFile,
    content: await fs.readFile(target, "utf8"),
    exists: true
  };
}
