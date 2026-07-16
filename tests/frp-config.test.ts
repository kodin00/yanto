import { describe, expect, it } from "vitest";
import { buildFrpcToml, buildFrpsToml, buildNodeFrpcToml, frpClientIdentity } from "../src/server/services/frp.js";

const desired = {
  serverAddr: "203.0.113.10",
  serverPort: 7000,
  authToken: "secret-token",
  tunnels: [{
    id: "frp_minecraft",
    protocol: "tcp",
    localHost: "host.docker.internal",
    localPort: 25565,
    remotePort: 25565
  }]
};

describe("FRPC configuration", () => {
  it("renders secure manual FRPC TOML with bounded proxy fields", () => {
    const rendered = buildFrpcToml(desired);
    expect(rendered).toContain('serverAddr = "203.0.113.10"');
    expect(rendered).toContain("serverPort = 7000");
    expect(rendered).toContain('token = "secret-token"');
    expect(rendered).toContain("additionalScopes = [\"HeartBeats\", \"NewWorkConns\"]");
    expect(rendered).toContain("wireProtocol = \"v2\"");
    expect(rendered).toContain("[transport.tls]");
    expect(rendered).toContain("enable = true");
    expect(rendered).toContain("[[proxies]]");
    expect(rendered).toContain('name = "frp_minecraft"');
    expect(rendered).toContain('localIP = "host.docker.internal"');
    expect(rendered).toContain("localPort = 25565");
    expect(rendered).toContain("remotePort = 25565");
  });

  it("uses an editable SSH sample when no tunnels exist yet", () => {
    const rendered = buildFrpcToml({ ...desired, serverAddr: "", authToken: "", tunnels: [] });
    expect(rendered).toContain('serverAddr = "x.x.x.x"');
    expect(rendered).toContain('token = "PASTE_FRP_TOKEN_HERE"');
    expect(rendered).toContain('name = "ssh"');
    expect(rendered).toContain("localPort = 22");
  });

  it("renders node-specific identities and does not invent tunnels", () => {
    const rendered = buildNodeFrpcToml({
      nodeId: "node/home server#1",
      serverAddr: "vps.example.com",
      serverPort: 7000,
      authToken: "secret-token",
      tunnels: []
    });
    const identity = frpClientIdentity("node/home server#1");
    expect(rendered).toContain(`clientID = "${identity}"`);
    expect(rendered).toContain(`user = "${identity}"`);
    expect(rendered).not.toContain("[[proxies]]");
    expect(frpClientIdentity("node/home.server#1")).not.toBe(identity);
  });

  it("renders a managed FRPS config with a bounded forwarding range", () => {
    const rendered = buildFrpsToml({ bindPort: 7001, portStart: 25000, portEnd: 25100, authToken: "node-token" });
    expect(rendered).toContain('bindAddr = "0.0.0.0"');
    expect(rendered).toContain("bindPort = 7001");
    expect(rendered).toContain("allowPorts = [{ start = 25000, end = 25100 }]");
    expect(rendered).toContain('additionalScopes = ["HeartBeats", "NewWorkConns"]');
    expect(rendered).toContain('token = "node-token"');
  });
});
