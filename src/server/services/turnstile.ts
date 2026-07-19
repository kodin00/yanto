import crypto from "node:crypto";
import type { Request } from "express";
import { config } from "../config.js";
import { HttpError } from "../http-utils.js";

const verifyUrl = "https://challenges.cloudflare.com/turnstile/v0/siteverify";

type TurnstileResult = {
  success?: boolean;
  action?: string;
};

export function turnstileEnabled() {
  return Boolean(config.turnstileSiteKey && config.turnstileSecretKey);
}

function visitorIp(req: Request) {
  // The app is normally reachable only through a reverse proxy. Cloudflare sets
  // this header at the edge; omit remoteip when it is not available.
  return req.header("cf-connecting-ip")?.trim() || undefined;
}

export async function verifyTurnstileToken(req: Request, token: string | undefined, action: string) {
  if (!turnstileEnabled()) return;
  if (!token) throw new HttpError(400, "Complete the security check before continuing.");

  let response: Response;
  try {
    response = await fetch(verifyUrl, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        secret: config.turnstileSecretKey,
        response: token,
        remoteip: visitorIp(req),
        idempotency_key: crypto.randomUUID()
      }),
      signal: AbortSignal.timeout(10_000)
    });
  } catch {
    throw new HttpError(503, "The security check is temporarily unavailable. Please try again.");
  }

  const result = await response.json().catch(() => null) as TurnstileResult | null;
  if (!response.ok || !result?.success || result.action !== action) {
    throw new HttpError(400, "Security check failed. Please try again.");
  }
}
