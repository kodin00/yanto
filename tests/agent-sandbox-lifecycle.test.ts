import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  runCommand: vi.fn(async (_command: string, args: string[]) => ({
    exitCode: args[0] === "inspect" ? 1 : 0,
    output: ""
  }))
}));

vi.mock("../src/server/services/commands.js", () => ({ runCommand: mocks.runCommand }));

import { AgentSandbox } from "../src/server/services/agent-tools.js";

describe("API-key agent sandbox ownership", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.runCommand.mockImplementation(async (_command: string, args: string[]) => ({
      exitCode: args[0] === "inspect" ? 1 : 0,
      output: ""
    }));
  });

  it("creates and starts a labeled container, then removes it idempotently", async () => {
    const sandbox = new AgentSandbox("agr_test", "/worktree", "agent:test", { taskId: "agt_test" });

    await sandbox.start();
    await sandbox.stop();
    await sandbox.stop();

    const createCall = mocks.runCommand.mock.calls.find((call) => call[1][0] === "create");
    expect(createCall?.[1]).toEqual(expect.arrayContaining([
      "--name", sandbox.containerName,
      "--label", "com.yanto.agent=true",
      "--label", "com.yanto.agent.run-id=agr_test",
      "--label", "com.yanto.agent.task-id=agt_test"
    ]));
    expect(mocks.runCommand).toHaveBeenCalledWith("docker", ["start", sandbox.containerName], expect.any(Object));
    expect(mocks.runCommand.mock.calls.filter((call) => call[1][0] === "rm")).toEqual([
      ["docker", ["rm", "-f", sandbox.containerName], expect.any(Object)]
    ]);
  });

  it("removes the created container without starting it when cancellation wins the race", async () => {
    const controller = new AbortController();
    mocks.runCommand.mockImplementation(async (_command: string, args: string[]) => {
      if (args[0] === "inspect") return { exitCode: 1, output: "" };
      if (args[0] === "create") controller.abort(new Error("Stopped by test."));
      return { exitCode: 0, output: "" };
    });
    const sandbox = new AgentSandbox("agr_test", "/worktree", "agent:test", { taskId: "agt_test", signal: controller.signal });

    await expect(sandbox.start()).rejects.toThrow("Stopped by test.");
    await sandbox.stop();

    expect(mocks.runCommand.mock.calls.some((call) => call[1][0] === "start")).toBe(false);
    expect(mocks.runCommand).toHaveBeenCalledWith("docker", ["rm", "-f", sandbox.containerName], expect.any(Object));
  });
});
