import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  runCommand: vi.fn(async () => ({ exitCode: 0, output: "ok" }))
}));

vi.mock("../src/server/services/commands.js", () => ({ runCommand: mocks.runCommand }));

import { AgentWorkspace } from "../src/server/services/agent-tools.js";

describe("host-native agent workspace lifecycle", () => {
  let root: string;

  beforeEach(async () => {
    vi.clearAllMocks();
    root = await fs.mkdtemp(path.join(os.tmpdir(), "yanto-agent-workspace-"));
  });

  afterEach(async () => { await fs.rm(root, { recursive: true, force: true }); });

  it("launches no Docker container and executes commands in the exact worktree", async () => {
    const workspace = new AgentWorkspace(root);
    await workspace.start();
    await workspace.execute("run_command", { command: "npm test" });
    await workspace.stop();
    await workspace.stop();

    expect(mocks.runCommand).toHaveBeenCalledOnce();
    expect(mocks.runCommand).toHaveBeenCalledWith("sh", ["-lc", "npm test"], expect.objectContaining({
      cwd: root,
      inheritEnv: false,
      killProcessGroup: true,
      signal: expect.any(AbortSignal)
    }));
    expect(mocks.runCommand.mock.calls.some(([command]) => command === "docker")).toBe(false);
  });

  it("refuses startup after cancellation", async () => {
    const controller = new AbortController();
    controller.abort(new Error("Stopped by test."));
    const workspace = new AgentWorkspace(root, { signal: controller.signal });

    await expect(workspace.start()).rejects.toThrow("Stopped by test.");
    expect(mocks.runCommand).not.toHaveBeenCalled();
  });
});
