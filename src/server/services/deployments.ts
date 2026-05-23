import { and, desc, eq, sql } from "drizzle-orm";
import fs from "node:fs/promises";
import path from "node:path";
import { db } from "../db/index.js";
import { deployments, projects, type DeploymentRow, type ProjectRow } from "../db/schema.js";
import { logger } from "../logger.js";
import { runCommand } from "./commands.js";
import { autoStartOverrideFile, buildAutoStartOverride } from "./compose.js";
import { pathExists } from "./paths.js";
import { createId } from "./tokens.js";
import { gitSshEnv, resolveGitPrivateKeyPath } from "./ssh.js";

const activeDeployments = new Map<string, DeploymentRow>();

function now() {
  return new Date();
}

async function appendDeploymentLog(deploymentId: string, chunk: string) {
  await db
    .update(deployments)
    .set({ logs: sql`${deployments.logs} || ${chunk}` })
    .where(eq(deployments.id, deploymentId));
}

async function runLogged(deploymentId: string, command: string, args: string[], cwd: string, env?: NodeJS.ProcessEnv) {
  await appendDeploymentLog(deploymentId, `$ ${command} ${args.join(" ")}\n`);
  const result = await runCommand(command, args, {
    cwd,
    env,
    onData: (chunk) => {
      void appendDeploymentLog(deploymentId, chunk);
    }
  });
  await appendDeploymentLog(deploymentId, `\n`);
  if (result.exitCode !== 0) {
    const tail = result.output.trim().split("\n").slice(-12).join("\n");
    throw new Error(`${command} ${args.join(" ")} failed with exit code ${result.exitCode}${tail ? `:\n${tail}` : ""}`);
  }
}

async function runLoggedOutput(deploymentId: string, command: string, args: string[], cwd: string, env?: NodeJS.ProcessEnv) {
  await appendDeploymentLog(deploymentId, `$ ${command} ${args.join(" ")}\n`);
  const result = await runCommand(command, args, {
    cwd,
    env,
    onData: (chunk) => {
      void appendDeploymentLog(deploymentId, chunk);
    }
  });
  await appendDeploymentLog(deploymentId, `\n`);
  if (result.exitCode !== 0) {
    const tail = result.output.trim().split("\n").slice(-12).join("\n");
    throw new Error(`${command} ${args.join(" ")} failed with exit code ${result.exitCode}${tail ? `:\n${tail}` : ""}`);
  }
  return result.output;
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

async function replaceComposeDeployment(deploymentId: string, composeArgs: string[], cwd: string) {
  const runningContainers = countLines(await runLoggedOutput(deploymentId, "docker", [...composeArgs, "ps", "-q", "--status", "running"], cwd));
  if (runningContainers > 0) {
    await appendDeploymentLog(
      deploymentId,
      `Found ${runningContainers} running compose container${runningContainers === 1 ? "" : "s"}; building before replacement to reduce downtime.\n`
    );
    await runLogged(deploymentId, "docker", [...composeArgs, "build"], cwd);
    await runLogged(deploymentId, "docker", [...composeArgs, "up", "-d", "--remove-orphans"], cwd);
    return;
  }

  await appendDeploymentLog(deploymentId, "No running compose containers found; starting with a fresh build.\n");
  await runLogged(deploymentId, "docker", [...composeArgs, "up", "-d", "--build", "--remove-orphans"], cwd);
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

async function runDeployment(project: ProjectRow, deployment: DeploymentRow) {
  const privateKeyPath = await resolveGitPrivateKeyPath();
  const env = gitSshEnv(privateKeyPath);
  try {
    await appendDeploymentLog(deployment.id, `Starting deployment for ${project.name}\n`);
    if (project.gitUrl) {
      await appendDeploymentLog(deployment.id, privateKeyPath ? `Using SSH key at ${privateKeyPath}\n` : "No SSH key found; Git SSH may fail.\n");
    }
    const exists = await pathExists(project.localPath);
    if (!exists && project.gitUrl) {
      await runLogged(deployment.id, "git", ["clone", "--branch", project.branch, project.gitUrl, project.localPath], process.cwd(), env);
    } else if (!exists) {
      await fs.mkdir(project.localPath, { recursive: true });
      await appendDeploymentLog(deployment.id, `Created compose project folder at ${project.localPath}.\n`);
    } else {
      await appendDeploymentLog(deployment.id, `Project folder exists at ${project.localPath}; leaving it in place.\n`);
    }

    const gitDirExists = await pathExists(`${project.localPath}/.git`);
    if (gitDirExists && project.gitUrl) {
      const targetRef = deployment.targetRef?.trim();
      if (targetRef) {
        await appendDeploymentLog(deployment.id, `Checking out deployment target ${targetRef}.\n`);
        await runLogged(deployment.id, "git", ["fetch", "--all", "--tags"], project.localPath, env);
        await runLogged(deployment.id, "git", ["checkout", targetRef], project.localPath, env);
      } else {
        await runLogged(deployment.id, "git", ["fetch", "origin", project.branch], project.localPath, env);
        await runLogged(deployment.id, "git", ["checkout", project.branch], project.localPath, env);
        await runLogged(deployment.id, "git", ["pull", "--ff-only", "origin", project.branch], project.localPath, env);
      }
    } else {
      await appendDeploymentLog(deployment.id, "Project folder is not a git repository; skipping git pull.\n");
    }

    if (gitDirExists) {
      const commitSha = await commandOutput("git", ["rev-parse", "HEAD"], project.localPath, env);
      const commitMessage = await commandOutput("git", ["log", "-1", "--pretty=%s"], project.localPath, env).catch(() => "");
      await db.update(deployments).set({ commitSha, commitMessage, targetRef: deployment.targetRef ?? commitSha }).where(eq(deployments.id, deployment.id));
      deployment.commitSha = commitSha;
      await appendDeploymentLog(deployment.id, `Using commit ${commitSha}${commitMessage ? ` (${commitMessage})` : ""}.\n`);
    }

    const composeFile = await detectComposeFile(project);
    if (project.composeContent?.trim()) {
      const composePath = path.join(project.localPath, composeFile);
      await fs.mkdir(path.dirname(composePath), { recursive: true });
      await fs.writeFile(composePath, project.composeContent, "utf8");
      await appendDeploymentLog(deployment.id, `Wrote compose file ${composeFile} from saved editor content.\n`);
    }
    await fs.access(`${project.localPath}/${composeFile}`);

    const composeArgs = ["compose", "-f", composeFile];
    const restartOverride = autoStartOverrideFile();
    const restartOverridePath = path.join(project.localPath, restartOverride);
    if (project.autoStart) {
      const composeContent = await fs.readFile(path.join(project.localPath, composeFile), "utf8");
      await fs.writeFile(restartOverridePath, buildAutoStartOverride(composeContent), "utf8");
      await appendDeploymentLog(deployment.id, "Auto start is enabled; applying restart: unless-stopped override.\n");
      composeArgs.push("-f", restartOverride);
    } else {
      await fs.rm(restartOverridePath, { force: true });
      await appendDeploymentLog(deployment.id, "Auto start is disabled; running compose without restart override.\n");
    }
    await replaceComposeDeployment(deployment.id, composeArgs, project.localPath);

    await db
      .update(deployments)
      .set({ status: "success", exitCode: 0, finishedAt: now() })
      .where(eq(deployments.id, deployment.id));
    logger.info("deployment succeeded", { projectId: project.id, deploymentId: deployment.id });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    await appendDeploymentLog(deployment.id, `Deployment failed: ${message}\n`);
    await db
      .update(deployments)
      .set({ status: "failed", exitCode: 1, finishedAt: now() })
      .where(eq(deployments.id, deployment.id));
    logger.error("deployment failed", { projectId: project.id, deploymentId: deployment.id, error: message });
  } finally {
    activeDeployments.delete(project.id);
  }
}

export type StartDeploymentOptions = {
  targetRef?: string;
  rollbackFromDeploymentId?: string;
};

export async function startDeployment(projectId: string, trigger: "manual" | "webhook" | "rollback", options: StartDeploymentOptions = {}) {
  const active = activeDeployments.get(projectId);
  if (active) {
    return { deployment: active, reused: true };
  }

  const [project] = await db.select().from(projects).where(eq(projects.id, projectId)).limit(1);
  if (!project) {
    throw new Error("Project not found.");
  }

  const [deployment] = await db
    .insert(deployments)
    .values({
      id: createId("dep"),
      projectId,
      status: "running",
      trigger,
      targetRef: options.targetRef?.trim() || null,
      rollbackFromDeploymentId: options.rollbackFromDeploymentId?.trim() || null,
      logs: "",
      startedAt: now()
    })
    .returning();

  activeDeployments.set(projectId, deployment);
  void runDeployment(project, deployment);
  return { deployment, reused: false };
}

export async function latestDeployments(limit = 20) {
  return db
    .select({
      id: deployments.id,
      projectId: deployments.projectId,
      projectName: projects.name,
      status: deployments.status,
      trigger: deployments.trigger,
      targetRef: deployments.targetRef,
      commitSha: deployments.commitSha,
      commitMessage: deployments.commitMessage,
      rollbackFromDeploymentId: deployments.rollbackFromDeploymentId,
      logs: deployments.logs,
      exitCode: deployments.exitCode,
      startedAt: deployments.startedAt,
      finishedAt: deployments.finishedAt
    })
    .from(deployments)
    .leftJoin(projects, eq(projects.id, deployments.projectId))
    .orderBy(desc(deployments.startedAt))
    .limit(limit);
}

export async function findDeployment(id: string) {
  const [deployment] = await db.select().from(deployments).where(eq(deployments.id, id)).limit(1);
  return deployment;
}

export async function rollbackTargetForProject(projectId: string, deploymentId?: string, targetRef?: string) {
  if (targetRef?.trim()) {
    return { targetRef: targetRef.trim(), rollbackFromDeploymentId: deploymentId ?? null };
  }

  if (deploymentId) {
    const [deployment] = await db.select().from(deployments).where(and(eq(deployments.id, deploymentId), eq(deployments.projectId, projectId))).limit(1);
    const ref = deployment?.commitSha ?? deployment?.targetRef;
    if (!ref) {
      throw new Error("Selected deployment does not have a recorded Git commit.");
    }
    return { targetRef: ref, rollbackFromDeploymentId: deployment.id };
  }

  const rows = await db
    .select()
    .from(deployments)
    .where(and(eq(deployments.projectId, projectId), eq(deployments.status, "success")))
    .orderBy(desc(deployments.startedAt))
    .limit(20);
  const distinct = rows.filter((deployment, index) => {
    const ref = deployment.commitSha ?? deployment.targetRef;
    return ref && rows.findIndex((candidate) => (candidate.commitSha ?? candidate.targetRef) === ref) === index;
  });
  const target = distinct[1];
  const ref = target?.commitSha ?? target?.targetRef;
  if (!target || !ref) {
    throw new Error("No previous successful Git deployment is available to roll back to.");
  }
  return { targetRef: ref, rollbackFromDeploymentId: target.id };
}

export async function activeDeploymentFor(projectId: string) {
  const [deployment] = await db
    .select()
    .from(deployments)
    .where(and(eq(deployments.projectId, projectId), eq(deployments.status, "running")))
    .limit(1);
  return deployment;
}
