import { describe, expect, it } from "vitest";
import { classifyRouteDiagnostic } from "../src/server/services/cloudflare-diagnostics.js";
import type { CloudflareRouteDiagnosticDnsRecord } from "../src/shared/types.js";

const expectedDnsTarget = "tun_123.cfargotunnel.com";
const hostname = "app.example.com";

function cname(content = expectedDnsTarget): CloudflareRouteDiagnosticDnsRecord {
  return { id: "dns_1", type: "CNAME", name: hostname, content, proxied: true };
}

function classify(patch: Partial<Parameters<typeof classifyRouteDiagnostic>[0]> = {}) {
  return classifyRouteDiagnostic({
    routeEnabled: true,
    hostname,
    expectedDnsTarget,
    dnsRecords: [cname()],
    tunnelExists: true,
    tunnelRuntimeRunning: true,
    tunnelHealthy: true,
    reachabilityStatus: "ok",
    ...patch
  });
}

describe("classifyRouteDiagnostic", () => {
  it("classifies healthy routes as ok", () => {
    const result = classify();

    expect(result.dnsStatus).toBe("ok");
    expect(result.tunnelStatus).toBe("running");
    expect(result.reachabilityStatus).toBe("ok");
  });

  it("detects missing DNS records", () => {
    expect(classify({ dnsRecords: [] }).dnsStatus).toBe("missing");
  });

  it("detects mismatched CNAME targets", () => {
    expect(classify({ dnsRecords: [cname("elsewhere.example.com")] }).dnsStatus).toBe("mismatch");
  });

  it("detects conflicting same-name records", () => {
    expect(classify({ dnsRecords: [{ id: "dns_2", type: "A", name: hostname, content: "203.0.113.10", proxied: true }] }).dnsStatus).toBe("conflict");
  });

  it("detects stopped tunnels", () => {
    expect(classify({ tunnelRuntimeRunning: false }).tunnelStatus).toBe("stopped");
  });

  it("detects unhealthy tunnels", () => {
    expect(classify({ tunnelHealthy: false }).tunnelStatus).toBe("unhealthy");
  });

  it("preserves unreachable hostname failures", () => {
    const result = classify({ reachabilityStatus: "failed" });

    expect(result.reachabilityStatus).toBe("failed");
    expect(result.messages).toContain("Hostname is not reachable over HTTPS.");
  });
});
