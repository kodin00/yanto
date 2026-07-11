import { createHash } from "node:crypto";
import fs from "node:fs/promises";
import os from "node:os";
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
  { name: "run_command", description: "Run a non-interactive shell command with the task worktree as its working directory. Use repository-local paths only.", input_schema: { type: "object", properties: { command: { type: "string" }, timeout_ms: { type: "integer", minimum: 1000, maximum: 600000 } }, required: ["command"], additionalProperties: false } }
] as const;

function stringArg(input: Record<string, unknown>, key: string, required = true) {
  const value = typeof input[key] === "string" ? input[key] as string : "";
  if (required && !value) throw new HttpError(400, `${key} is required.`);
  return value;
}

async function safePath(root: string, candidate: string, forWrite = false) {
  const resolvedRoot = path.resolve(root);
  const canonicalRoot = await fs.realpath(resolvedRoot);
  const relative = candidate.trim().replace(/^\/+/, "") || ".";
  if (relative.split(/[\\/]/).includes(".git")) throw new HttpError(400, "Direct access to Git metadata is not allowed.");
  const resolved = path.resolve(resolvedRoot, relative);
  if (resolved !== resolvedRoot && !resolved.startsWith(`${resolvedRoot}${path.sep}`)) throw new HttpError(400, "Path is outside the task workspace.");
  const exists = await fs.lstat(resolved).then(() => true).catch(() => false);
  let checkPath = forWrite && !exists ? path.dirname(resolved) : resolved;
  while (forWrite && !await fs.lstat(checkPath).then(() => true).catch(() => false) && checkPath !== resolvedRoot) checkPath = path.dirname(checkPath);
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

function workspaceEnvironment(worktreePath: string, homePath: string): NodeJS.ProcessEnv {
  const copy = (name: string) => process.env[name] ? { [name]: process.env[name] } : {};
  return {
    ...copy("PATH"),
    ...copy("LANG"),
    ...copy("LC_ALL"),
    ...copy("TERM"),
    ...copy("TMPDIR"),
    ...copy("SSL_CERT_FILE"),
    ...copy("SSL_CERT_DIR"),
    HOME: homePath,
    CI: "true",
    DOCKER_HOST: "unix:///dev/null",
    YANTO_AGENT_WORKSPACE: worktreePath
  };
}

/** Host-native tools scoped to one Yanto-managed task worktree. */
export class AgentWorkspace {
  private started = false;
  private launchPromise?: Promise<void>;
  private cleanupPromise?: Promise<void>;
  private activeCommand?: Promise<Awaited<ReturnType<typeof runCommand>>>;
  private readonly signal: AbortSignal;
  private readonly homePath: string;

  constructor(
    private readonly worktreePath: string,
    options: { signal?: AbortSignal } = {}
  ) {
    this.signal = options.signal ?? new AbortController().signal;
    const digest = createHash("sha256").update(path.resolve(worktreePath)).digest("hex").slice(0, 32);
    this.homePath = path.join(os.tmpdir(), "yanto-agent-homes", digest);
  }

  async start() {
    if (this.launchPromise) return this.launchPromise;
    this.launchPromise = (async () => {
      this.signal.throwIfAborted();
      const stat = await fs.stat(await fs.realpath(this.worktreePath));
      if (!stat.isDirectory()) throw new Error("Task worktree is not a directory.");
      await fs.mkdir(this.homePath, { recursive: true, mode: 0o700 });
      this.signal.throwIfAborted();
      this.started = true;
    })();
    return this.launchPromise;
  }

  async stop() {
    if (this.cleanupPromise) return this.cleanupPromise;
    this.cleanupPromise = (async () => {
      await this.launchPromise?.catch(() => undefined);
      await this.activeCommand?.catch(() => undefined);
      this.started = false;
      await fs.rm(this.homePath, { recursive: true, force: true });
    })();
    return this.cleanupPromise;
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
        if (!this.started) throw new Error("Task workspace is not running.");
        this.signal.throwIfAborted();
        const cwd = await safePath(this.worktreePath, ".");
        const timeoutMs = Math.min(config.agentCommandTimeoutMs, Math.max(1_000, Number(input.timeout_ms) || config.agentCommandTimeoutMs));
        const command = runCommand("sh", ["-lc", stringArg(input, "command")], {
          cwd,
          env: workspaceEnvironment(cwd, this.homePath),
          inheritEnv: false,
          signal: this.signal,
          killProcessGroup: true,
          timeoutMs,
          maxOutputBytes: config.agentCommandOutputMaxBytes
        });
        this.activeCommand = command;
        const result = await command.finally(() => {
          if (this.activeCommand === command) this.activeCommand = undefined;
        });
        this.signal.throwIfAborted();
        return `exit_code=${result.exitCode}${result.timedOut ? " timed_out=true" : ""}${result.truncated ? " truncated=true" : ""}\n${result.output}`;
      }
      default:
        throw new Error(`Unknown tool: ${name}`);
    }
  }
}
