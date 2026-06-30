import { describe, expect, it } from "vitest";
import { frpSettingsInput, frpTunnelInput } from "../src/server/route-schemas.js";
import { validateFrpRemotePort } from "../src/server/services/frp.js";

describe("FRP validation", () => {
  it("accepts direct IPs and DNS-only hostnames but rejects URLs", () => {
    expect(frpSettingsInput.parse({ publicHost: "203.0.113.10" }).publicHost).toBe("203.0.113.10");
    expect(frpSettingsInput.parse({ publicHost: "games.example.com" }).publicHost).toBe("games.example.com");
    expect(() => frpSettingsInput.parse({ publicHost: "https://games.example.com" })).toThrow();
  });

  it("requires TCP or UDP and valid service ports", () => {
    const base = { name: "Minecraft", localHost: "127.0.0.1", localPort: 25565, remotePort: 25565 };
    expect(frpTunnelInput.parse({ ...base, protocol: "tcp" })).toMatchObject({ protocol: "tcp", enabled: true });
    expect(() => frpTunnelInput.parse({ ...base, protocol: "http" })).toThrow();
    expect(() => frpTunnelInput.parse({ ...base, protocol: "tcp", localPort: 70000 })).toThrow();
  });

  it("enforces the configured public port range", () => {
    expect(() => validateFrpRemotePort(25559)).toThrow("between 25560 and 25600");
    expect(() => validateFrpRemotePort(25601)).toThrow("between 25560 and 25600");
    expect(() => validateFrpRemotePort(25565)).not.toThrow();
  });
});
