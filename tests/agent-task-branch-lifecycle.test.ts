import { execFileSync } from "node:child_process";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AgentTaskRow, ProjectRow } from "../src/server/db/schema.js";
import { cleanupTaskWorktree, commitTaskWorktree, prepareTaskWorktree, pushTaskWorktree } from "../src/server/services/agent-worktrees.js";

function git(cwd: string, ...args: string[]) {
  return execFileSync("git", args, { cwd, encoding: "utf8" }).trim();
}

describe("deleted agent task branch retention", () => {
  let root: string;
  let remote: string;
  let checkout: string;
  let project: ProjectRow;

  beforeEach(async () => {
    root = await fs.mkdtemp(path.join(os.tmpdir(), "yanto-agent-resume-"));
    const seed = path.join(root, "seed");
    remote = path.join(root, "remote.git");
    checkout = path.join(root, "project");
    await fs.mkdir(seed);
    git(root, "init", "--bare", remote);
    git(seed, "init");
    git(seed, "config", "user.email", "test@example.com");
    git(seed, "config", "user.name", "Test");
    await fs.writeFile(path.join(seed, "README.md"), "seed\n");
    git(seed, "add", "README.md");
    git(seed, "commit", "-m", "seed");
    git(seed, "branch", "-M", "main");
    git(seed, "remote", "add", "origin", remote);
    git(seed, "push", "-u", "origin", "main");
    git(remote, "symbolic-ref", "HEAD", "refs/heads/main");
    git(root, "clone", remote, checkout);
    project = {
      id: "prj_test", name: "Project", gitUrl: remote, branch: "main", folderName: "project", localPath: checkout,
      composeFile: "compose.yml", composeContent: null, envFile: ".env", autoStart: false, manualDeployEnabled: true,
      githubWebhookEnabled: true, targetNodeId: "node_master_local", deployToken: "token", sshPrivateKeyPath: null,
      sshPublicKey: null, agentImage: "", createdAt: new Date(), updatedAt: new Date()
    };
  });

  afterEach(async () => fs.rm(root, { recursive: true, force: true }));

  function task(id: string, taskBranch = "task/pushed"): AgentTaskRow {
    return {
      id, projectId: project.id, modelId: "aim_test", title: id, prompt: "Do it", status: "backlog", sourceBranch: "main",
      taskBranch, sourceSha: null, worktreePath: path.join(root, id), codexThreadId: null,
      resumeExistingBranch: false, autoCommit: false, autoPush: false, autoCleanup: false, lastError: null,
      createdAt: new Date(), updatedAt: new Date(), startedAt: null, finishedAt: null, pushedAt: null, archivedAt: null
    };
  }

  it("explicitly resumes a pushed branch after its original task is deleted", async () => {
    const original = task("agt_original");
    const prepared = await prepareTaskWorktree(project, original);
    const activeOriginal = { ...original, sourceSha: prepared.sourceSha, worktreePath: prepared.worktreePath };
    await fs.writeFile(path.join(prepared.worktreePath, "result.txt"), "completed\n");
    const pushedSha = await commitTaskWorktree(project, activeOriginal, "complete original task");
    await pushTaskWorktree(project, activeOriginal);
    await cleanupTaskWorktree(project, activeOriginal);
    expect(git(checkout, "rev-parse", "refs/heads/task/pushed")).toBe(pushedSha);
    expect(git(remote, "rev-parse", "refs/heads/task/pushed")).toBe(pushedSha);

    // Database deletion is intentionally represented by dropping the old row. Git branches must remain untouched.
    const replacement = { ...task("agt_replacement"), resumeExistingBranch: true };
    const resumed = await prepareTaskWorktree(project, replacement);

    expect(git(resumed.worktreePath, "rev-parse", "HEAD")).toBe(pushedSha);
    expect(git(remote, "rev-parse", "refs/heads/task/pushed")).toBe(pushedSha);
    await cleanupTaskWorktree(project, { ...replacement, worktreePath: resumed.worktreePath });
  });

  it("rejects an ownerless local-only branch without deleting its unpushed commit", async () => {
    git(checkout, "checkout", "-b", "task/local-only");
    await fs.writeFile(path.join(checkout, "local.txt"), "local work\n");
    git(checkout, "add", "local.txt");
    git(checkout, "commit", "-m", "local-only work");
    const localSha = git(checkout, "rev-parse", "HEAD");
    git(checkout, "checkout", "main");

    await expect(prepareTaskWorktree(project, {
      ...task("agt_replacement", "task/local-only"),
      resumeExistingBranch: true
    })).rejects.toMatchObject({ status: 409 });

    expect(git(checkout, "rev-parse", "refs/heads/task/local-only")).toBe(localSha);
    expect(() => git(remote, "show-ref", "--verify", "--quiet", "refs/heads/task/local-only")).toThrow();
  });

  it("fast-forwards a retained local branch only to its matching origin branch", async () => {
    const original = task("agt_original");
    const prepared = await prepareTaskWorktree(project, original);
    const activeOriginal = { ...original, sourceSha: prepared.sourceSha, worktreePath: prepared.worktreePath };
    await fs.writeFile(path.join(prepared.worktreePath, "result.txt"), "first push\n");
    await commitTaskWorktree(project, activeOriginal, "first pushed work");
    await pushTaskWorktree(project, activeOriginal);
    await cleanupTaskWorktree(project, activeOriginal);

    const peer = path.join(root, "peer");
    git(root, "clone", remote, peer);
    git(peer, "config", "user.email", "peer@example.com");
    git(peer, "config", "user.name", "Peer");
    git(peer, "checkout", "task/pushed");
    await fs.writeFile(path.join(peer, "remote.txt"), "remote advancement\n");
    git(peer, "add", "remote.txt");
    git(peer, "commit", "-m", "advance remote branch");
    const remoteSha = git(peer, "rev-parse", "HEAD");
    git(peer, "push", "origin", "task/pushed");

    const replacement = { ...task("agt_replacement"), resumeExistingBranch: true };
    const resumed = await prepareTaskWorktree(project, replacement);

    expect(git(resumed.worktreePath, "rev-parse", "HEAD")).toBe(remoteSha);
    expect(git(checkout, "rev-parse", "refs/heads/task/pushed")).toBe(remoteSha);
    await cleanupTaskWorktree(project, { ...replacement, worktreePath: resumed.worktreePath });
  });

  it("rejects a local branch ahead of origin so unpushed commits are preserved", async () => {
    const original = task("agt_original");
    const prepared = await prepareTaskWorktree(project, original);
    const activeOriginal = { ...original, sourceSha: prepared.sourceSha, worktreePath: prepared.worktreePath };
    await fs.writeFile(path.join(prepared.worktreePath, "result.txt"), "pushed\n");
    const pushedSha = await commitTaskWorktree(project, activeOriginal, "pushed work");
    await pushTaskWorktree(project, activeOriginal);
    await cleanupTaskWorktree(project, activeOriginal);

    git(checkout, "checkout", "task/pushed");
    await fs.writeFile(path.join(checkout, "local.txt"), "not pushed\n");
    git(checkout, "add", "local.txt");
    git(checkout, "commit", "-m", "unpushed local work");
    const localSha = git(checkout, "rev-parse", "HEAD");
    git(checkout, "checkout", "main");

    await expect(prepareTaskWorktree(project, {
      ...task("agt_replacement"),
      resumeExistingBranch: true
    })).rejects.toMatchObject({ status: 409 });

    expect(git(checkout, "rev-parse", "refs/heads/task/pushed")).toBe(localSha);
    expect(git(remote, "rev-parse", "refs/heads/task/pushed")).toBe(pushedSha);
  });
});
