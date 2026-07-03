import { describe, expect, it } from "vitest";
import { findFrpTunnelProxy } from "../src/server/services/frp.js";

const tunnel = {
  id: "frp_internal_id",
  name: "kain-mc",
  protocol: "tcp",
  remotePort: 25565
} as const;

describe("FRP tunnel runtime status matching", () => {
  it("matches generated and manually named proxies", () => {
    expect(findFrpTunnelProxy(tunnel, [
      { name: "yanto-manual-frpc.frp_internal_id", proxyType: "tcp", status: "online" }
    ])?.status).toBe("online");
    expect(findFrpTunnelProxy(tunnel, [
      { name: "kain-mc", proxyType: "tcp", status: "online" }
    ])?.status).toBe("online");
  });

  it("matches a renamed proxy by its unique protocol and remote port", () => {
    expect(findFrpTunnelProxy(tunnel, [
      { name: "custom-name", proxyType: "tcp", conf: { remotePort: 25565 }, status: "online" }
    ])?.status).toBe("online");
  });

  it("does not match the same port on a different protocol", () => {
    expect(findFrpTunnelProxy(tunnel, [
      { name: "udp-service", proxyType: "udp", conf: { remotePort: 25565 }, status: "online" }
    ])).toBeUndefined();
  });

  it("prefers the connected proxy over stale offline dashboard history", () => {
    expect(findFrpTunnelProxy(tunnel, [
      { name: "frp_internal_id", proxyType: "tcp", conf: { remotePort: 25565 }, status: "offline" },
      { name: "kain-mc", proxyType: "tcp", conf: { remotePort: 25565 }, status: "online" }
    ])?.status).toBe("online");
  });
});
