import { spawn, type ChildProcess } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";
import readline from "node:readline";
import { config } from "../config.js";
import { resolveHostMountPath } from "./agent-tools.js";
import { ensureCodexSandboxReady } from "./codex-sandbox-probe.js";
import { codexDockerCreateArgs } from "./codex-sandbox.js";

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
  ensureSandbox?: () => Promise<void>;
  prepareTaskHome?: (taskId: string, threadId: string | null) => Promise<string>;
};

function canceledError(signal: AbortSignal) {
  return signal.reason instanceof Error ? signal.reason : new Error("Codex run canceled.");
}

function childExit(child: ChildProcess) {
  return new Promise<number | null>((resolve, reject) => {
    child.once("exit", resolve);
    child.once("error", reject);
  });
}

function containerName(runId: string) {
  return `yanto-codex-${runId.replace(/[^a-zA-Z0-9_.-]/g, "-").slice(0, 48)}`;
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

export async function runCodexAccount(input: Input) {
  input.signal.throwIfAborted();
  await (input.ensureSandbox ?? ensureCodexSandboxReady)();
  input.signal.throwIfAborted();
  const taskHome = await (input.prepareTaskHome ?? prepareCodexTaskHome)(input.taskId, input.threadId);
  input.signal.throwIfAborted();
  const [worktreeHost, codexHost] = await Promise.all([
    resolveHostMountPath(input.worktreePath),
    resolveHostMountPath(taskHome)
  ]);
  input.signal.throwIfAborted();

  const name = containerName(input.runId);
  let dockerChild: ChildProcess | undefined;
  let dockerExit: Promise<number | null> | undefined;
  let dockerPhase: "create" | "start" | undefined;
  let lines: readline.Interface | undefined;
  let createAttempted = false;
  let cleanupPromise: Promise<void> | undefined;

  const removeContainer = async () => {
    const remover = spawn("docker", ["rm", "-f", name], { stdio: ["ignore", "pipe", "pipe"] });
    let output = "";
    remover.stdout!.on("data", (chunk) => { output = `${output}${String(chunk)}`.slice(-32_000); });
    remover.stderr!.on("data", (chunk) => { output = `${output}${String(chunk)}`.slice(-32_000); });
    const exitCode = await childExit(remover);
    if (exitCode !== 0 && !output.toLowerCase().includes("no such container")) {
      throw new Error(output.trim() || `Unable to remove Codex runner container ${name}.`);
    }
  };

  const cleanup = () => cleanupPromise ??= (async () => {
    input.signal.removeEventListener("abort", onAbort);
    lines?.close();
    const child = dockerChild;
    const exit = dockerExit;
    const phase = dockerPhase;
    child?.stdin?.destroy();
    child?.stdout?.destroy();
    child?.stderr?.destroy();
    child?.kill("SIGTERM");

    if (phase === "create") await exit?.catch(() => undefined);
    const removal = createAttempted ? removeContainer() : Promise.resolve();
    if (phase === "start") await exit?.catch(() => undefined);
    await removal;
  })();
  const onAbort = () => { void cleanup().catch(() => undefined); };

  input.registerStop?.(cleanup);
  input.signal.addEventListener("abort", onAbort, { once: true });
  input.signal.throwIfAborted();

  try {
    createAttempted = true;
    dockerPhase = "create";
    dockerChild = spawn("docker", codexDockerCreateArgs(config.agentDefaultImage, {
      name,
      workspaceHost: worktreeHost,
      codexHomeHost: codexHost,
      labels: [
        "com.yanto.agent=true",
        `com.yanto.agent.run-id=${input.runId}`,
        `com.yanto.agent.task-id=${input.taskId}`
      ],
      entrypoint: "node",
      command: ["/app/dist/server/server/services/codex-runner.js"]
    }), { stdio: ["pipe", "pipe", "pipe"] });
    dockerExit = childExit(dockerChild);
    let createOutput = "";
    dockerChild.stdout!.on("data", (chunk) => { createOutput = `${createOutput}${String(chunk)}`.slice(-32_000); });
    dockerChild.stderr!.on("data", (chunk) => { createOutput = `${createOutput}${String(chunk)}`.slice(-32_000); });
    const createExitCode = await dockerExit;
    dockerChild = undefined;
    dockerExit = undefined;
    dockerPhase = undefined;
    if (createExitCode !== 0) throw new Error(createOutput.trim() || `Unable to create Codex runner container ${name}.`);

    input.signal.throwIfAborted();
    dockerPhase = "start";
    dockerChild = spawn("docker", ["start", "-a", "-i", name], { stdio: ["pipe", "pipe", "pipe"] });
    dockerExit = childExit(dockerChild);
    dockerChild.stdin!.end(JSON.stringify({ prompt: input.prompt, model: input.model, threadId: input.threadId }));
    let stderr = "";
    dockerChild.stderr!.on("data", (chunk) => { stderr = `${stderr}${String(chunk)}`.slice(-32_000); });
    let result: { success?: boolean; assistantText?: string; threadId?: string | null; error?: string } | null = null;
    lines = readline.createInterface({ input: dockerChild.stdout! });
    for await (const line of lines) {
      let message: Record<string, unknown>;
      try { message = JSON.parse(line) as Record<string, unknown>; } catch { continue; }
      switch (message.type) {
        case "thread": await input.event("codex_thread", { threadId: message.threadId }); break;
        case "assistant": await input.event("assistant_delta", { delta: String(message.text ?? "") }); break;
        case "reasoning": await input.event("reasoning", { text: message.text }); break;
        case "command": await input.event("tool_result", message); break;
        case "file_change": await input.event("file_change", message); break;
        case "tool": case "web_search": case "todo": await input.event("tool_call", message); break;
        case "result": result = message; break;
      }
    }
    const exitCode = await dockerExit;
    dockerChild = undefined;
    dockerExit = undefined;
    dockerPhase = undefined;
    input.signal.throwIfAborted();
    if (!result?.success || exitCode !== 0) throw new Error(String(result?.error || stderr.trim() || `Codex runner exited with ${exitCode}.`));
    return { assistantText: String(result.assistantText ?? ""), threadId: typeof result.threadId === "string" ? result.threadId : input.threadId };
  } catch (error) {
    if (input.signal.aborted) throw canceledError(input.signal);
    throw error;
  } finally {
    await cleanup();
  }
}
