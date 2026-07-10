import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import fs from "node:fs/promises";
import path from "node:path";
import readline from "node:readline";
import { config } from "../config.js";
import { clearCodexTaskAuthentication } from "./codex-account-runner.js";

export type CodexLogin = { loginId: string; verificationUrl: string; userCode: string };
export type CodexAccountStatus = { connected: boolean; email: string | null; planType: string | null; login: CodexLogin | null };

type Pending = { resolve: (value: unknown) => void; reject: (error: Error) => void; timer: NodeJS.Timeout };

class CodexAppServer {
  private child: ChildProcessWithoutNullStreams | null = null;
  private pending = new Map<number, Pending>();
  private nextId = 1;
  private starting: Promise<void> | null = null;
  login: CodexLogin | null = null;

  private async start() {
    if (this.child) return;
    if (this.starting) return this.starting;
    this.starting = (async () => {
      await fs.mkdir(config.codexHome, { recursive: true, mode: 0o700 });
      const cli = path.resolve(process.cwd(), "node_modules/@openai/codex/bin/codex.js");
      const env: NodeJS.ProcessEnv = { ...process.env, CODEX_HOME: config.codexHome };
      delete env.OPENAI_API_KEY;
      delete env.CODEX_API_KEY;
      const child = spawn(process.execPath, [cli, "app-server", "--stdio"], { env, stdio: ["pipe", "pipe", "pipe"] });
      this.child = child;
      readline.createInterface({ input: child.stdout }).on("line", (line) => this.receive(line));
      child.stderr.on("data", () => undefined);
      child.once("exit", (_code, signal) => {
        if (this.child !== child) return;
        this.child = null;
        const error = new Error(`Codex authentication service stopped${signal ? ` (${signal})` : ""}.`);
        for (const request of this.pending.values()) { clearTimeout(request.timer); request.reject(error); }
        this.pending.clear();
      });
      child.once("error", (error) => {
        if (this.child === child) this.child = null;
        for (const request of this.pending.values()) { clearTimeout(request.timer); request.reject(error); }
        this.pending.clear();
      });
      await this.rawRequest("initialize", { clientInfo: { name: "yanto", title: "Yanto", version: "0.9.0" } });
    })().finally(() => { this.starting = null; });
    return this.starting;
  }

  private receive(line: string) {
    let message: { id?: number; method?: string; result?: unknown; error?: { message?: string } };
    try { message = JSON.parse(line) as typeof message; } catch { return; }
    if (typeof message.id === "number") {
      const request = this.pending.get(message.id);
      if (!request) return;
      this.pending.delete(message.id);
      clearTimeout(request.timer);
      if (message.error) request.reject(new Error(message.error.message || "Codex request failed."));
      else request.resolve(message.result);
      return;
    }
    if (message.method === "account/login/completed" || message.method === "account/updated") this.login = null;
  }

  private rawRequest(method: string, params?: unknown) {
    if (!this.child) return Promise.reject(new Error("Codex authentication service is unavailable."));
    const id = this.nextId++;
    return new Promise<unknown>((resolve, reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error(`Codex ${method} timed out.`)); }, 30_000);
      timer.unref();
      this.pending.set(id, { resolve, reject, timer });
      this.child!.stdin.write(`${JSON.stringify({ method, id, ...(params === undefined ? {} : { params }) })}\n`);
    });
  }

  async request<T>(method: string, params?: unknown): Promise<T> {
    await this.start();
    return this.rawRequest(method, params) as Promise<T>;
  }
}

const server = new CodexAppServer();

export async function startCodexLogin() {
  if (server.login) await server.request("account/login/cancel", { loginId: server.login.loginId }).catch(() => undefined);
  const result = await server.request<CodexLogin>("account/login/start", { type: "chatgptDeviceCode" });
  server.login = result;
  return result;
}

export async function cancelCodexLogin() {
  if (!server.login) return;
  await server.request("account/login/cancel", { loginId: server.login.loginId });
  server.login = null;
}

export async function getCodexAccountStatus(): Promise<CodexAccountStatus> {
  const result = await server.request<{ account: null | { type: string; email?: string | null; planType?: string } }>("account/read", { refreshToken: false });
  return { connected: Boolean(result.account), email: result.account?.email ?? null, planType: result.account?.planType ?? null, login: server.login };
}

export async function logoutCodexAccount() {
  await server.request("account/logout");
  await clearCodexTaskAuthentication();
  server.login = null;
}

export async function listCodexModels() {
  const models: Array<{ id: string; name: string }> = [];
  let cursor: string | null = null;
  do {
    const result: { data: Array<{ model: string; displayName: string; hidden: boolean }>; nextCursor: string | null } = await server.request("model/list", { cursor, limit: 100, includeHidden: false });
    for (const model of result.data) if (!model.hidden) models.push({ id: model.model, name: model.displayName || model.model });
    cursor = result.nextCursor;
  } while (cursor);
  return models;
}
