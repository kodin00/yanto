import { describe, expect, it } from "vitest";
import { compactPersistedAgentEvent } from "../src/server/services/agent-event-payload.js";

describe("persisted agent event payloads", () => {
  it("caps persisted tool output while retaining review metadata", () => {
    const payload = { id: "call_1", name: "shell", status: "completed", output: "x".repeat(20_000), isError: false };
    const compact = compactPersistedAgentEvent("tool_result", payload, 1_024);

    expect(Buffer.byteLength(JSON.stringify(compact))).toBeLessThanOrEqual(1_024);
    expect(compact).toMatchObject({ id: "call_1", name: "shell", status: "completed", isError: false, truncated: true });
    expect(String(compact.output).length).toBeLessThan(payload.output.length);
  });

  it("does not cap final-state payloads", () => {
    const payload = { assistantText: "x".repeat(2_000), status: "succeeded" };
    expect(compactPersistedAgentEvent("run_finished", payload, 128)).toBe(payload);
  });

  it("caps incremental tool updates", () => {
    const compact = compactPersistedAgentEvent("tool_update", { id: "call_2", status: "in_progress", output: "x".repeat(20_000) }, 1_024);
    expect(Buffer.byteLength(JSON.stringify(compact))).toBeLessThanOrEqual(1_024);
    expect(compact).toMatchObject({ id: "call_2", status: "in_progress", truncated: true });
  });
});
