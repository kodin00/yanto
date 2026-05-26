import crypto from "node:crypto";

export function createId(prefix: string) {
  return `${prefix}_${crypto.randomBytes(12).toString("hex")}`;
}

export function createDeployToken() {
  return `ydp_${crypto.randomBytes(32).toString("base64url")}`;
}

export function createWorkerToken() {
  return `ywk_${crypto.randomBytes(32).toString("base64url")}`;
}

export function createWorkerJoinToken() {
  return `ywj_${crypto.randomBytes(32).toString("base64url")}`;
}

export function hashToken(token: string, secret = "") {
  return secret ? crypto.createHmac("sha256", secret).update(token).digest("hex") : crypto.createHash("sha256").update(token).digest("hex");
}

export function constantTimeEqual(a: string, b: string) {
  const left = Buffer.from(a);
  const right = Buffer.from(b);
  if (left.length !== right.length) {
    return false;
  }
  return crypto.timingSafeEqual(left, right);
}
