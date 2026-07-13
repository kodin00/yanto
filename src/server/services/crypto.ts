import { createCipheriv, createDecipheriv, randomBytes, scryptSync } from "node:crypto";
import { config } from "../config.js";

const ALGORITHM = "aes-256-gcm";
const IV_LENGTH = 16;
const AUTH_TAG_LENGTH = 16;
const ENVELOPE_PREFIX = "enc:v1";
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
  return `${ENVELOPE_PREFIX}:${iv.toString("base64")}:${tag.toString("base64")}:${encrypted.toString("base64")}`;
}

function decodeBase64(value: string, expectedBytes?: number) {
  if (!value || !/^[A-Za-z0-9+/]+={0,2}$/.test(value)) return null;
  const decoded = Buffer.from(value, "base64");
  if (decoded.toString("base64") !== value || (expectedBytes !== undefined && decoded.byteLength !== expectedBytes)) {
    return null;
  }
  return decoded;
}

function encryptedParts(ciphertext: string) {
  const parts = ciphertext.split(":");
  const encoded = parts[0] === "enc" && parts[1] === "v1" && parts.length === 5
    ? parts.slice(2)
    : parts.length === 3
      ? parts
      : [];
  if (encoded.length !== 3) return null;
  const iv = decodeBase64(encoded[0], IV_LENGTH);
  const tag = decodeBase64(encoded[1], AUTH_TAG_LENGTH);
  const data = decodeBase64(encoded[2]);
  return iv && tag && data ? { iv, tag, data } : null;
}

export function decrypt(ciphertext: string): string {
  const parts = encryptedParts(ciphertext);
  if (!parts) throw new Error("Invalid encrypted format");
  const { iv, tag, data } = parts;
  const decipher = createDecipheriv(ALGORITHM, getKey(), iv);
  decipher.setAuthTag(tag);
  return Buffer.concat([decipher.update(data), decipher.final()]).toString("utf8");
}

export function isEncrypted(value: string): boolean {
  return encryptedParts(value) !== null;
}
