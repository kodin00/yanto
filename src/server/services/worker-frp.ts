import { spawn, type ChildProcess } from "node:child_process";
import fs from "node:fs/promises";
import path from "node:path";
import { config } from "../config.js";
import { logger } from "../logger.js";
import { runCommand } from "./commands.js";

export type WorkerFrpConfig = {
  configured: boolean;
  nodeId: string;
  serverAddr: string;
  serverPort: number;
  wireProtocol: "v2";
  revision: string;
  authToken: string;
  tunnels: {
    id: string;
    name: string;
    protocol: string;
    localHost: string;
    localPort: number;
    remotePort: number;
  }[];
};

export type WorkerFrpStatus = {
  appliedRevision: string | null;
  processStatus: "running" | "stopped" | "error";
  frpcVersion: string | null;
  lastError: string | null;
};

export function buildFrpcConfig(input: WorkerFrpConfig) {
  return {
    clientID: input.nodeId,
    user: input.nodeId,
    serverAddr: input.serverAddr,
    serverPort: input.serverPort,
    loginFailExit: false,
    auth: {
      method: "token",
      token: input.authToken,
      additionalScopes: ["HeartBeats", "NewWorkConns"]
    },
    transport: {
      wireProtocol: input.wireProtocol,
      tls: { enable: true }
    },
    log: {
      to: "console",
      level: "info",
      disablePrintColor: true
    },
    proxies: input.tunnels.map((tunnel) => ({
      name: tunnel.id,
      type: tunnel.protocol,
      localIP: tunnel.localHost,
      localPort: tunnel.localPort,
      remotePort: tunnel.remotePort
    }))
  };
}

function waitForExit(child: ChildProcess, timeoutMs: number) {
  if (child.exitCode !== null || child.signalCode !== null) return Promise.resolve();
  return new Promise<void>((resolve) => {
    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      resolve();
    }, timeoutMs);
    child.once("exit", () => {
      clearTimeout(timer);
      resolve();
    });
  });
}

export class WorkerFrpManager {
  private child: ChildProcess | null = null;
  private appliedRevision: string | null = null;
  private frpcVersion: string | null = null;
  private lastError: string | null = null;
  private logTail = "";

  private async version() {
    if (this.frpcVersion) return this.frpcVersion;
    const result = await runCommand("frpc", ["--version"], { timeoutMs: 5000 });
    this.frpcVersion = result.exitCode === 0 ? result.output.trim() : null;
    return this.frpcVersion;
  }

  private status(processStatus?: WorkerFrpStatus["processStatus"]): WorkerFrpStatus {
    const running = Boolean(this.child && this.child.exitCode === null && this.child.signalCode === null);
    return {
      appliedRevision: this.appliedRevision,
      processStatus: processStatus ?? (running ? "running" : this.lastError ? "error" : "stopped"),
      frpcVersion: this.frpcVersion,
      lastError: this.lastError
    };
  }

  private capture(chunk: Buffer) {
    this.logTail = `${this.logTail}${chunk.toString("utf8")}`.slice(-4000);
  }

  private async stopChild() {
    const child = this.child;
    this.child = null;
    if (!child || child.exitCode !== null || child.signalCode !== null) return;
    child.kill("SIGTERM");
    await waitForExit(child, 5000);
  }

  private startChild(configPath: string) {
    const child = spawn("frpc", ["-c", configPath], { stdio: ["ignore", "pipe", "pipe"] });
    this.child = child;
    this.logTail = "";
    child.stdout.on("data", (chunk: Buffer) => this.capture(chunk));
    child.stderr.on("data", (chunk: Buffer) => this.capture(chunk));
    child.once("exit", (code, signal) => {
      if (this.child === child) {
        this.child = null;
        if (code !== 0) {
          this.lastError = `frpc exited with ${signal ?? `code ${code ?? 1}`}.${this.logTail ? ` ${this.logTail.trim().slice(-1000)}` : ""}`;
          logger.error("frpc process exited", { code, signal, error: this.lastError });
        }
      }
    });
  }

  async reconcile(input: WorkerFrpConfig): Promise<WorkerFrpStatus> {
    await this.version();
    if (!input.configured || !input.serverAddr || !input.authToken) {
      await this.stopChild();
      this.appliedRevision = null;
      this.lastError = input.configured ? "FRP authentication token is unavailable." : null;
      return this.status(this.lastError ? "error" : "stopped");
    }

    const running = Boolean(this.child && this.child.exitCode === null && this.child.signalCode === null);
    if (running && this.appliedRevision === input.revision) return this.status("running");

    await fs.mkdir(config.frpDataDir, { recursive: true, mode: 0o700 });
    const configPath = path.join(config.frpDataDir, "frpc.json");
    const temporaryPath = `${configPath}.tmp`;
    await fs.writeFile(temporaryPath, `${JSON.stringify(buildFrpcConfig(input), null, 2)}\n`, { mode: 0o600 });
    const verification = await runCommand("frpc", ["verify", "-c", temporaryPath], { timeoutMs: 10_000 });
    if (verification.exitCode !== 0) {
      await fs.rm(temporaryPath, { force: true });
      this.lastError = verification.output.trim() || "FRP client configuration validation failed.";
      return this.status("error");
    }

    await fs.rename(temporaryPath, configPath);
    await fs.chmod(configPath, 0o600);
    await this.stopChild();
    this.lastError = null;
    this.appliedRevision = input.revision;
    this.startChild(configPath);
    await new Promise((resolve) => setTimeout(resolve, 250));
    if (!this.child || this.child.exitCode !== null) {
      return this.status("error");
    }
    return this.status("running");
  }

  async shutdown() {
    await this.stopChild();
  }
}
