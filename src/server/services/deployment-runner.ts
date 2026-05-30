import fs from "node:fs/promises";
import path from "node:path";
import type { DeploymentRow, ProjectRow } from "../db/schema.js";
import { runCommand } from "./commands.js";
import { autoStartOverrideFile, buildAutoStartOverride } from "./compose.js";
import { pathExists } from "./paths.js";
import { gitSshEnv, resolveGitPrivateKeyPath } from "./ssh.js";

export type DeploymentMetadata = {
  commitSha?: string | null;
  commitMessage?: string | null;
  targetRef?: string | null;
};

export type DeploymentRunnerCallbacks = {
  appendLog: (chunk: string) => Promise<void>;
  updateMetadata?: (metadata: DeploymentMetadata) => Promise<void>;
};

async function runLogged(callbacks: DeploymentRunnerCallbacks, command: string, args: string[], cwd: string, env?: NodeJS.ProcessEnv): Promise<void>;
async function runLogged(callbacks: DeploymentRunnerCallbacks, command: string, args: string[], cwd: string, env: NodeJS.ProcessEnv | undefined, returnOutput: true): Promise<string>;
async function runLogged(callbacks: DeploymentRunnerCallbacks, command: string, args: string[], cwd: string, env?: NodeJS.ProcessEnv, returnOutput?: boolean): Promise<string | void> {
  await callbacks.appendLog(`$ ${command} ${args.join(" ")}\n`);
  const result = await runCommand(command, args, {
    cwd,
    env,
    onData: (chunk) => {
      void callbacks.appendLog(chunk);
    }
  });
  await callbacks.appendLog("\n");
  if (result.exitCode !== 0) {
    const tail = result.output.trim().split("\n").slice(-12).join("\n");
    throw new Error(`${command} ${args.join(" ")} failed with exit code ${result.exitCode}${tail ? `:\n${tail}` : ""}`);
  }
  if (returnOutput) return result.output;
}

async function commandOutput(command: string, args: string[], cwd: string, env?: NodeJS.ProcessEnv) {
  const result = await runCommand(command, args, { cwd, env });
  if (result.exitCode !== 0) {
    throw new Error(result.output || `${command} ${args.join(" ")} failed.`);
  }
  return result.output.trim();
}

function countLines(output: string) {
  return output
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean).length;
}

async function replaceComposeDeployment(callbacks: DeploymentRunnerCallbacks, composeArgs: string[], cwd: string) {
  const runningContainers = countLines(await runLogged(callbacks, "docker", [...composeArgs, "ps", "-q", "--status", "running"], cwd, undefined, true));
  if (runningContainers > 0) {
    await callbacks.appendLog(`Found ${runningContainers} running compose container${runningContainers === 1 ? "" : "s"}; building before replacement to reduce downtime.\n`);
    await runLogged(callbacks, "docker", [...composeArgs, "build"], cwd);
    await runLogged(callbacks, "docker", [...composeArgs, "up", "-d", "--remove-orphans"], cwd);
    return;
  }

  await callbacks.appendLog("No running compose containers found; starting with a fresh build.\n");
  await runLogged(callbacks, "docker", [...composeArgs, "up", "-d", "--build", "--remove-orphans"], cwd);
}

async function detectComposeFile(project: ProjectRow) {
  if (project.composeFile) {
    return project.composeFile;
  }
  if (await pathExists(`${project.localPath}/compose.yml`)) {
    return "compose.yml";
  }
  return "docker-compose.yml";
}

export async function runProjectDeployment(project: ProjectRow, deployment: DeploymentRow, callbacks: DeploymentRunnerCallbacks) {
  const privateKeyPath = await resolveGitPrivateKeyPath();
  const env = gitSshEnv(privateKeyPath);
  await callbacks.appendLog(`Starting deployment for ${project.name}\n`);
  if (project.gitUrl) {
    await callbacks.appendLog(privateKeyPath ? `Using SSH key at ${privateKeyPath}\n` : "No SSH key found; Git SSH may fail.\n");
  }
  const exists = await pathExists(project.localPath);
  if (!exists && project.gitUrl) {
    await runLogged(callbacks, "git", ["clone", "--branch", project.branch, project.gitUrl, project.localPath], process.cwd(), env);
  } else if (!exists) {
    await fs.mkdir(project.localPath, { recursive: true });
    await callbacks.appendLog(`Created compose project folder at ${project.localPath}.\n`);
  } else {
    await callbacks.appendLog(`Project folder exists at ${project.localPath}; leaving it in place.\n`);
  }

  const gitDirExists = await pathExists(`${project.localPath}/.git`);
  if (gitDirExists && project.gitUrl) {
    const targetRef = deployment.targetRef?.trim();
    if (targetRef) {
      await callbacks.appendLog(`Checking out deployment target ${targetRef}.\n`);
      await runLogged(callbacks, "git", ["fetch", "--all", "--tags"], project.localPath, env);
      await runLogged(callbacks, "git", ["checkout", targetRef], project.localPath, env);
    } else {
      await runLogged(callbacks, "git", ["fetch", "origin", project.branch], project.localPath, env);
      await runLogged(callbacks, "git", ["checkout", project.branch], project.localPath, env);
      await runLogged(callbacks, "git", ["pull", "--ff-only", "origin", project.branch], project.localPath, env);
    }
  } else {
    await callbacks.appendLog("Project folder is not a git repository; skipping git pull.\n");
  }

  if (gitDirExists) {
    const commitSha = await commandOutput("git", ["rev-parse", "HEAD"], project.localPath, env);
    const commitMessage = await commandOutput("git", ["log", "-1", "--pretty=%s"], project.localPath, env).catch(() => "");
    await callbacks.updateMetadata?.({ commitSha, commitMessage, targetRef: deployment.targetRef ?? commitSha });
    await callbacks.appendLog(`Using commit ${commitSha}${commitMessage ? ` (${commitMessage})` : ""}.\n`);
  }

  const composeFile = await detectComposeFile(project);
  if (project.composeContent?.trim()) {
    const composePath = path.join(project.localPath, composeFile);
    await fs.mkdir(path.dirname(composePath), { recursive: true });
    await fs.writeFile(composePath, project.composeContent, "utf8");
    await callbacks.appendLog(`Wrote compose file ${composeFile} from saved editor content.\n`);
  }
  await fs.access(`${project.localPath}/${composeFile}`);

  const composeArgs = ["compose", "-f", composeFile];
  const restartOverride = autoStartOverrideFile();
  const restartOverridePath = path.join(project.localPath, restartOverride);
  if (project.autoStart) {
    const composeContent = await fs.readFile(path.join(project.localPath, composeFile), "utf8");
    await fs.writeFile(restartOverridePath, buildAutoStartOverride(composeContent), "utf8");
    await callbacks.appendLog("Auto start is enabled; applying restart: unless-stopped override.\n");
    composeArgs.push("-f", restartOverride);
  } else {
    await fs.rm(restartOverridePath, { force: true });
    await callbacks.appendLog("Auto start is disabled; running compose without restart override.\n");
  }
  await replaceComposeDeployment(callbacks, composeArgs, project.localPath);
}
