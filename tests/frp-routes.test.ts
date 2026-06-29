import express, { type NextFunction, type Request, type Response } from "express";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  controlFrpServer: vi.fn(),
  createFrpTunnel: vi.fn(),
  deleteFrpTunnel: vi.fn(),
  frpOverview: vi.fn(),
  saveFrpSettings: vi.fn(),
  updateFrpTunnel: vi.fn()
}));

vi.mock("../src/server/auth.js", () => ({
  currentUser: () => ({ username: "admin" }),
  requireAuth: (req: Request, res: Response, next: NextFunction) => {
    if (req.header("authorization") === "ok") return next();
    res.status(401).json({ message: "Authentication required." });
  }
}));
vi.mock("../src/server/services/audit.js", () => ({ recordAuditLog: vi.fn() }));
vi.mock("../src/server/services/frp.js", () => mocks);

const { default: router } = await import("../src/server/routes/frp.js");

async function call(path: string, options: RequestInit = {}) {
  const app = express();
  app.use(express.json());
  app.use(router);
  app.use((_error: unknown, _req: Request, res: Response, next: NextFunction) => {
    void next;
    res.status(400).json({ message: "Invalid request." });
  });
  const server = app.listen(0);
  try {
    const address = server.address();
    if (!address || typeof address === "string") throw new Error("Server unavailable");
    const response = await fetch(`http://127.0.0.1:${address.port}${path}`, options);
    return { status: response.status, body: response.status === 204 ? null : await response.json() };
  } finally {
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
}

describe("FRP routes", () => {
  beforeEach(() => vi.clearAllMocks());

  it("protects the overview", async () => {
    expect(await call("/api/frp/overview")).toMatchObject({ status: 401 });
  });

  it("creates a validated TCP tunnel", async () => {
    const tunnel = { id: "frp_1", protocol: "tcp", remotePort: 25565 };
    mocks.createFrpTunnel.mockResolvedValue(tunnel);
    const payload = { name: "Minecraft", nodeId: "node_1", protocol: "tcp", localHost: "host.docker.internal", localPort: 25565, remotePort: 25565, enabled: true };
    const response = await call("/api/frp/tunnels", {
      method: "POST",
      headers: { authorization: "ok", "content-type": "application/json" },
      body: JSON.stringify(payload)
    });
    expect(response).toEqual({ status: 201, body: tunnel });
    expect(mocks.createFrpTunnel).toHaveBeenCalledWith(payload);
  });

  it("rejects unsupported protocols before calling the service", async () => {
    const response = await call("/api/frp/tunnels", {
      method: "POST",
      headers: { authorization: "ok", "content-type": "application/json" },
      body: JSON.stringify({ name: "Bad", nodeId: "node_1", protocol: "http", localHost: "localhost", localPort: 80, remotePort: 25565 })
    });
    expect(response.status).toBe(400);
    expect(mocks.createFrpTunnel).not.toHaveBeenCalled();
  });

  it("routes server lifecycle actions", async () => {
    mocks.controlFrpServer.mockResolvedValue({ running: true });
    const response = await call("/api/frp/server/restart", { method: "POST", headers: { authorization: "ok" } });
    expect(response).toEqual({ status: 200, body: { running: true } });
    expect(mocks.controlFrpServer).toHaveBeenCalledWith("restart");
  });
});
