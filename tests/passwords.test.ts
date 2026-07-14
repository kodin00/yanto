import bcrypt from "bcryptjs";
import { describe, expect, it } from "vitest";
import { hashPassword, passwordHashNeedsUpgrade, verifyPassword } from "../src/server/services/passwords.js";

describe("account password hashing", () => {
  it("round-trips long Unicode passwords without trimming", async () => {
    const password = `  密碼-${"🙂".repeat(5000)}  `;
    const hash = await hashPassword(password);
    expect(hash).toMatch(/^scrypt\$/);
    expect(await verifyPassword(password, hash)).toBe(true);
    expect(await verifyPassword(password.trim(), hash)).toBe(false);
    expect(passwordHashNeedsUpgrade(hash)).toBe(false);
  });

  it("rejects empty passwords", async () => {
    await expect(hashPassword("")).rejects.toThrow("Password cannot be empty");
  });

  it("verifies legacy bcrypt hashes and marks them for upgrade", async () => {
    const hash = await bcrypt.hash("legacy-password", 4);
    expect(await verifyPassword("legacy-password", hash)).toBe(true);
    expect(await verifyPassword("wrong", hash)).toBe(false);
    expect(passwordHashNeedsUpgrade(hash)).toBe(true);
  });
});
