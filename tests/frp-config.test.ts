import { describe, expect, it } from "vitest";
import { buildFrpcToml } from "../src/server/services/frp.js";

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
});
