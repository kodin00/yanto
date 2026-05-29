import { createCipheriv, createDecipheriv, randomBytes, scryptSync } from "node:crypto";
import { config } from "../config.js";

const ALGORITHM = "aes-256-gcm";
const IV_LENGTH = 16;
const SALT = "yanto-tunnel-token-salt";

let derivedKey: Buffer | null = null;

function getKey(): Buffer {
  if (!derivedKey) {
    derivedKey = scryptSync(config.jwtSecret, SALT, 32);
  }
  return derivedKey;
}

export function encrypt(plaintext: string): string {
  const iv = randomBytes(IV_LENGTH);
  const cipher = createCipheriv(ALGORITHM, getKey(), iv);
  const encrypted = Buffer.concat([cipher.update(plaintext, "utf8"), cipher.final()]);
  const tag = cipher.getAuthTag();
  return `${iv.toString("base64")}:${tag.toString("base64")}:${encrypted.toString("base64")}`;
}

export function decrypt(ciphertext: string): string {
  const [ivB64, tagB64, dataB64] = ciphertext.split(":");
  if (!ivB64 || !tagB64 || !dataB64) {
    throw new Error("Invalid encrypted format");
  }
  const iv = Buffer.from(ivB64, "base64");
  const tag = Buffer.from(tagB64, "base64");
  const data = Buffer.from(dataB64, "base64");
  const decipher = createDecipheriv(ALGORITHM, getKey(), iv);
  decipher.setAuthTag(tag);
  return decipher.update(data) + decipher.final("utf8");
}

export function isEncrypted(value: string): boolean {
  const parts = value.split(":");
  return parts.length === 3 && parts.every((p) => /^[A-Za-z0-9+/=]+$/.test(p));
}
