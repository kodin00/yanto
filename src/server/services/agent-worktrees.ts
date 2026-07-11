import fs from "node:fs/promises";
import path from "node:path";
import type { AgentTaskRow, ProjectRow } from "../db/schema.js";
import type { AgentGitFile, AgentGitPreview, ProjectBranch } from "../../shared/types.js";
import { config } from "../config.js";
import { HttpError } from "../http-utils.js";
import { runCommand } from "./commands.js";
import { pathExists } from "./paths.js";
import { gitSshEnv, resolveGitPrivateKeyPath } from "./ssh.js";

const refPattern = /^(?!-)(?!.*\.\.)(?!.*@\{)(?!.*[~^:?*\\\s])[^/]+(?:\/[^/]+)*$/;

function validateRef(ref: string, label: string) {
  const value = ref.trim();
  if (!value || !refPattern.test(value) || value.includes("[") || value.endsWith(".") || value.endsWith("/") || value.includes("//")) {
    throw new HttpError(400, `${label} is not a valid Git branch name.`);
  }
  return value;
}

async function git(project: ProjectRow, args: string[], options: { cwd?: string; maxOutputBytes?: number } = {}) {
  const privateKeyPath = await resolveGitPrivateKeyPath();
  const result = await runCommand("git", args, {
    cwd: options.cwd ?? project.localPath,
    env: gitSshEnv(privateKeyPath),
    timeoutMs: 5 * 60 * 1000,
    maxOutputBytes: options.maxOutputBytes ?? 2 * 1024 * 1024
  });
  if (result.exitCode !== 0) throw new HttpError(400, result.output.trim() || `git ${args.join(" ")} failed.`);
  return result.output.trim();
}

export async function ensureAgentRepository(project: ProjectRow) {
  if (!project.gitUrl) throw new HttpError(400, "AI tasks require a Git-backed project.");
  if (!await pathExists(project.localPath)) {
    await fs.mkdir(path.dirname(project.localPath), { recursive: true });
    await git(project, ["clone", project.gitUrl, project.localPath], { cwd: process.cwd() });
  }
  const result = await runCommand("git", ["rev-parse", "--git-dir"], { cwd: project.localPath, maxOutputBytes: 64 * 1024 });
  if (result.exitCode !== 0) throw new HttpError(400, "Project path is not a Git repository.");
}

export async function fetchProjectBranches(project: ProjectRow): Promise<ProjectBranch[]> {
  await ensureAgentRepository(project);
  await git(project, ["fetch", "origin", "--prune"]);
  const output = await git(project, ["for-each-ref", "--format=%(refname:short)%09%(objectname)", "refs/remotes/origin", "refs/heads"]);
  const byName = new Map<string, ProjectBranch>();
  for (const line of output.split("\n")) {
    if (!line.trim()) continue;
    const [rawName, sha] = line.split("\t");
    const remote = rawName.startsWith("origin/");
    const name = remote ? rawName.slice("origin/".length) : rawName;
    if (!name || name === "HEAD") continue;
    const current = byName.get(name);
    if (!current || remote) byName.set(name, { name, sha, remote });
  }
  return [...byName.values()].sort((a, b) => a.name.localeCompare(b.name));
}

export function taskWorktreePath(project: ProjectRow, task: AgentTaskRow) {
  return path.join(config.agentWorktreesRoot, project.folderName, task.id);
}

async function refExists(project: ProjectRow, ref: string) {
  const result = await runCommand("git", ["show-ref", "--verify", "--quiet", ref], { cwd: project.localPath, maxOutputBytes: 32 * 1024 });
  return result.exitCode === 0;
}

async function isAncestor(project: ProjectRow, ancestor: string, descendant: string) {
  const result = await runCommand("git", ["merge-base", "--is-ancestor", ancestor, descendant], {
    cwd: project.localPath,
    maxOutputBytes: 32 * 1024
  });
  if (result.exitCode !== 0 && result.exitCode !== 1) {
    throw new HttpError(400, result.output.trim() || `Unable to compare ${ancestor} with ${descendant}.`);
  }
  return result.exitCode === 0;
}

export async function prepareTaskWorktree(project: ProjectRow, task: AgentTaskRow) {
  await ensureAgentRepository(project);
  const sourceBranch = validateRef(task.sourceBranch, "Source branch");
  const taskBranch = validateRef(task.taskBranch, "Task branch");
  await git(project, ["fetch", "origin", "--prune"]);
  const sourceRef = await refExists(project, `refs/remotes/origin/${sourceBranch}`) ? `origin/${sourceBranch}` : sourceBranch;
  const sourceSha = await git(project, ["rev-parse", "--verify", `${sourceRef}^{commit}`]);
  const worktreePath = task.worktreePath || taskWorktreePath(project, task);

  if (await pathExists(worktreePath)) {
    const currentBranch = await git(project, ["-C", worktreePath, "branch", "--show-current"]);
    if (currentBranch !== taskBranch) throw new HttpError(409, `Existing task worktree is on branch ${currentBranch || "detached HEAD"}.`);
    return { worktreePath, sourceSha };
  }

  await fs.mkdir(path.dirname(worktreePath), { recursive: true });
  const localExists = await refExists(project, `refs/heads/${taskBranch}`);
  const remoteExists = await refExists(project, `refs/remotes/origin/${taskBranch}`);
  if (sourceBranch === taskBranch) {
    const primaryBranch = await git(project, ["branch", "--show-current"]);
    if (primaryBranch === taskBranch) {
      const primaryChanges = await git(project, ["status", "--porcelain=v1"]);
      if (primaryChanges) {
        throw new HttpError(409, `Cannot use source branch ${taskBranch} while the primary checkout has uncommitted changes.`);
      }
    }
    if (localExists) {
      await git(project, ["worktree", "add", "--force", worktreePath, taskBranch]);
      if (remoteExists) await git(project, ["merge", "--ff-only", `origin/${taskBranch}`], { cwd: worktreePath });
    } else if (remoteExists) {
      await git(project, ["worktree", "add", "--track", "-b", taskBranch, worktreePath, `origin/${taskBranch}`]);
    } else {
      throw new HttpError(409, `Source branch ${taskBranch} does not exist locally or on origin.`);
    }
    return { worktreePath, sourceSha };
  }
  if (localExists) {
    if (!task.sourceSha) {
      if (!task.resumeExistingBranch || !remoteExists) {
        throw new HttpError(409, `Local branch ${taskBranch} already exists outside this task.`);
      }
      const localRef = `refs/heads/${taskBranch}`;
      const remoteRef = `refs/remotes/origin/${taskBranch}`;
      if (!await isAncestor(project, localRef, remoteRef)) {
        throw new HttpError(409, `Local branch ${taskBranch} has commits that are not on origin and cannot be resumed safely.`);
      }
    }
    await git(project, ["worktree", "add", worktreePath, taskBranch]);
    if (remoteExists) await git(project, ["merge", "--ff-only", `origin/${taskBranch}`], { cwd: worktreePath });
    return { worktreePath, sourceSha };
  }
  if (remoteExists && !task.resumeExistingBranch) {
    throw new HttpError(409, `Branch ${taskBranch} already exists on origin. Enable “Resume existing branch” to use it.`);
  }

  if (remoteExists) {
    await git(project, ["worktree", "add", "--track", "-b", taskBranch, worktreePath, `origin/${taskBranch}`]);
  } else {
    await git(project, ["worktree", "add", "-b", taskBranch, worktreePath, sourceRef]);
  }
  return { worktreePath, sourceSha };
}

function parseNumstat(output: string) {
  const byPath = new Map<string, Pick<AgentGitFile, "additions" | "deletions" | "binary">>();
  for (const line of output.split("\n")) {
    if (!line) continue;
    const [added, deleted, ...parts] = line.split("\t");
    const binary = added === "-" || deleted === "-";
    byPath.set(parts.join("\t"), {
      additions: binary ? null : Number.parseInt(added, 10),
      deletions: binary ? null : Number.parseInt(deleted, 10),
      binary
    });
  }
  return byPath;
}

export async function taskGitPreview(project: ProjectRow, task: AgentTaskRow): Promise<AgentGitPreview> {
  if (!task.worktreePath || !await pathExists(task.worktreePath)) throw new HttpError(409, "Task worktree has not been created.");
  const cwd = task.worktreePath;
  const [statusOutput, numstat, diff, headSha, counts] = await Promise.all([
    git(project, ["status", "--porcelain=v1", "-z"], { cwd }),
    git(project, ["diff", "--numstat", "HEAD"], { cwd }),
    git(project, ["diff", "--no-ext-diff", "HEAD"], { cwd, maxOutputBytes: 2 * 1024 * 1024 }),
    git(project, ["rev-parse", "HEAD"], { cwd }),
    git(project, ["rev-list", "--left-right", "--count", `${task.sourceSha ?? `origin/${task.sourceBranch}`}...HEAD`], { cwd }).catch(() => "0\t0")
  ]);
  const stats = parseNumstat(numstat);
  const files: AgentGitFile[] = [];
  let combinedDiff = diff;
  const entries = statusOutput.split("\0").filter(Boolean);
  for (let index = 0; index < entries.length; index += 1) {
    const entry = entries[index];
    if (!entry) continue;
    const status = entry.slice(0, 2).trim() || "?";
    const filePath = entry.slice(3);
    if ((status.includes("R") || status.includes("C")) && entries[index + 1]) index += 1;
    let fileStats = stats.get(filePath) ?? { additions: null, deletions: null, binary: false };
    if (status === "?" || status === "??") {
      const fullPath = path.join(cwd, filePath);
      const stat = await fs.stat(fullPath).catch(() => null);
      if (stat?.isFile() && stat.size <= 200 * 1024) {
        const content = await fs.readFile(fullPath, "utf8").catch(() => "");
        if (!content.includes("\0")) {
          const lines = content.split("\n");
          fileStats = { additions: lines.length, deletions: 0, binary: false };
          combinedDiff += `\ndiff --git a/${filePath} b/${filePath}\nnew file mode 100644\n--- /dev/null\n+++ b/${filePath}\n@@ -0,0 +1,${lines.length} @@\n${lines.map((line) => `+${line}`).join("\n")}\n`;
        } else fileStats = { additions: null, deletions: null, binary: true };
      }
    }
    files.push({ path: filePath, status, ...fileStats });
  }
  const [behind, ahead] = counts.split(/\s+/).map((value) => Number.parseInt(value, 10) || 0);
  return { branch: task.taskBranch, baseBranch: task.sourceBranch, headSha, isClean: files.length === 0, ahead, behind, files, diff: combinedDiff.slice(0, 2 * 1024 * 1024) };
}

function safeSelectedPaths(worktreePath: string, paths: string[]) {
  return [...new Set(paths.map((value) => value.trim()).filter(Boolean))].map((value) => {
    const resolved = path.resolve(worktreePath, value);
    if (resolved !== worktreePath && !resolved.startsWith(`${worktreePath}${path.sep}`)) throw new HttpError(400, `Invalid selected path: ${value}`);
    return path.relative(worktreePath, resolved);
  });
}

export async function commitTaskWorktree(project: ProjectRow, task: AgentTaskRow, message: string, selectedPaths?: string[]) {
  if (!task.worktreePath) throw new HttpError(409, "Task worktree has not been created.");
  const commitMessage = message.trim();
  if (!commitMessage) throw new HttpError(400, "Commit message is required.");
  const paths = selectedPaths?.length ? safeSelectedPaths(task.worktreePath, selectedPaths) : [];
  if (paths.length) await git(project, ["add", "--", ...paths], { cwd: task.worktreePath });
  else await git(project, ["add", "-A"], { cwd: task.worktreePath });
  const staged = await git(project, ["diff", "--cached", "--name-only"], { cwd: task.worktreePath });
  if (!staged) throw new HttpError(409, "No staged changes to commit.");
  await git(project, ["-c", "user.name=Yanto Agent", "-c", "user.email=yanto@localhost", "commit", "-m", commitMessage], { cwd: task.worktreePath });
  return git(project, ["rev-parse", "HEAD"], { cwd: task.worktreePath });
}

export async function pushTaskWorktree(project: ProjectRow, task: AgentTaskRow) {
  if (!task.worktreePath) throw new HttpError(409, "Task worktree has not been created.");
  if (!project.gitUrl) throw new HttpError(400, "Project has no Git remote.");
  await git(project, ["push", "-u", "origin", task.taskBranch], { cwd: task.worktreePath });
  return git(project, ["rev-parse", "HEAD"], { cwd: task.worktreePath });
}

export async function cleanupTaskWorktree(project: ProjectRow, task: AgentTaskRow, force = false) {
  const worktreePath = task.worktreePath || taskWorktreePath(project, task);
  if (await pathExists(worktreePath)) await git(project, ["worktree", "remove", ...(force ? ["--force"] : []), worktreePath]);
  await pruneTaskWorktrees(project);
}

export async function pruneTaskWorktrees(project: ProjectRow) {
  if (await pathExists(project.localPath)) await git(project, ["worktree", "prune"]);
}
