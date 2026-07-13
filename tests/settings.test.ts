import { beforeEach, describe, expect, it, vi } from "vitest";

const dbMocks = vi.hoisted(() => ({
  select: vi.fn(),
  from: vi.fn(),
  where: vi.fn(),
  limit: vi.fn(),
  insert: vi.fn(),
  values: vi.fn(),
  onConflictDoUpdate: vi.fn()
}));

vi.mock("../src/server/db/index.js", () => ({
  db: {
    select: dbMocks.select,
    insert: dbMocks.insert
  }
}));

import { decrypt, encrypt, isEncrypted } from "../src/server/services/crypto.js";
import { getStoredR2Settings, publicMultiNodeSettings, saveMultiNodeSettings, saveR2Settings } from "../src/server/services/settings.js";

describe("multi-node settings", () => {
  beforeEach(() => {
    dbMocks.select.mockReset();
    dbMocks.from.mockReset();
    dbMocks.where.mockReset();
    dbMocks.limit.mockReset();
    dbMocks.insert.mockReset();
    dbMocks.values.mockReset();
    dbMocks.onConflictDoUpdate.mockReset();

    dbMocks.select.mockReturnValue({ from: dbMocks.from });
    dbMocks.from.mockReturnValue({ where: dbMocks.where });
    dbMocks.where.mockReturnValue({ limit: dbMocks.limit });
    dbMocks.insert.mockReturnValue({ values: dbMocks.values });
    dbMocks.values.mockReturnValue({ onConflictDoUpdate: dbMocks.onConflictDoUpdate });
    dbMocks.onConflictDoUpdate.mockResolvedValue(undefined);
  });

  it("defaults to off with beta release stage", async () => {
    dbMocks.limit.mockResolvedValue([]);

    await expect(publicMultiNodeSettings()).resolves.toEqual({
      enabled: false,
      releaseStage: "beta"
    });
  });

  it("falls back to off for malformed stored JSON", async () => {
    dbMocks.limit.mockResolvedValue([{ value: "not-json" }]);

    await expect(publicMultiNodeSettings()).resolves.toEqual({
      enabled: false,
      releaseStage: "beta"
    });
  });

  it("saves the enabled flag and returns the public shape", async () => {
    dbMocks.limit.mockResolvedValue([{ value: JSON.stringify({ enabled: true }) }]);

    await expect(saveMultiNodeSettings({ enabled: true })).resolves.toEqual({
      enabled: true,
      releaseStage: "beta"
    });
    expect(dbMocks.values).toHaveBeenCalledWith(expect.objectContaining({
      key: "feature.multi_node",
      value: JSON.stringify({ enabled: true }),
      updatedAt: expect.any(Date)
    }));
  });
});

describe("R2 credential storage", () => {
  beforeEach(() => {
    for (const mock of Object.values(dbMocks)) mock.mockReset();
    dbMocks.select.mockReturnValue({ from: dbMocks.from });
    dbMocks.from.mockReturnValue({ where: dbMocks.where });
    dbMocks.where.mockReturnValue({ limit: dbMocks.limit });
    dbMocks.insert.mockReturnValue({ values: dbMocks.values });
    dbMocks.values.mockReturnValue({ onConflictDoUpdate: dbMocks.onConflictDoUpdate });
    dbMocks.onConflictDoUpdate.mockResolvedValue(undefined);
  });

  it("reads both legacy plaintext and encrypted credentials", async () => {
    dbMocks.limit.mockResolvedValueOnce([{ value: JSON.stringify({
      enabled: true,
      accountId: "a".repeat(32),
      bucket: "backups",
      accessKeyId: "legacy-access",
      secretAccessKey: encrypt("encrypted-secret"),
      prefix: "dumps"
    }) }]);

    await expect(getStoredR2Settings()).resolves.toMatchObject({
      accessKeyId: "legacy-access",
      secretAccessKey: "encrypted-secret"
    });
  });

  it("encrypts both credentials when settings are saved", async () => {
    let persisted = "";
    dbMocks.limit.mockImplementation(async () => persisted ? [{ value: persisted }] : []);
    dbMocks.values.mockImplementation((row: { value: string }) => {
      persisted = row.value;
      return { onConflictDoUpdate: dbMocks.onConflictDoUpdate };
    });

    await saveR2Settings({
      enabled: true,
      accountId: "a".repeat(32),
      bucket: "backups",
      accessKeyId: "access-key",
      secretAccessKey: "secret-key",
      prefix: "dumps"
    });

    const stored = JSON.parse(persisted) as { accessKeyId: string; secretAccessKey: string };
    expect(isEncrypted(stored.accessKeyId)).toBe(true);
    expect(isEncrypted(stored.secretAccessKey)).toBe(true);
    expect(decrypt(stored.accessKeyId)).toBe("access-key");
    expect(decrypt(stored.secretAccessKey)).toBe("secret-key");
    expect(persisted).not.toContain('"accessKeyId":"access-key"');
    expect(persisted).not.toContain('"secretAccessKey":"secret-key"');
  });
});
