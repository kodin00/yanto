import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DeploymentRow, ProjectRow } from "../src/server/db/schema.js";

const commandMocks = vi.hoisted(() => ({
  runCommand: vi.fn()
}));

vi.mock("../src/server/services/commands.js", () => ({
  runCommand: commandMocks.runCommand
}));

vi.mock("../src/server/services/ssh.js", () => ({
  gitSshEnv: vi.fn(() => ({ GIT_SSH_COMMAND: "ssh" })),
  resolveGitPrivateKeyPath: vi.fn(async () => null)
}));

import { runProjectDeployment } from "../src/server/services/deployment-runner.js";

let tempDir: string;

function project(overrides: Partial<ProjectRow> = {}): ProjectRow {
  return {
    id: "prj_test",
    name: "Test project",
    gitUrl: null,
    branch: "master",
    folderName: "test-project",
    localPath: tempDir,
    composeFile: "docker-compose.yml",
    composeContent: null,
    envFile: ".env",
    autoStart: false,
    manualDeployEnabled: true,
    githubWebhookEnabled: true,
    targetNodeId: "node_master_local",
    deployToken: "token",
    sshPrivateKeyPath: null,
    sshPublicKey: null,
    createdAt: new Date(),
    updatedAt: new Date(),
    ...overrides
  };
}

function deployment(overrides: Partial<DeploymentRow> = {}): DeploymentRow {
  return {
    id: "dep_test",
    projectId: "prj_test",
    nodeId: "node_master_local",
    status: "running",
    trigger: "manual",
    targetRef: null,
    commitSha: null,
    commitMessage: null,
    rollbackFromDeploymentId: null,
    logs: "",
    exitCode: null,
    startedAt: new Date(),
    finishedAt: null,
    ...overrides
  };
}

describe("project deployment runner", () => {
  beforeEach(async () => {
    tempDir = await fs.mkdtemp(path.join(os.tmpdir(), "yanto-deploy-"));
    commandMocks.runCommand.mockReset();
    commandMocks.runCommand.mockImplementation(async (command: string, args: string[]) => {
      if (command === "git" && args.join(" ") === "rev-parse --verify master") {
        return { exitCode: 1, output: "" };
      }
      if (command === "git" && args.join(" ") === "rev-parse HEAD") {
        return { exitCode: 0, output: "abc123\n" };
      }
      if (command === "git" && args.join(" ") === "log -1 --pretty=%s") {
        return { exitCode: 0, output: "latest commit\n" };
      }
      return { exitCode: 0, output: "" };
    });
  });

  afterEach(async () => {
    await fs.rm(tempDir, { recursive: true, force: true });
  });

  it("initializes a Git checkout in an existing non-Git project folder", async () => {
    await fs.writeFile(path.join(tempDir, ".env"), "APP_PORT=3000\n");
    await fs.writeFile(path.join(tempDir, "compose.yml"), "services:\n  web:\n    image: nginx\n");

    const logs: string[] = [];
    await runProjectDeployment(project({ gitUrl: "git@github.com:kodin00/envchecker.git" }), deployment(), {
      appendLog: async (chunk) => {
        logs.push(chunk);
      },
      updateMetadata: vi.fn()
    });

    expect(logs.join("")).toContain("initializing Git checkout in place");
    expect(commandMocks.runCommand).toHaveBeenCalledWith("git", ["init"], expect.objectContaining({ cwd: tempDir, env: expect.any(Object) }));
    expect(commandMocks.runCommand).toHaveBeenCalledWith("git", ["remote", "add", "origin", "git@github.com:kodin00/envchecker.git"], expect.any(Object));
    expect(commandMocks.runCommand).toHaveBeenCalledWith("git", ["checkout", "-B", "master", "origin/master"], expect.any(Object));
    expect(commandMocks.runCommand).toHaveBeenCalledWith("docker", ["compose", "-f", "compose.yml", "up", "-d", "--build", "--remove-orphans"], expect.any(Object));
  });

  it("reports a clear error when the compose file is missing", async () => {
    await expect(runProjectDeployment(project(), deployment(), { appendLog: vi.fn() })).rejects.toThrow(
      `Compose file docker-compose.yml was not found at ${path.join(tempDir, "docker-compose.yml")}`
    );
  });

  it("writes pending env after source checkout and before compose", async () => {
    await fs.writeFile(path.join(tempDir, "compose.yml"), "services:\n  web:\n    image: nginx\n");

    const logs: string[] = [];
    await runProjectDeployment(
      project({ gitUrl: "git@github.com:kodin00/envchecker.git" }),
      deployment(),
      {
        appendLog: async (chunk) => {
          logs.push(chunk);
        },
        updateMetadata: vi.fn()
      },
      { mode: "variables", variables: [{ key: "APP_PORT", value: "3000" }] }
    );

    expect(logs.join("")).toContain("Wrote .env after source checkout.");
    await expect(fs.readFile(path.join(tempDir, ".env"), "utf8")).resolves.toBe("APP_PORT=3000\n");
  });
});
