import express, { type NextFunction, type Request, type Response } from "express";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  controlFrpServer: vi.fn(),
  createFrpServer: vi.fn(),
  createFrpTunnel: vi.fn(),
  deleteFrpServer: vi.fn(),
  deleteFrpTunnel: vi.fn(),
  frpClientSetup: vi.fn(),
  frpNodeClientSetup: vi.fn(),
  frpOverview: vi.fn(),
  listFrpNodeAssignments: vi.fn(),
  listFrpServers: vi.fn(),
  saveFrpNodeAssignment: vi.fn(),
  saveFrpSettings: vi.fn(),
  updateFrpServer: vi.fn(),
  updateFrpTunnel: vi.fn()
}));

vi.mock("../src/server/auth.js", () => ({
  currentUser: () => ({ username: "admin" }),
  requireOwner: (req: Request, res: Response, next: NextFunction) => {
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
    const payload = { name: "Minecraft", protocol: "tcp", localHost: "127.0.0.1", localPort: 25565, remotePort: 25565, enabled: true };
    const response = await call("/api/frp/tunnels", {
      method: "POST",
      headers: { authorization: "ok", "content-type": "application/json" },
      body: JSON.stringify(payload)
    });
    expect(response).toEqual({ status: 201, body: tunnel });
    expect(mocks.createFrpTunnel).toHaveBeenCalledWith(payload);
  });

  it("assigns a tunnel to one client and server pair", async () => {
    const tunnel = { id: "frp_1", protocol: "tcp", remotePort: 25565, clientNodeId: "node_home", serverId: "frps_vps" };
    mocks.createFrpTunnel.mockResolvedValue(tunnel);
    const payload = { name: "Minecraft", protocol: "tcp", localHost: "127.0.0.1", localPort: 25565, remotePort: 25565, enabled: true, clientNodeId: "node_home", serverId: "frps_vps" };
    const response = await call("/api/frp/tunnels", {
      method: "POST",
      headers: { authorization: "ok", "content-type": "application/json" },
      body: JSON.stringify(payload)
    });
    expect(response).toEqual({ status: 201, body: tunnel });
    expect(mocks.createFrpTunnel).toHaveBeenCalledWith(payload);
  });

  it("creates servers and saves node assignments", async () => {
    const server = { id: "frps_vps", nodeId: "node_vps", publicHost: "vps.example.com" };
    mocks.createFrpServer.mockResolvedValue(server);
    const serverBody = { nodeId: "node_vps", name: "VPS", publicHost: "vps.example.com", bindPort: 7000, portStart: 25560, portEnd: 25600 };
    expect(await call("/api/frp/servers", {
      method: "POST",
      headers: { authorization: "ok", "content-type": "application/json" },
      body: JSON.stringify(serverBody)
    })).toEqual({ status: 201, body: server });
    expect(mocks.createFrpServer).toHaveBeenCalledWith(serverBody);

    const assignment = { nodeId: "node_home", role: "client", serverId: "frps_vps", desiredRevision: 1 };
    mocks.saveFrpNodeAssignment.mockResolvedValue(assignment);
    expect(await call("/api/frp/node-assignments/node_home", {
      method: "PUT",
      headers: { authorization: "ok", "content-type": "application/json" },
      body: JSON.stringify({ role: "client", serverId: "frps_vps" })
    })).toEqual({ status: 200, body: assignment });
    expect(mocks.saveFrpNodeAssignment).toHaveBeenCalledWith("node_home", { role: "client", serverId: "frps_vps" });
  });

  it("rejects unsupported protocols before calling the service", async () => {
    const response = await call("/api/frp/tunnels", {
      method: "POST",
      headers: { authorization: "ok", "content-type": "application/json" },
      body: JSON.stringify({ name: "Bad", protocol: "http", localHost: "localhost", localPort: 80, remotePort: 25565 })
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

  it("reveals manual FRPC setup for authenticated users", async () => {
    mocks.frpClientSetup.mockResolvedValue({ frpcToml: "serverAddr = \"x.x.x.x\"\n", installScript: "#!/usr/bin/env bash\n" });
    const response = await call("/api/frp/client-setup", { headers: { authorization: "ok" } });
    expect(response).toEqual({ status: 200, body: { frpcToml: "serverAddr = \"x.x.x.x\"\n", installScript: "#!/usr/bin/env bash\n" } });
    expect(mocks.frpClientSetup).toHaveBeenCalled();
  });

  it("reveals a node-specific FRPC setup", async () => {
    mocks.frpNodeClientSetup.mockResolvedValue({ frpcToml: 'clientID = "yanto-node-home"\n', tunnelCount: 1, tokenConfigured: true });
    const response = await call("/api/frp/nodes/node_home/client-setup", { headers: { authorization: "ok" } });
    expect(response).toEqual({ status: 200, body: { frpcToml: 'clientID = "yanto-node-home"\n', tunnelCount: 1, tokenConfigured: true } });
    expect(mocks.frpNodeClientSetup).toHaveBeenCalledWith("node_home");
  });
});
