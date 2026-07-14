import express, { type NextFunction, type Request, type Response } from "express";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DeploymentLogEvent } from "../src/server/services/deployment-events.js";

const mocks = vi.hoisted(() => {
  let listener: ((event: DeploymentLogEvent) => void) | undefined;
  const unsubscribe = vi.fn(() => {
    listener = undefined;
  });
  return {
    findDeployment: vi.fn(),
    latestDeployments: vi.fn(),
    onLogUpdate: vi.fn((_id: string, next: (event: DeploymentLogEvent) => void) => {
      listener = next;
      return unsubscribe;
    }),
    publish(event: DeploymentLogEvent) {
      listener?.(event);
    },
    reset() {
      listener = undefined;
      unsubscribe.mockReset();
    },
    unsubscribe
  };
});

vi.mock("../src/server/auth.js", () => ({
  authPrincipal: (req: Request) => req.yantoAuth,
  requireAuth: (req: Request, _res: Response, next: NextFunction) => {
    req.yantoAuth = { id: "usr_owner", username: "admin", role: "owner", status: "active", sessionVersion: 1, projectAccess: [] };
    next();
  }
}));
vi.mock("../src/server/services/deployments.js", () => ({
  findDeployment: mocks.findDeployment,
  latestDeployments: mocks.latestDeployments
}));
vi.mock("../src/server/services/deployment-events.js", () => ({
  deploymentEvents: { onLogUpdate: mocks.onLogUpdate }
}));

const { default: deploymentsRouter } = await import("../src/server/routes/deployments.js");

function createApp() {
  const app = express();
  app.use(deploymentsRouter);
  return app;
}

describe("deployment event streaming", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.reset();
  });

  afterEach(() => vi.restoreAllMocks());

  it("does not lose a terminal update while loading the initial snapshot", async () => {
    let finishSnapshot!: (value: Record<string, unknown>) => void;
    mocks.findDeployment.mockImplementationOnce(() => new Promise((resolve) => {
      finishSnapshot = resolve;
    }));
    const server = createApp().listen(0);
    try {
      const address = server.address();
      if (!address || typeof address === "string") throw new Error("Server did not start.");
      const responsePromise = fetch(`http://127.0.0.1:${address.port}/api/deployments/dep_test/logs/stream`);

      await vi.waitFor(() => expect(mocks.onLogUpdate).toHaveBeenCalledWith("dep_test", expect.any(Function)));
      mocks.publish({ deploymentId: "dep_test", logs: "complete\n", status: "success", done: true });
      finishSnapshot({ id: "dep_test", logs: "starting\n", status: "running" });

      const response = await responsePromise;
      const output = await response.text();
      expect(output).toContain('"logs":"starting\\n"');
      expect(output).toContain('"logs":"complete\\n"');
      expect(output).toContain('"done":true');
      expect(mocks.unsubscribe).toHaveBeenCalledOnce();
    } finally {
      await new Promise<void>((resolve) => {
        server.close(() => resolve());
        server.closeAllConnections();
      });
    }
  });

  it("unsubscribes when a streaming response is disconnected", async () => {
    mocks.findDeployment.mockResolvedValueOnce({ id: "dep_test", logs: "", status: "running" });
    const server = createApp().listen(0);
    const controller = new AbortController();
    try {
      const address = server.address();
      if (!address || typeof address === "string") throw new Error("Server did not start.");
      const response = await fetch(`http://127.0.0.1:${address.port}/api/deployments/dep_test/logs/stream`, {
        signal: controller.signal
      });
      expect(response.ok).toBe(true);
      controller.abort();
      await vi.waitFor(() => expect(mocks.unsubscribe).toHaveBeenCalledOnce());
    } finally {
      controller.abort();
      await new Promise<void>((resolve) => {
        server.close(() => resolve());
        server.closeAllConnections();
      });
    }
  });
});
