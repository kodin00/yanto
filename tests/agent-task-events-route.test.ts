import express, { type NextFunction, type Request, type Response } from "express";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentLiveEvent } from "../src/server/services/agent-events.js";

const mocks = vi.hoisted(() => {
  let listener: ((event: AgentLiveEvent) => void) | undefined;
  return {
    getAgentTask: vi.fn(),
    agentTaskEvents: vi.fn(),
    subscribe: vi.fn((_taskId: string, next: (event: AgentLiveEvent) => void) => {
      listener = next;
      return () => { listener = undefined; };
    }),
    publish(event: AgentLiveEvent) { listener?.(event); },
    resetListener() { listener = undefined; }
  };
});

vi.mock("../src/server/auth.js", () => ({
  currentUser: () => ({ username: "admin" }),
  requireAuth: (_req: Request, _res: Response, next: NextFunction) => next()
}));
vi.mock("../src/server/services/audit.js", () => ({ recordAuditLog: vi.fn() }));
vi.mock("../src/server/services/agent-events.js", () => ({ agentEventBus: { subscribe: mocks.subscribe } }));
vi.mock("../src/server/services/agent-tasks.js", () => ({
  agentTaskEvents: mocks.agentTaskEvents,
  branchesForProject: vi.fn(),
  cleanupAgentTask: vi.fn(),
  commitAgentTask: vi.fn(),
  createAgentTask: vi.fn(),
  deleteAgentTask: vi.fn(),
  getAgentTask: mocks.getAgentTask,
  gitPreviewForTask: vi.fn(),
  listAgentTasks: vi.fn(),
  pushAgentTask: vi.fn(),
  startAgentTask: vi.fn(),
  stopAgentTask: vi.fn(),
  updateAgentTask: vi.fn()
}));

const { default: agentTasksRouter } = await import("../src/server/routes/agent-tasks.js");

function createApp() {
  const app = express();
  app.use(express.json());
  app.use(agentTasksRouter);
  return app;
}

describe("agent task event streaming", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.resetListener();
    mocks.getAgentTask.mockResolvedValue({ id: "agt_test", status: "running", latestRun: { id: "agr_test" } });
    mocks.agentTaskEvents.mockResolvedValue([]);
  });

  afterEach(() => vi.restoreAllMocks());

  it("delivers a terminal event emitted between the snapshot query and live streaming", async () => {
    let finishSnapshot!: (events: unknown[]) => void;
    mocks.agentTaskEvents.mockImplementationOnce(() => new Promise((resolve) => { finishSnapshot = resolve; }));
    const server = createApp().listen(0);
    const controller = new AbortController();
    try {
      const address = server.address();
      if (!address || typeof address === "string") throw new Error("Server did not start.");
      const responsePromise = fetch(`http://127.0.0.1:${address.port}/api/agent/tasks/agt_test/events/stream`, { signal: controller.signal });
      await vi.waitFor(() => expect(mocks.agentTaskEvents).toHaveBeenCalledOnce());
      expect(mocks.subscribe.mock.invocationCallOrder[0]).toBeLessThan(mocks.getAgentTask.mock.invocationCallOrder[0]);

      mocks.publish({
        taskId: "agt_test", runId: "agr_test", sequence: 2, kind: "task_finished", payload: { status: "review" }, done: true
      });
      finishSnapshot([]);

      const response = await responsePromise;
      const reader = response.body!.getReader();
      const decoder = new TextDecoder();
      const first = await reader.read();
      let output = first.value ? decoder.decode(first.value, { stream: true }) : "";
      const second = await Promise.race([
        reader.read(),
        new Promise<null>((resolve) => setTimeout(() => resolve(null), 100))
      ]);
      if (second?.value) output += decoder.decode(second.value, { stream: true });
      await reader.cancel();
      controller.abort();

      expect(output).toContain('"kind":"task_finished"');
      expect(output).toContain('"done":true');
    } finally {
      controller.abort();
      await new Promise<void>((resolve) => {
        server.close(() => resolve());
        server.closeAllConnections();
      });
    }
  });

  it("includes a latest-run watermark and does not replay buffered events already in the snapshot", async () => {
    let finishSnapshot!: (events: Array<Record<string, unknown>>) => void;
    mocks.agentTaskEvents.mockImplementationOnce(() => new Promise((resolve) => { finishSnapshot = resolve; }));
    const server = createApp().listen(0);
    const controller = new AbortController();
    try {
      const address = server.address();
      if (!address || typeof address === "string") throw new Error("Server did not start.");
      const responsePromise = fetch(`http://127.0.0.1:${address.port}/api/agent/tasks/agt_test/events/stream`, { signal: controller.signal });
      await vi.waitFor(() => expect(mocks.agentTaskEvents).toHaveBeenCalledOnce());

      mocks.publish({ taskId: "agt_test", runId: "agr_test", sequence: 4, kind: "tool_call", payload: { name: "shell" } });
      mocks.publish({ taskId: "agt_test", runId: "agr_test", sequence: 5, kind: "task_finished", payload: { status: "review" }, done: true });
      finishSnapshot([{ id: "age_4", runId: "agr_test", sequence: 4, kind: "tool_call", payload: { name: "shell" } }]);

      const response = await responsePromise;
      const output = await response.text();
      expect(output).toContain('"watermark":{"runId":"agr_test","sequence":4}');
      expect(output.match(/"kind":"tool_call"/g)).toHaveLength(1);
      expect(output).toContain('"sequence":5');
      expect(mocks.agentTaskEvents).toHaveBeenCalledWith("agt_test", "agr_test");
    } finally {
      controller.abort();
      await new Promise<void>((resolve) => {
        server.close(() => resolve());
        server.closeAllConnections();
      });
    }
  });
});
