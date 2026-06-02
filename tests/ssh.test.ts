import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { config } from "../src/server/config.js";
import { generateManagedSshPrivateKey, managedSshKeyStatus } from "../src/server/services/ssh.js";

let tempDir = "";
let originalManagedPath = "";

describe("managed SSH key generation", () => {
  beforeEach(async () => {
    tempDir = await fs.mkdtemp(path.join(os.tmpdir(), "yanto-ssh-"));
    originalManagedPath = config.managedSshPrivateKeyPath;
    config.managedSshPrivateKeyPath = path.join(tempDir, "id_ed25519");
  });

  afterEach(async () => {
    config.managedSshPrivateKeyPath = originalManagedPath;
    await fs.rm(tempDir, { recursive: true, force: true });
  });

  it("creates a managed key and returns public key status", async () => {
    const status = await generateManagedSshPrivateKey();

    expect(status.hasManagedKey).toBe(true);
    expect(status.activePrivateKeyPath).toBe(config.managedSshPrivateKeyPath);
    expect(status.publicKey).toMatch(/^ssh-ed25519 /);
    const stat = await fs.stat(config.managedSshPrivateKeyPath);
    expect(stat.mode & 0o777).toBe(0o600);
    await expect(fs.readFile(`${config.managedSshPrivateKeyPath}.pub`, "utf8")).resolves.toContain(status.publicKey ?? "");
  });

  it("does not overwrite an existing managed key", async () => {
    await generateManagedSshPrivateKey();
    const before = await fs.readFile(config.managedSshPrivateKeyPath, "utf8");

    await expect(generateManagedSshPrivateKey()).rejects.toThrow("Managed SSH key already exists.");
    await expect(fs.readFile(config.managedSshPrivateKeyPath, "utf8")).resolves.toBe(before);
  });

  it("reports generated key status", async () => {
    await generateManagedSshPrivateKey();

    await expect(managedSshKeyStatus()).resolves.toMatchObject({
      hasManagedKey: true,
      activePrivateKeyPath: config.managedSshPrivateKeyPath
    });
  });
});
