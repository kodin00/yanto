import fs from "node:fs";
import { describe, expect, it } from "vitest";
import YAML from "yaml";

describe("FRP compose runtime", () => {
  const compose = YAML.parse(fs.readFileSync("compose.yml", "utf8"));

  it("keeps the dashboard internal and publishes the bounded forwarding range", () => {
    const frps = compose.services.frps;
    expect(frps.container_name).toBe("yanto-frps");
    expect(frps.ports).toEqual([
      "${FRP_BIND_PORT:-7000}:${FRP_BIND_PORT:-7000}/tcp",
      "${FRP_PORT_START:-25560}-${FRP_PORT_END:-25600}:${FRP_PORT_START:-25560}-${FRP_PORT_END:-25600}/tcp",
      "${FRP_PORT_START:-25560}-${FRP_PORT_END:-25600}:${FRP_PORT_START:-25560}-${FRP_PORT_END:-25600}/udp"
    ]);
    expect(frps.ports.join(" ")).not.toContain("7500");
  });

  it("shares only the persistent token volume with the app without making Yanto depend on FRPS health", () => {
    expect(compose.services.frps.volumes).toContain("yanto_frp:/data/frp");
    expect(compose.services.app.volumes).toContain("yanto_frp:/data/frp:ro");
    expect(compose.services.app.depends_on.frps).toBeUndefined();
  });

  it("forces TLS and limits allowed ports in the FRPS config", () => {
    const config = fs.readFileSync("config/frps.toml", "utf8");
    expect(config).toContain("transport.tls.force = true");
    expect(config).toContain("auth.tokenSource.type = \"file\"");
    expect(config).toContain("allowPorts");
    expect(config).toContain("maxPortsPerClient = 20");
  });
});
