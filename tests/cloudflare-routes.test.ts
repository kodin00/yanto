import express, { type NextFunction, type Request, type Response } from "express";
import { beforeEach, describe, expect, it, vi } from "vitest";

const cloudflareMocks = vi.hoisted(() => ({
  createCloudflareClient: vi.fn(),
  createManagedHostname: vi.fn(),
  createManagedTunnel: vi.fn(),
  createTunnelAssignment: vi.fn(),
  createDnsRecord: vi.fn(),
  deleteDnsRecord: vi.fn(),
  deleteCloudflareClient: vi.fn(),
  deleteManagedHostname: vi.fn(),
  deleteManagedTunnel: vi.fn(),
  deleteTunnelAssignment: vi.fn(),
  deleteProjectRoute: vi.fn(),
  disableProjectRoute: vi.fn(),
  enableProjectRoute: vi.fn(),
  getCloudflaredStatus: vi.fn(),
  getTunnelForNode: vi.fn(),
  getTunnelHealth: vi.fn(),
  listDnsRecords: vi.fn(),
  listCloudflareClients: vi.fn(),
  listCloudflareZones: vi.fn(),
  listManagedHostnames: vi.fn(),
  listPublicTunnels: vi.fn(),
  listRouteDiagnostics: vi.fn(),
  listRoutesForProject: vi.fn(),
  listTunnels: vi.fn(),
  listTunnelAssignments: vi.fn(),
  publishProjectRoute: vi.fn(),
  restartCloudflared: vi.fn(),
  retryManagedHostname: vi.fn(),
  startCloudflared: vi.fn(),
  stopCloudflared: vi.fn(),
  updateDnsRecord: vi.fn(),
  updateCloudflareClient: vi.fn(),
  validateCloudflareClient: vi.fn()
}));

vi.mock("../src/server/auth.js", () => ({
  requireAuth: (req: Request, res: Response, next: NextFunction) => {
    if (req.header("authorization") === "ok") {
      next();
      return;
    }
    res.status(401).json({ message: "Authentication required." });
  }
}));

vi.mock("../src/server/services/audit.js", () => ({
  recordAuditLog: vi.fn()
}));

vi.mock("../src/server/services/cloudflare.js", () => cloudflareMocks);

const { default: cloudflareRouter } = await import("../src/server/routes/cloudflare.js");

function createApp() {
  const app = express();
  app.use(express.json());
  app.use(cloudflareRouter);
  return app;
}

async function request(path: string, authorization?: string) {
  const server = createApp().listen(0);
  try {
    const address = server.address();
    if (!address || typeof address === "string") throw new Error("Server did not start.");
    const response = await fetch(`http://127.0.0.1:${address.port}${path}`, {
      headers: authorization ? { authorization } : {}
    });
    return { status: response.status, body: await response.json() as unknown };
  } finally {
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
}

describe("cloudflare routes", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("returns route diagnostics for authenticated users", async () => {
    const diagnostics = [{
      routeId: "cfr_1",
      tunnelId: "cft_1",
      projectId: "prj_1",
      projectName: "App",
      hostname: "app.example.com",
      serviceTarget: "http://app:3000",
      routeEnabled: true,
      expectedDnsTarget: "tun_1.cfargotunnel.com",
      actualDnsRecords: [],
      dnsStatus: "missing",
      tunnelStatus: "running",
      reachabilityStatus: "skipped",
      messages: ["DNS record is missing."],
      recommendedFixes: ["Create a CNAME."],
      checkedAt: "2026-06-05T00:00:00.000Z"
    }];
    cloudflareMocks.listRouteDiagnostics.mockResolvedValue(diagnostics);

    const response = await request("/api/cloudflare/routes/diagnostics", "ok");

    expect(response.status).toBe(200);
    expect(response.body).toEqual(diagnostics);
    expect(cloudflareMocks.listRouteDiagnostics).toHaveBeenCalledOnce();
  });

  it("blocks unauthenticated route diagnostics", async () => {
    const response = await request("/api/cloudflare/routes/diagnostics");

    expect(response.status).toBe(401);
    expect(response.body).toEqual({ message: "Authentication required." });
  });

  it("returns only the public tunnel representation", async () => {
    const tunnels = [{ id: "cft_1", tunnelName: "Acme", dockerNetworkName: "yanto-cf-cft_1" }];
    cloudflareMocks.listPublicTunnels.mockResolvedValue(tunnels);
    const response = await request("/api/cloudflare/tunnels", "ok");
    expect(response.status).toBe(200);
    expect(response.body).toEqual(tunnels);
    expect(cloudflareMocks.listPublicTunnels).toHaveBeenCalledOnce();
  });
});
