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
  const maxLen = Math.max(left.length, right.length);
  const paddedLeft = Buffer.alloc(maxLen);
  const paddedRight = Buffer.alloc(maxLen);
  left.copy(paddedLeft);
  right.copy(paddedRight);
  const contentsEqual = crypto.timingSafeEqual(paddedLeft, paddedRight);
  const lengthEqual = left.length === right.length;
  return contentsEqual && lengthEqual;
}
