import { execFileSync } from "node:child_process";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AgentTaskRow, ProjectRow } from "../src/server/db/schema.js";
import { cleanupTaskWorktree, commitTaskWorktree, fetchProjectBranches, prepareTaskWorktree, pushTaskWorktree, taskGitPreview } from "../src/server/services/agent-worktrees.js";

function git(cwd: string, ...args: string[]) {
  return execFileSync("git", args, { cwd, encoding: "utf8" }).trim();
}

describe("agent worktrees", () => {
  let root: string;
  let seed: string;
  let remote: string;
  let checkout: string;
  let project: ProjectRow;

  beforeEach(async () => {
    root = await fs.mkdtemp(path.join(os.tmpdir(), "yanto-agent-worktree-"));
    seed = path.join(root, "seed");
    remote = path.join(root, "remote.git");
    checkout = path.join(root, "project");
    await fs.mkdir(seed);
    git(root, "init", "--bare", remote);
    git(seed, "init");
    git(seed, "config", "user.email", "test@example.com");
    git(seed, "config", "user.name", "Test");
    await fs.writeFile(path.join(seed, "README.md"), "first\n");
    git(seed, "add", "README.md");
    git(seed, "commit", "-m", "first");
    git(seed, "branch", "-M", "main");
    git(seed, "remote", "add", "origin", remote);
    git(seed, "push", "-u", "origin", "main");
    git(remote, "symbolic-ref", "HEAD", "refs/heads/main");
    git(root, "clone", remote, checkout);
    project = {
      id: "prj_test", name: "Test", gitUrl: remote, branch: "main", folderName: "project", localPath: checkout,
      composeFile: "compose.yml", composeContent: null, envFile: ".env", autoStart: false, manualDeployEnabled: true,
      githubWebhookEnabled: true, targetNodeId: "node_master_local", deployToken: "token", sshPrivateKeyPath: null,
      sshPublicKey: null, agentImage: "", createdAt: new Date(), updatedAt: new Date()
    };
  });

  afterEach(async () => { await fs.rm(root, { recursive: true, force: true }); });

  function task(overrides: Partial<AgentTaskRow> = {}): AgentTaskRow {
    return {
      id: "agt_test", projectId: project.id, modelId: "aim_test", title: "Test", prompt: "Do it", status: "backlog",
      sourceBranch: "main", taskBranch: "task/test", sourceSha: null, worktreePath: path.join(root, "worktree"),
      resumeExistingBranch: false, autoCommit: false, autoPush: false, autoCleanup: false, lastError: null,
      createdAt: new Date(), updatedAt: new Date(), startedAt: null, finishedAt: null, pushedAt: null, ...overrides
    };
  }

  it("fetches origin and creates the task branch from its newest commit", async () => {
    await fs.writeFile(path.join(seed, "README.md"), "second\n");
    git(seed, "add", "README.md");
    git(seed, "commit", "-m", "second");
    git(seed, "push", "origin", "main");
    const remoteHead = git(seed, "rev-parse", "HEAD");

    const branches = await fetchProjectBranches(project);
    expect(branches).toContainEqual({ name: "main", sha: remoteHead, remote: true });

    const row = task();
    const prepared = await prepareTaskWorktree(project, row);
    expect(prepared.sourceSha).toBe(remoteHead);
    expect(git(prepared.worktreePath, "branch", "--show-current")).toBe("task/test");
    expect(await fs.readFile(path.join(prepared.worktreePath, "README.md"), "utf8")).toBe("second\n");
    await fs.writeFile(path.join(prepared.worktreePath, "new.txt"), "new file\n");
    const activeTask = { ...row, sourceSha: prepared.sourceSha, worktreePath: prepared.worktreePath };
    const preview = await taskGitPreview(project, activeTask);
    expect(preview.files).toEqual(expect.arrayContaining([expect.objectContaining({ path: "new.txt", status: "??", additions: 2 })]));
    expect(preview.diff).toContain("+++ b/new.txt");
    const commit = await commitTaskWorktree(project, activeTask, "add new file");
    await expect(pushTaskWorktree(project, activeTask)).resolves.toBe(commit);
    expect(git(remote, "rev-parse", "refs/heads/task/test")).toBe(commit);
    expect((await taskGitPreview(project, activeTask)).isClean).toBe(true);
    await cleanupTaskWorktree(project, activeTask);
  });

  it("requires explicit resume before tracking an existing remote task branch", async () => {
    git(seed, "checkout", "-b", "task/existing");
    git(seed, "push", "-u", "origin", "task/existing");
    git(seed, "checkout", "main");
    const row = task({ taskBranch: "task/existing" });

    await expect(prepareTaskWorktree(project, row)).rejects.toThrow("Enable “Resume existing branch”");
    const prepared = await prepareTaskWorktree(project, { ...row, resumeExistingBranch: true });
    expect(git(prepared.worktreePath, "branch", "--show-current")).toBe("task/existing");
    await cleanupTaskWorktree(project, { ...row, resumeExistingBranch: true, worktreePath: prepared.worktreePath });
  });
});
