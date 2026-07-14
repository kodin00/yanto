import crypto from "node:crypto";
import bcrypt from "bcryptjs";

const SCRYPT_KEY_LENGTH = 64;
const SCRYPT_N = 16_384;
const SCRYPT_R = 8;
const SCRYPT_P = 1;

function scrypt(password: string, salt: Buffer, keyLength: number, options: crypto.ScryptOptions) {
  return new Promise<Buffer>((resolve, reject) => {
    crypto.scrypt(password, salt, keyLength, options, (error, derivedKey) => {
      if (error) reject(error);
      else resolve(derivedKey);
    });
  });
}

export async function hashPassword(password: string) {
  if (password.length === 0) {
    throw new Error("Password cannot be empty.");
  }
  const salt = crypto.randomBytes(16);
  const derived = await scrypt(password, salt, SCRYPT_KEY_LENGTH, {
    N: SCRYPT_N,
    r: SCRYPT_R,
    p: SCRYPT_P
  });
  return `scrypt$${SCRYPT_N}$${SCRYPT_R}$${SCRYPT_P}$${salt.toString("base64url")}$${derived.toString("base64url")}`;
}

export async function verifyPassword(password: string, encoded: string) {
  if (encoded.startsWith("$2")) {
    return bcrypt.compare(password, encoded);
  }
  const [algorithm, nRaw, rRaw, pRaw, saltRaw, hashRaw] = encoded.split("$");
  if (algorithm !== "scrypt" || !nRaw || !rRaw || !pRaw || !saltRaw || !hashRaw) {
    return false;
  }
  const N = Number(nRaw);
  const r = Number(rRaw);
  const p = Number(pRaw);
  if (!Number.isInteger(N) || !Number.isInteger(r) || !Number.isInteger(p) || N < 2 || r < 1 || p < 1) {
    return false;
  }
  try {
    const expected = Buffer.from(hashRaw, "base64url");
    const actual = await scrypt(password, Buffer.from(saltRaw, "base64url"), expected.length, { N, r, p });
    return actual.byteLength === expected.byteLength && crypto.timingSafeEqual(actual, expected);
  } catch {
    return false;
  }
}

export function passwordHashNeedsUpgrade(encoded: string) {
  return !encoded.startsWith("scrypt$");
}
