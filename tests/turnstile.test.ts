import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Request } from "express";
import { HttpError } from "../src/server/http-utils.js";

const configMock = vi.hoisted(() => ({
  turnstileSiteKey: "site-key",
  turnstileSecretKey: "secret-key"
}));

vi.mock("../src/server/config.js", () => ({ config: configMock }));

import { turnstileEnabled, verifyTurnstileToken } from "../src/server/services/turnstile.js";

function request(ip = "203.0.113.10") {
  return { header: vi.fn((name: string) => name === "cf-connecting-ip" ? ip : undefined) } as unknown as Request;
}

describe("Turnstile verification", () => {
  beforeEach(() => {
    configMock.turnstileSiteKey = "site-key";
    configMock.turnstileSecretKey = "secret-key";
    vi.restoreAllMocks();
  });

  it("posts the token, expected action context, and Cloudflare visitor IP to Siteverify", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify({ success: true, action: "login" }), { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(verifyTurnstileToken(request(), "challenge-token", "login")).resolves.toBeUndefined();

    expect(fetchMock).toHaveBeenCalledWith(
      "https://challenges.cloudflare.com/turnstile/v0/siteverify",
      expect.objectContaining({ method: "POST" })
    );
    const [, options] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(JSON.parse(String(options.body))).toEqual(expect.objectContaining({
      secret: "secret-key",
      response: "challenge-token",
      remoteip: "203.0.113.10"
    }));
  });

  it("rejects missing, invalid, and wrong-action tokens", async () => {
    await expect(verifyTurnstileToken(request(), undefined, "login")).rejects.toEqual(new HttpError(400, "Complete the security check before continuing."));
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(JSON.stringify({ success: true, action: "owner_setup" }), { status: 200 })));
    await expect(verifyTurnstileToken(request(), "challenge-token", "login")).rejects.toEqual(new HttpError(400, "Security check failed. Please try again."));
  });

  it("does not require Turnstile when either deployment key is absent", async () => {
    configMock.turnstileSecretKey = "";
    expect(turnstileEnabled()).toBe(false);
    await expect(verifyTurnstileToken(request(), undefined, "login")).resolves.toBeUndefined();
  });
});
