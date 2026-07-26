import { describe, expect, it, vi } from "vitest";
import { runRoutePublicationLifecycle } from "../src/server/services/cloudflare-route-publication.js";

function lifecycle(overrides: Partial<Parameters<typeof runRoutePublicationLifecycle>[0]> = {}) {
  return {
    publishDns: vi.fn().mockResolvedValue("dns_1"),
    publishIngress: vi.fn().mockResolvedValue(undefined),
    prepareRuntime: vi.fn().mockResolvedValue(undefined),
    cleanupPrevious: vi.fn().mockResolvedValue(undefined),
    markActive: vi.fn().mockResolvedValue(undefined),
    markError: vi.fn().mockResolvedValue(undefined),
    ...overrides
  };
}

describe("Cloudflare route publication lifecycle", () => {
  it("activates only after DNS, ingress, runtime, and old-DNS cleanup succeed", async () => {
    const steps = lifecycle();

    await runRoutePublicationLifecycle(steps);

    expect(steps.markActive).toHaveBeenCalledWith("dns_1");
    expect(steps.markError).not.toHaveBeenCalled();
    expect(steps.publishDns.mock.invocationCallOrder[0]).toBeLessThan(steps.publishIngress.mock.invocationCallOrder[0]);
    expect(steps.publishIngress.mock.invocationCallOrder[0]).toBeLessThan(steps.prepareRuntime.mock.invocationCallOrder[0]);
    expect(steps.prepareRuntime.mock.invocationCallOrder[0]).toBeLessThan(steps.cleanupPrevious.mock.invocationCallOrder[0]);
    expect(steps.cleanupPrevious.mock.invocationCallOrder[0]).toBeLessThan(steps.markActive.mock.invocationCallOrder[0]);
  });

  it.each(["publishDns", "publishIngress", "prepareRuntime", "cleanupPrevious"] as const)(
    "records a recoverable error when %s fails",
    async (failedStep) => {
      const failure = new Error(`${failedStep} failed`);
      const steps = lifecycle({ [failedStep]: vi.fn().mockRejectedValue(failure) });

      await expect(runRoutePublicationLifecycle(steps)).rejects.toBe(failure);

      expect(steps.markActive).not.toHaveBeenCalled();
      expect(steps.markError).toHaveBeenCalledWith(failure.message, failedStep === "publishDns" ? null : "dns_1");
    }
  );

  it("does not hide the publication failure when recording lastError also fails", async () => {
    const failure = new Error("DNS failed");
    const steps = lifecycle({
      publishDns: vi.fn().mockRejectedValue(failure),
      markError: vi.fn().mockRejectedValue(new Error("database unavailable"))
    });

    await expect(runRoutePublicationLifecycle(steps)).rejects.toBe(failure);
  });
});
