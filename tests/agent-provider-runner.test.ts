import { describe, expect, it, vi } from "vitest";
import { executeAgentTools } from "../src/server/services/agent-provider-runner.js";
import type { AgentWorkspace } from "../src/server/services/agent-tools.js";

describe("agent provider tool execution", () => {
  it("propagates cancellation and does not execute later queued tools", async () => {
    const controller = new AbortController();
    const execute = vi.fn(async () => {
      controller.abort(new Error("Stopped by test."));
      throw controller.signal.reason;
    });
    const workspace = { execute } as unknown as AgentWorkspace;
    const event = vi.fn(async () => undefined);

    await expect(executeAgentTools([
      { id: "call-1", name: "run_command", input: { command: "sleep 30" } },
      { id: "call-2", name: "write_file", input: { path: "after-stop.txt", content: "must not be written" } }
    ], workspace, controller.signal, event)).rejects.toThrow("Stopped by test.");

    expect(execute).toHaveBeenCalledTimes(1);
    expect(event).toHaveBeenCalledTimes(1);
    expect(event).toHaveBeenCalledWith("tool_call", expect.objectContaining({ id: "call-1" }));
  });

  it("continues after an ordinary tool error when the run is not canceled", async () => {
    const execute = vi.fn()
      .mockRejectedValueOnce(new Error("command failed"))
      .mockResolvedValueOnce("Wrote next.txt.");
    const workspace = { execute } as unknown as AgentWorkspace;
    const event = vi.fn(async () => undefined);

    const results = await executeAgentTools([
      { id: "call-1", name: "run_command", input: { command: "false" } },
      { id: "call-2", name: "write_file", input: { path: "next.txt", content: "ok" } }
    ], workspace, new AbortController().signal, event);

    expect(execute).toHaveBeenCalledTimes(2);
    expect(results).toEqual([
      { id: "call-1", name: "run_command", output: "command failed", isError: true },
      { id: "call-2", name: "write_file", output: "Wrote next.txt.", isError: false }
    ]);
  });
});
