import { describe, expect, it } from "vitest";
import { decrypt, encrypt, isEncrypted } from "../src/server/services/crypto.js";

describe("encrypted secret envelopes", () => {
  it("round-trips versioned ciphertext and recognizes legacy ciphertext", () => {
    const ciphertext = encrypt("sensitive-value");
    expect(ciphertext).toMatch(/^enc:v1:/);
    expect(isEncrypted(ciphertext)).toBe(true);
    expect(decrypt(ciphertext)).toBe("sensitive-value");

    const legacy = ciphertext.split(":").slice(2).join(":");
    expect(isEncrypted(legacy)).toBe(true);
    expect(decrypt(legacy)).toBe("sensitive-value");
  });

  it("does not mistake arbitrary colon-delimited text for ciphertext", () => {
    expect(isEncrypted("foo:bar:baz")).toBe(false);
    expect(() => decrypt("foo:bar:baz")).toThrow("Invalid encrypted format");
  });
});
