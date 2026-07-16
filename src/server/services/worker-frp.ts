import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { config } from "../config.js";
import { runCommand } from "./commands.js";

export type WorkerFrpDesiredComponent = {
  containerName: string;
  configPath: string;
  configToml: string;
};

export type WorkerFrpDesiredConfig = {
  nodeId: string;
  role: "disabled" | "client" | "server" | "both";
  revision: number;
  frpc: WorkerFrpDesiredComponent | null;
  frps: WorkerFrpDesiredComponent | null;
};

async function removeContainer(name: string) {
  await runCommand("docker", ["rm", "-f", name], { timeoutMs: 30_000, maxOutputBytes: 64 * 1024 });
}

async function currentImage() {
  const result = await runCommand("docker", ["inspect", "--format", "{{.Config.Image}}", os.hostname()], { timeoutMs: 10_000 });
  if (result.exitCode !== 0 || !result.output.trim()) throw new Error("Unable to determine the Yanto worker image for FRP.");
  return result.output.trim();
}

async function applyComponent(component: "frpc" | "frps", desired: WorkerFrpDesiredComponent | null, image: string) {
  const containerName = desired?.containerName ?? `yanto-${component}`;
  if (!desired) {
    await removeContainer(containerName);
    return;
  }
  const configPath = path.resolve(desired.configPath);
  if (!configPath.startsWith(`${config.frpDataDir}${path.sep}`)) throw new Error("FRP config path escapes the worker FRP directory.");
  await fs.mkdir(path.dirname(configPath), { recursive: true, mode: 0o700 });
  const temporaryPath = `${configPath}.tmp`;
  await fs.writeFile(temporaryPath, desired.configToml, { mode: 0o600 });
  const verification = await runCommand(component, ["verify", "-c", temporaryPath], { timeoutMs: 15_000, maxOutputBytes: 64 * 1024 });
  if (verification.exitCode !== 0) {
    await fs.rm(temporaryPath, { force: true });
    throw new Error(verification.output.trim() || `${component} configuration is invalid.`);
  }
  await fs.rename(temporaryPath, configPath);
  await removeContainer(containerName);
  const started = await runCommand("docker", [
    "run", "-d", "--name", containerName,
    "--restart", "unless-stopped",
    "--network", "host",
    "--volumes-from", os.hostname(),
    "--entrypoint", component,
    image,
    "-c", configPath
  ], { timeoutMs: 60_000, maxOutputBytes: 64 * 1024 });
  if (started.exitCode !== 0) throw new Error(started.output.trim() || `Unable to start ${component}.`);
}

export async function reconcileWorkerFrp(desired: WorkerFrpDesiredConfig) {
  const revisionPath = path.join(config.frpDataDir, "applied-revision");
  const appliedRevision = Number(await fs.readFile(revisionPath, "utf8").catch(() => "-1"));
  if (appliedRevision === desired.revision) return { changed: false, status: desired.role === "disabled" ? "disabled" as const : "online" as const };
  const image = await currentImage();
  await applyComponent("frpc", desired.frpc, image);
  await applyComponent("frps", desired.frps, image);
  await fs.mkdir(config.frpDataDir, { recursive: true, mode: 0o700 });
  await fs.writeFile(revisionPath, `${desired.revision}\n`, { mode: 0o600 });
  return { changed: true, status: desired.role === "disabled" ? "disabled" as const : "online" as const };
}
