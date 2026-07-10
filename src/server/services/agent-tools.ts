import fs from "node:fs/promises";
import path from "node:path";
import { config } from "../config.js";
import { HttpError } from "../http-utils.js";
import { runCommand } from "./commands.js";

export type AgentToolName = "list_files" | "search_files" | "read_file" | "write_file" | "replace_text" | "run_command";

export const agentToolDefinitions = [
  { name: "list_files", description: "List files and directories inside the task workspace.", input_schema: { type: "object", properties: { path: { type: "string" }, depth: { type: "integer", minimum: 1, maximum: 6 } }, additionalProperties: false } },
  { name: "search_files", description: "Search text in workspace files and return matching lines.", input_schema: { type: "object", properties: { query: { type: "string" }, path: { type: "string" } }, required: ["query"], additionalProperties: false } },
  { name: "read_file", description: "Read a UTF-8 text file with optional one-based line bounds.", input_schema: { type: "object", properties: { path: { type: "string" }, start_line: { type: "integer", minimum: 1 }, end_line: { type: "integer", minimum: 1 } }, required: ["path"], additionalProperties: false } },
  { name: "write_file", description: "Create or replace a UTF-8 text file inside the workspace.", input_schema: { type: "object", properties: { path: { type: "string" }, content: { type: "string" } }, required: ["path", "content"], additionalProperties: false } },
  { name: "replace_text", description: "Replace one exact text occurrence in a UTF-8 file. Fails if the old text is absent or occurs more than once.", input_schema: { type: "object", properties: { path: { type: "string" }, old_text: { type: "string" }, new_text: { type: "string" } }, required: ["path", "old_text", "new_text"], additionalProperties: false } },
  { name: "run_command", description: "Run a non-interactive shell command in the isolated task container at /workspace. Git credentials and Docker are unavailable.", input_schema: { type: "object", properties: { command: { type: "string" }, timeout_ms: { type: "integer", minimum: 1000, maximum: 600000 } }, required: ["command"], additionalProperties: false } }
] as const;

function stringArg(input: Record<string, unknown>, key: string, required = true) {
  const value = typeof input[key] === "string" ? input[key] as string : "";
  if (required && !value) throw new HttpError(400, `${key} is required.`);
  return value;
}

async function safePath(root: string, candidate: string, forWrite = false) {
  const canonicalRoot = await fs.realpath(root);
  const relative = candidate.trim().replace(/^\/+/, "") || ".";
  if (relative.split(/[\\/]/).includes(".git")) throw new HttpError(400, "Direct access to Git metadata is not allowed.");
  const resolved = path.resolve(root, relative);
  if (resolved !== root && !resolved.startsWith(`${root}${path.sep}`)) throw new HttpError(400, "Path is outside the task workspace.");
  const exists = await fs.lstat(resolved).then(() => true).catch(() => false);
  let checkPath = forWrite && !exists ? path.dirname(resolved) : resolved;
  while (forWrite && !await fs.lstat(checkPath).then(() => true).catch(() => false) && checkPath !== root) checkPath = path.dirname(checkPath);
  const real = await fs.realpath(checkPath);
  if (real !== canonicalRoot && !real.startsWith(`${canonicalRoot}${path.sep}`)) throw new HttpError(400, "Symlink resolves outside the task workspace.");
  return resolved;
}

async function listFiles(root: string, relativePath: string, maxDepth: number) {
  const start = await safePath(root, relativePath);
  const output: string[] = [];
  async function walk(current: string, depth: number) {
    if (output.length >= 500) return;
    const entries = await fs.readdir(current, { withFileTypes: true });
    entries.sort((a, b) => a.name.localeCompare(b.name));
    for (const entry of entries) {
      if (entry.name === ".git" || entry.name === "node_modules") continue;
      const full = path.join(current, entry.name);
      output.push(`${path.relative(root, full)}${entry.isDirectory() ? "/" : ""}`);
      if (entry.isDirectory() && depth < maxDepth) await walk(full, depth + 1);
      if (output.length >= 500) break;
    }
  }
  await walk(start, 1);
  return output.join("\n") || "(empty directory)";
}

async function searchFiles(root: string, relativePath: string, query: string) {
  const start = await safePath(root, relativePath);
  const matches: string[] = [];
  async function walk(current: string) {
    if (matches.length >= 200) return;
    const entries = await fs.readdir(current, { withFileTypes: true });
    for (const entry of entries) {
      if (entry.name === ".git" || entry.name === "node_modules") continue;
      const full = path.join(current, entry.name);
      if (entry.isDirectory()) await walk(full);
      else if (entry.isFile()) {
        const stat = await fs.stat(full);
        if (stat.size > 1024 * 1024) continue;
        const content = await fs.readFile(full, "utf8").catch(() => "");
        content.split("\n").forEach((line, index) => {
          if (matches.length < 200 && line.includes(query)) matches.push(`${path.relative(root, full)}:${index + 1}:${line.slice(0, 500)}`);
        });
      }
      if (matches.length >= 200) return;
    }
  }
  await walk(start);
  return matches.join("\n") || "No matches found.";
}

export async function resolveHostMountPath(containerPath: string) {
  const hostname = process.env.HOSTNAME;
  if (!hostname) return containerPath;
  const inspected = await runCommand("docker", ["inspect", hostname, "--format", "{{json .Mounts}}"], { maxOutputBytes: 256 * 1024, timeoutMs: 10_000 });
  if (inspected.exitCode !== 0) return containerPath;
  try {
    const mounts = JSON.parse(inspected.output) as Array<{ Source: string; Destination: string }>;
    const match = mounts.filter((mount) => containerPath === mount.Destination || containerPath.startsWith(`${mount.Destination}/`)).sort((a, b) => b.Destination.length - a.Destination.length)[0];
    return match ? path.join(match.Source, path.relative(match.Destination, containerPath)) : containerPath;
  } catch {
    return containerPath;
  }
}

export class AgentSandbox {
  readonly containerName: string;
  private started = false;

  constructor(private readonly runId: string, private readonly worktreePath: string, private readonly image: string) {
    this.containerName = `yanto-agent-${runId.replace(/[^a-zA-Z0-9_.-]/g, "-").slice(0, 48)}`;
  }

  async start() {
    const hostPath = await resolveHostMountPath(this.worktreePath);
    const result = await runCommand("docker", [
      "run", "-d", "--rm", "--name", this.containerName,
      "--workdir", "/workspace", "--network", "bridge",
      "--memory", "4g", "--cpus", "2", "--pids-limit", "512",
      "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
      "-v", `${hostPath}:/workspace`,
      "--entrypoint", "sh", this.image, "-c", "while :; do sleep 3600; done"
    ], { timeoutMs: 10 * 60 * 1000, maxOutputBytes: 512 * 1024 });
    if (result.exitCode !== 0) throw new Error(result.output.trim() || `Unable to start agent image ${this.image}.`);
    this.started = true;
  }

  async stop() {
    if (!this.started) return;
    this.started = false;
    await runCommand("docker", ["rm", "-f", this.containerName], { timeoutMs: 30_000, maxOutputBytes: 64 * 1024 });
  }

  async execute(name: string, rawInput: unknown): Promise<string> {
    const input = rawInput && typeof rawInput === "object" ? rawInput as Record<string, unknown> : {};
    switch (name as AgentToolName) {
      case "list_files":
        return listFiles(this.worktreePath, stringArg(input, "path", false) || ".", Math.min(6, Math.max(1, Number(input.depth) || 3)));
      case "search_files":
        return searchFiles(this.worktreePath, stringArg(input, "path", false) || ".", stringArg(input, "query"));
      case "read_file": {
        const file = await safePath(this.worktreePath, stringArg(input, "path"));
        const lines = (await fs.readFile(file, "utf8")).split("\n");
        const start = Math.max(1, Number(input.start_line) || 1);
        const end = Math.min(lines.length, Number(input.end_line) || start + 399);
        return lines.slice(start - 1, end).map((line, index) => `${start + index}: ${line}`).join("\n");
      }
      case "write_file": {
        const file = await safePath(this.worktreePath, stringArg(input, "path"), true);
        await fs.mkdir(path.dirname(file), { recursive: true });
        await fs.writeFile(file, stringArg(input, "content", false), "utf8");
        return `Wrote ${path.relative(this.worktreePath, file)}.`;
      }
      case "replace_text": {
        const file = await safePath(this.worktreePath, stringArg(input, "path"));
        const oldText = stringArg(input, "old_text");
        const content = await fs.readFile(file, "utf8");
        const first = content.indexOf(oldText);
        if (first < 0) throw new Error("old_text was not found.");
        if (content.indexOf(oldText, first + oldText.length) >= 0) throw new Error("old_text occurs more than once; include more context.");
        await fs.writeFile(file, `${content.slice(0, first)}${stringArg(input, "new_text", false)}${content.slice(first + oldText.length)}`, "utf8");
        return `Updated ${path.relative(this.worktreePath, file)}.`;
      }
      case "run_command": {
        if (!this.started) throw new Error("Task sandbox is not running.");
        const timeoutMs = Math.min(config.agentCommandTimeoutMs, Math.max(1_000, Number(input.timeout_ms) || config.agentCommandTimeoutMs));
        const result = await runCommand("docker", ["exec", this.containerName, "sh", "-lc", stringArg(input, "command")], {
          timeoutMs,
          maxOutputBytes: config.agentCommandOutputMaxBytes
        });
        return `exit_code=${result.exitCode}${result.timedOut ? " timed_out=true" : ""}${result.truncated ? " truncated=true" : ""}\n${result.output}`;
      }
      default:
        throw new Error(`Unknown tool: ${name}`);
    }
  }
}
