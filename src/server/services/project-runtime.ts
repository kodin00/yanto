import fs from "node:fs/promises";
import path from "node:path";
import type { ProjectRow } from "../db/schema.js";
import { runCommand } from "./commands.js";
import { autoStartOverrideFile } from "./compose.js";
import { pathExists } from "./paths.js";

async function composeArgs(project: ProjectRow) {
  const composeFile = project.composeFile || ((await pathExists(path.join(project.localPath, "compose.yml"))) ? "compose.yml" : "docker-compose.yml");
  await fs.access(path.join(project.localPath, composeFile));

  const args = ["compose", "-f", composeFile];
  const restartOverride = autoStartOverrideFile();
  if (await pathExists(path.join(project.localPath, restartOverride))) {
    args.push("-f", restartOverride);
  }
  return args;
}

async function runProjectCompose(project: ProjectRow, command: "stop" | "restart") {
  const args = [...(await composeArgs(project)), command];
  const result = await runCommand("docker", args, { cwd: project.localPath });
  if (result.exitCode !== 0) {
    throw new Error(result.output || `Unable to ${command} project compose services.`);
  }
  return result.output;
}

export async function stopProjectCompose(project: ProjectRow) {
  return runProjectCompose(project, "stop");
}

export async function restartProjectCompose(project: ProjectRow) {
  return runProjectCompose(project, "restart");
}
