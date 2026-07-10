import { EventEmitter } from "node:events";
import { PassThrough, Writable } from "node:stream";
import { beforeEach, describe, expect, it, vi } from "vitest";

type FakeChild = EventEmitter & {
  stdin: Writable;
  stdout: PassThrough;
  stderr: PassThrough;
  kill: ReturnType<typeof vi.fn>;
};

function childProcess(): FakeChild {
  const child = new EventEmitter() as FakeChild;
  child.stdin = new Writable({ write(_chunk, _encoding, callback) { callback(); } });
  child.stdout = new PassThrough();
  child.stderr = new PassThrough();
  child.kill = vi.fn();
  return child;
}

const mocks = vi.hoisted(() => ({
  spawn: vi.fn(),
  mkdir: vi.fn(async () => undefined),
  resolveHostMountPath: vi.fn(async (value: string) => `/host${value}`)
}));

vi.mock("node:child_process", () => ({ spawn: mocks.spawn }));
vi.mock("node:fs/promises", () => ({ default: { mkdir: mocks.mkdir } }));
vi.mock("../src/server/services/agent-tools.js", () => ({ resolveHostMountPath: mocks.resolveHostMountPath }));

import { runCodexAccount } from "../src/server/services/codex-account-runner.js";

function input(signal: AbortSignal, event = vi.fn(async () => undefined)) {
  return {
    runId: "agr_test",
    taskId: "agt_test",
    worktreePath: "/worktree",
    prompt: "Do the task",
    model: "codex-test",
    threadId: null,
    signal,
    event,
    ensureSandbox: vi.fn(async () => undefined),
    prepareTaskHome: vi.fn(async () => "/task-home")
  };
}

describe("Codex account runner ownership", () => {
  beforeEach(() => vi.clearAllMocks());

  it("removes the exact Docker container when stopped immediately after spawn", async () => {
    const controller = new AbortController();
    const creator = childProcess();
    const remover = childProcess();
    mocks.spawn.mockImplementation((_command: string, args: string[]) => {
      if (args[0] === "create") {
        queueMicrotask(() => {
          controller.abort(new Error("Stopped by test."));
          creator.emit("exit", 137);
        });
        return creator;
      }
      if (args[0] === "rm") {
        queueMicrotask(() => remover.emit("exit", 0));
        return remover;
      }
      throw new Error(`Unexpected docker command: ${args.join(" ")}`);
    });

    await expect(runCodexAccount(input(controller.signal))).rejects.toThrow("Stopped by test.");

    const createArgs = mocks.spawn.mock.calls.find((call) => call[1][0] === "create")![1] as string[];
    const containerName = createArgs[createArgs.indexOf("--name") + 1];
    expect(createArgs).toEqual(expect.arrayContaining([
      "--label", "com.yanto.agent=true",
      "--label", "com.yanto.agent.run-id=agr_test",
      "--label", "com.yanto.agent.task-id=agt_test"
    ]));
    expect(createArgs).toContain("/host/task-home:/data/codex");
    expect(createArgs.join(" ")).not.toContain("/var/run/docker.sock");
    expect(createArgs.join(" ")).not.toContain("/.ssh");
    expect(mocks.spawn.mock.calls.some((call) => call[1][0] === "start")).toBe(false);
    expect(mocks.spawn).toHaveBeenCalledWith("docker", ["rm", "-f", containerName], expect.any(Object));
  });

  it("fails closed before Docker when the credential-isolation probe is unavailable", async () => {
    const testInput = input(new AbortController().signal);
    testInput.ensureSandbox = vi.fn(async () => { throw new Error("sandbox profile unavailable"); });

    await expect(runCodexAccount(testInput)).rejects.toThrow("sandbox profile unavailable");

    expect(testInput.prepareTaskHome).not.toHaveBeenCalled();
    expect(mocks.spawn).not.toHaveBeenCalled();
  });

  it("still removes the active container when an event callback throws", async () => {
    const creator = childProcess();
    const runner = childProcess();
    const remover = childProcess();
    mocks.spawn.mockImplementation((_command: string, args: string[]) => {
      if (args[0] === "create") {
        queueMicrotask(() => creator.emit("exit", 0));
        return creator;
      }
      if (args[0] === "start") {
        queueMicrotask(() => runner.stdout.write(`${JSON.stringify({ type: "assistant", text: "partial" })}\n`));
        return runner;
      }
      if (args[0] === "rm") {
        queueMicrotask(() => {
          runner.emit("exit", 137);
          remover.emit("exit", 0);
        });
        return remover;
      }
      throw new Error(`Unexpected docker command: ${args.join(" ")}`);
    });
    const event = vi.fn(async () => { throw new Error("subscriber failed"); });

    await expect(runCodexAccount(input(new AbortController().signal, event))).rejects.toThrow("subscriber failed");

    const createArgs = mocks.spawn.mock.calls.find((call) => call[1][0] === "create")![1] as string[];
    const containerName = createArgs[createArgs.indexOf("--name") + 1];
    expect(mocks.spawn).toHaveBeenCalledWith("docker", ["rm", "-f", containerName], expect.any(Object));
  });
});
