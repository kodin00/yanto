import { spawn } from "node:child_process";
import fs from "node:fs/promises";
import readline from "node:readline";
import { config } from "../config.js";
import { resolveHostMountPath } from "./agent-tools.js";

type Event = (kind: string, payload: Record<string, unknown>) => Promise<void>;
type Input = { runId: string; worktreePath: string; prompt: string; model: string; threadId: string | null; signal: AbortSignal; event: Event };

export async function runCodexAccount(input: Input) {
  await fs.mkdir(config.codexHome, { recursive: true, mode: 0o700 });
  const [worktreeHost, codexHost] = await Promise.all([resolveHostMountPath(input.worktreePath), resolveHostMountPath(config.codexHome)]);
  const name = `yanto-codex-${input.runId.replace(/[^a-zA-Z0-9_.-]/g, "-").slice(0, 48)}`;
  const child = spawn("docker", [
    "run", "-i", "--rm", "--name", name, "--workdir", "/workspace", "--network", "bridge",
    "--memory", "4g", "--cpus", "2", "--pids-limit", "512", "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
    "-e", "CODEX_HOME=/data/codex", "-v", `${worktreeHost}:/workspace`, "-v", `${codexHost}:/data/codex`,
    "--entrypoint", "node", config.agentDefaultImage, "/app/dist/server/server/services/codex-runner.js"
  ], { stdio: ["pipe", "pipe", "pipe"] });
  const stop = () => { spawn("docker", ["rm", "-f", name], { stdio: "ignore" }); };
  input.signal.addEventListener("abort", stop, { once: true });
  child.stdin.end(JSON.stringify({ prompt: input.prompt, model: input.model, threadId: input.threadId }));
  const exitPromise = new Promise<number | null>((resolve, reject) => { child.once("exit", resolve); child.once("error", reject); });
  let stderr = "";
  child.stderr.on("data", (chunk) => { stderr = `${stderr}${String(chunk)}`.slice(-32_000); });
  let result: { success?: boolean; assistantText?: string; threadId?: string | null; error?: string } | null = null;
  const lines = readline.createInterface({ input: child.stdout });
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
  const exitCode = await exitPromise;
  input.signal.removeEventListener("abort", stop);
  if (input.signal.aborted) throw input.signal.reason instanceof Error ? input.signal.reason : new Error("Codex run canceled.");
  if (!result?.success || exitCode !== 0) throw new Error(String(result?.error || stderr.trim() || `Codex runner exited with ${exitCode}.`));
  return { assistantText: String(result.assistantText ?? ""), threadId: typeof result.threadId === "string" ? result.threadId : input.threadId };
}
