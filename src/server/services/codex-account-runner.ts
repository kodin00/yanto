import { createHash } from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";
import { Codex, type CodexOptions, type ThreadEvent, type ThreadItem, type ThreadOptions } from "@openai/codex-sdk";
import { config } from "../config.js";

type Event = (kind: string, payload: Record<string, unknown>) => Promise<void>;
type Input = {
  runId: string;
  taskId: string;
  worktreePath: string;
  prompt: string;
  model: string;
  threadId: string | null;
  signal: AbortSignal;
  event: Event;
  registerStop?: (stop: () => Promise<void>) => void;
  prepareTaskHome?: (taskId: string, threadId: string | null) => Promise<string>;
  createCodex?: (options: CodexOptions) => Pick<Codex, "startThread" | "resumeThread">;
};

const CODEX_TOOL_UPDATE_INTERVAL_MS = 250;
const CODEX_TOOL_UPDATE_MAX_BYTES = 16 * 1024;

function canceledError(signal: AbortSignal) {
  return signal.reason instanceof Error ? signal.reason : new Error("Codex run canceled.");
}

async function pathExists(value: string) {
  try { await fs.stat(value); return true; } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return false;
    throw error;
  }
}

async function copyLegacyThread(sourceRoot: string, targetRoot: string, threadId: string) {
  if (!await pathExists(sourceRoot)) return;
  const visit = async (directory: string): Promise<boolean> => {
    for (const entry of await fs.readdir(directory, { withFileTypes: true })) {
      const source = path.join(directory, entry.name);
      if (entry.isDirectory()) {
        if (await visit(source)) return true;
      } else if (entry.isFile() && entry.name.includes(threadId)) {
        const relative = path.relative(sourceRoot, source);
        const target = path.join(targetRoot, relative);
        if (!await pathExists(target)) {
          await fs.mkdir(path.dirname(target), { recursive: true, mode: 0o700 });
          await fs.copyFile(source, target);
          await fs.chmod(target, 0o600);
        }
        return true;
      }
    }
    return false;
  };
  await visit(sourceRoot);
}

export async function prepareCodexTaskHome(taskId: string, threadId: string | null) {
  await fs.mkdir(config.codexHome, { recursive: true, mode: 0o700 });
  const digest = createHash("sha256").update(taskId).digest("hex").slice(0, 32);
  const taskHome = path.join(config.codexHome, "task-sessions", digest);
  await fs.mkdir(taskHome, { recursive: true, mode: 0o700 });
  const accountAuth = path.join(config.codexHome, "auth.json");
  const taskAuth = path.join(taskHome, "auth.json");
  let sourceStat;
  try { sourceStat = await fs.stat(accountAuth); } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") {
      throw new Error("Codex account credentials are unavailable. Sign in again before starting this task.");
    }
    throw error;
  }
  const targetStat = await fs.stat(taskAuth).catch((error: NodeJS.ErrnoException) => {
    if (error.code === "ENOENT") return undefined;
    throw error;
  });
  // Preserve a newer task-local refresh, but propagate a newer explicit account login.
  if (!targetStat || sourceStat.mtimeMs > targetStat.mtimeMs) {
    await fs.copyFile(accountAuth, taskAuth);
    await fs.chmod(taskAuth, 0o600);
  }
  if (threadId) {
    await Promise.all([
      copyLegacyThread(path.join(config.codexHome, "sessions"), path.join(taskHome, "sessions"), threadId),
      copyLegacyThread(path.join(config.codexHome, "archived_sessions"), path.join(taskHome, "archived_sessions"), threadId)
    ]);
  }
  return taskHome;
}

export async function clearCodexTaskAuthentication() {
  const taskHomes = path.join(config.codexHome, "task-sessions");
  let entries;
  try { entries = await fs.readdir(taskHomes, { withFileTypes: true }); } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return;
    throw error;
  }
  await Promise.all(entries
    .filter((entry) => entry.isDirectory())
    .map((entry) => fs.rm(path.join(taskHomes, entry.name, "auth.json"), { force: true })));
}

function codexEnvironment(taskHome: string): Record<string, string> {
  const environment: Record<string, string> = {
    CODEX_HOME: taskHome,
    HOME: taskHome,
    DOCKER_HOST: "unix:///dev/null"
  };
  for (const name of ["PATH", "LANG", "LC_ALL", "TERM", "TMPDIR", "SSL_CERT_FILE", "SSL_CERT_DIR", "HTTP_PROXY", "HTTPS_PROXY", "NO_PROXY"]) {
    if (process.env[name]) environment[name] = process.env[name]!;
  }
  return environment;
}

function boundedTail(value: string, maxBytes: number) {
  if (Buffer.byteLength(value, "utf8") <= maxBytes) return { output: value, outputTruncated: false };
  let low = 0;
  let high = value.length;
  while (low < high) {
    const length = Math.ceil((low + high) / 2);
    if (Buffer.byteLength(value.slice(value.length - length), "utf8") <= maxBytes) low = length;
    else high = length - 1;
  }
  return { output: value.slice(value.length - low), outputTruncated: true };
}

function toolPayload(item: ThreadItem, outputMaxBytes = config.agentCommandOutputMaxBytes) {
  switch (item.type) {
    case "command_execution":
      return {
        id: item.id,
        type: "command",
        name: "Shell command",
        command: item.command,
        ...boundedTail(item.aggregated_output, outputMaxBytes),
        exitCode: item.exit_code ?? null,
        status: item.status
      };
    case "mcp_tool_call":
      return { id: item.id, type: "mcp_tool_call", name: item.tool, server: item.server, input: item.arguments, status: item.status, error: item.error?.message ?? null };
    case "web_search":
      return { id: item.id, type: "web_search", name: "Web search", query: item.query };
    case "todo_list":
      return { id: item.id, type: "todo", name: "Task plan", items: item.items };
    default:
      return null;
  }
}

async function emitItem(event: Event, phase: "started" | "updated" | "completed", item: ThreadItem, outputMaxBytes?: number) {
  if (item.type === "agent_message") {
    if (phase === "completed") await event("assistant_delta", { delta: item.text });
    return;
  }
  if (item.type === "reasoning") {
    await event("reasoning", { id: item.id, phase, text: item.text });
    return;
  }
  if (item.type === "file_change") {
    await event("file_change", { id: item.id, phase, changes: item.changes, status: item.status });
    return;
  }
  if (item.type === "error") {
    await event("tool_result", { id: item.id, phase, name: "Codex", status: "failed", error: item.message, isError: true });
    return;
  }
  const payload = toolPayload(item, outputMaxBytes);
  if (!payload) return;
  if (phase === "started") await event("tool_call", { ...payload, phase });
  else if (phase === "updated") await event("tool_update", { ...payload, phase });
  else await event("tool_result", {
    ...payload,
    phase,
    isError: (item.type === "command_execution" && item.status === "failed") || (item.type === "mcp_tool_call" && item.status === "failed")
  });
}

export async function runCodexAccount(input: Input) {
  input.signal.throwIfAborted();
  const taskHome = await (input.prepareTaskHome ?? prepareCodexTaskHome)(input.taskId, input.threadId);
  input.signal.throwIfAborted();
  const localController = new AbortController();
  const signal = AbortSignal.any([input.signal, localController.signal]);
  const stop = async () => {
    if (!localController.signal.aborted) localController.abort(new Error("Codex run stopped."));
  };
  input.registerStop?.(stop);

  try {
    const options: CodexOptions = { env: codexEnvironment(taskHome) };
    const codex = input.createCodex?.(options) ?? new Codex(options);
    const threadOptions: ThreadOptions = {
      workingDirectory: input.worktreePath,
      sandboxMode: "danger-full-access",
      approvalPolicy: "never",
      ...(input.model && input.model !== "default" ? { model: input.model } : {})
    };
    const thread = input.threadId ? codex.resumeThread(input.threadId, threadOptions) : codex.startThread(threadOptions);
    const prompt = `Yanto task context: work only in the current Git worktree. Do not run Docker, switch branches, commit, push, or access unrelated host paths; Yanto owns those operations.\n\nUser request:\n${input.prompt}`;
    const streamed = await thread.runStreamed(prompt, { signal });
    const messages: string[] = [];
    const lastToolUpdateAt = new Map<string, number>();
    let threadId = input.threadId;
    for await (const event of streamed.events as AsyncGenerator<ThreadEvent>) {
      if (event.type === "thread.started") {
        threadId = event.thread_id;
        await input.event("codex_thread", { threadId });
      } else if (event.type === "item.started") {
        await emitItem(input.event, "started", event.item);
      } else if (event.type === "item.updated") {
        const now = Date.now();
        const lastEmittedAt = lastToolUpdateAt.get(event.item.id) ?? 0;
        if (now - lastEmittedAt >= CODEX_TOOL_UPDATE_INTERVAL_MS) {
          lastToolUpdateAt.set(event.item.id, now);
          await emitItem(input.event, "updated", event.item, CODEX_TOOL_UPDATE_MAX_BYTES);
        }
      } else if (event.type === "item.completed") {
        lastToolUpdateAt.delete(event.item.id);
        await emitItem(input.event, "completed", event.item);
        if (event.item.type === "agent_message") messages.push(event.item.text);
      } else if (event.type === "turn.failed") {
        throw new Error(event.error.message);
      } else if (event.type === "error") {
        throw new Error(event.message);
      }
    }
    signal.throwIfAborted();
    return { assistantText: messages.join("\n\n"), threadId };
  } catch (error) {
    if (signal.aborted) throw canceledError(input.signal.aborted ? input.signal : signal);
    throw error;
  }
}
