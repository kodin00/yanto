import { describe, expect, it, vi } from "vitest";
import { runCodexAccount } from "../src/server/services/codex-account-runner.js";

type Event = {
  type: string;
  [key: string]: unknown;
};

async function* events(values: Event[]) {
  for (const value of values) yield value;
}

function harness(values: Event[]) {
  const runStreamed = vi.fn(async (...args: [string, { signal: AbortSignal }]) => {
    void args;
    return { events: events(values) };
  });
  const thread = { runStreamed };
  const startThread = vi.fn(() => thread);
  const resumeThread = vi.fn(() => thread);
  const createCodex = vi.fn(() => ({ startThread, resumeThread }));
  return { runStreamed, startThread, resumeThread, createCodex };
}

function input(values: Event[], overrides: Record<string, unknown> = {}) {
  const sdk = harness(values);
  const event = vi.fn(async () => undefined);
  return {
    sdk,
    event,
    value: {
      runId: "agr_test",
      taskId: "agt_test",
      worktreePath: "/worktrees/agt_test",
      prompt: "Do the task",
      model: "codex-test",
      threadId: null,
      signal: new AbortController().signal,
      event,
      prepareTaskHome: vi.fn(async () => "/codex/task-home"),
      createCodex: sdk.createCodex,
      ...overrides
    }
  };
}

describe("host-native Codex account runner", () => {
  it("runs the SDK directly in the task worktree with a task-local Codex home", async () => {
    const test = input([
      { type: "thread.started", thread_id: "thread-1" },
      { type: "item.completed", item: { id: "msg-1", type: "agent_message", text: "Finished" } },
      { type: "turn.completed", usage: { input_tokens: 1, cached_input_tokens: 0, output_tokens: 1, reasoning_output_tokens: 0 } }
    ]);

    await expect(runCodexAccount(test.value as never)).resolves.toEqual({ assistantText: "Finished", threadId: "thread-1" });

    expect(test.sdk.createCodex).toHaveBeenCalledWith({
      env: expect.objectContaining({ CODEX_HOME: "/codex/task-home", HOME: "/codex/task-home" })
    });
    expect(test.sdk.startThread).toHaveBeenCalledWith({
      workingDirectory: "/worktrees/agt_test",
      sandboxMode: "danger-full-access",
      approvalPolicy: "never",
      model: "codex-test"
    });
    expect(test.sdk.runStreamed).toHaveBeenCalledWith(expect.stringContaining("User request:\nDo the task"), { signal: expect.any(AbortSignal) });
    expect(test.event).toHaveBeenCalledWith("codex_thread", { threadId: "thread-1" });
    expect(test.event).toHaveBeenCalledWith("assistant_delta", { delta: "Finished" });
  });

  it("emits started, updated, and completed command activity live", async () => {
    const base = { id: "cmd-1", type: "command_execution", command: "npm test" };
    const test = input([
      { type: "item.started", item: { ...base, aggregated_output: "", status: "in_progress" } },
      { type: "item.updated", item: { ...base, aggregated_output: "halfway", status: "in_progress" } },
      { type: "item.completed", item: { ...base, aggregated_output: "passed", exit_code: 0, status: "completed" } },
      { type: "turn.completed", usage: { input_tokens: 1, cached_input_tokens: 0, output_tokens: 1, reasoning_output_tokens: 0 } }
    ]);

    await runCodexAccount(test.value as never);

    expect(test.event).toHaveBeenNthCalledWith(1, "tool_call", expect.objectContaining({ id: "cmd-1", command: "npm test", phase: "started", status: "in_progress" }));
    expect(test.event).toHaveBeenNthCalledWith(2, "tool_update", expect.objectContaining({ id: "cmd-1", output: "halfway", phase: "updated" }));
    expect(test.event).toHaveBeenNthCalledWith(3, "tool_result", expect.objectContaining({ id: "cmd-1", output: "passed", exitCode: 0, phase: "completed", isError: false }));
  });

  it("coalesces bursty command updates and bounds their cumulative output", async () => {
    const base = { id: "cmd-1", type: "command_execution", command: "npm test", status: "in_progress" };
    const firstOutput = `prefix-${"x".repeat(40_000)}-first-tail`;
    const test = input([
      { type: "item.updated", item: { ...base, aggregated_output: firstOutput } },
      { type: "item.updated", item: { ...base, aggregated_output: `${firstOutput}-second-tail` } },
      { type: "item.updated", item: { ...base, aggregated_output: `${firstOutput}-third-tail` } },
      { type: "item.completed", item: { ...base, aggregated_output: "finished", exit_code: 0, status: "completed" } },
      { type: "turn.completed", usage: { input_tokens: 1, cached_input_tokens: 0, output_tokens: 1, reasoning_output_tokens: 0 } }
    ]);

    await runCodexAccount(test.value as never);

    const updates = test.event.mock.calls.filter(([kind]) => kind === "tool_update");
    expect(updates).toHaveLength(1);
    expect(updates[0][1]).toMatchObject({ id: "cmd-1", outputTruncated: true, phase: "updated" });
    expect(Buffer.byteLength(String(updates[0][1].output), "utf8")).toBeLessThanOrEqual(16 * 1024);
    expect(String(updates[0][1].output)).toContain("first-tail");
    expect(test.event).toHaveBeenCalledWith("tool_result", expect.objectContaining({ output: "finished", outputTruncated: false }));
  });

  it("passes cancellation to the SDK and exposes an idempotent stop callback", async () => {
    let streamedSignal!: AbortSignal;
    let release!: () => void;
    const sdk = harness([]);
    sdk.runStreamed.mockImplementationOnce(async (_prompt, options) => {
      streamedSignal = options.signal;
      return {
        events: (async function* () {
          yield* [];
          await new Promise<void>((resolve, reject) => {
            release = resolve;
            options.signal.addEventListener("abort", () => reject(options.signal.reason), { once: true });
          });
        })()
      };
    });
    let stop!: () => Promise<void>;
    const test = input([], {
      createCodex: sdk.createCodex,
      registerStop: (callback: () => Promise<void>) => { stop = callback; }
    });

    const running = runCodexAccount(test.value as never);
    await vi.waitFor(() => expect(streamedSignal).toBeInstanceOf(AbortSignal));
    await stop();
    await stop();
    await expect(running).rejects.toThrow("Codex run stopped");
    expect(streamedSignal.aborted).toBe(true);
    release?.();
  });
});
