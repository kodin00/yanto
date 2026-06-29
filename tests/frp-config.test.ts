import { describe, expect, it } from "vitest";
import { buildFrpcConfig, type WorkerFrpConfig } from "../src/server/services/worker-frp.js";

const desired: WorkerFrpConfig = {
  configured: true,
  nodeId: "node_home",
  serverAddr: "203.0.113.10",
  serverPort: 7000,
  wireProtocol: "v2",
  revision: "revision",
  authToken: "secret-token",
  tunnels: [{
    id: "frp_minecraft",
    name: "Minecraft Java",
    protocol: "tcp",
    localHost: "host.docker.internal",
    localPort: 25565,
    remotePort: 25565
  }]
};

describe("FRPC configuration", () => {
  it("renders worker identity, secure transport, and bounded proxy fields", () => {
    expect(buildFrpcConfig(desired)).toEqual({
      clientID: "node_home",
      user: "node_home",
      serverAddr: "203.0.113.10",
      serverPort: 7000,
      loginFailExit: false,
      auth: { method: "token", token: "secret-token", additionalScopes: ["HeartBeats", "NewWorkConns"] },
      transport: { wireProtocol: "v2", tls: { enable: true } },
      log: { to: "console", level: "info", disablePrintColor: true },
      proxies: [{
        name: "frp_minecraft",
        type: "tcp",
        localIP: "host.docker.internal",
        localPort: 25565,
        remotePort: 25565
      }]
    });
  });

  it("does not place display names in FRP wire identifiers", () => {
    const rendered = buildFrpcConfig({ ...desired, tunnels: [{ ...desired.tunnels[0], name: "Name with spaces" }] });
    expect(rendered.proxies[0].name).toBe("frp_minecraft");
    expect(JSON.stringify(rendered)).not.toContain("Name with spaces");
  });
});
