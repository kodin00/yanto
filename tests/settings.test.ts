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

import { publicMultiNodeSettings, saveMultiNodeSettings } from "../src/server/services/settings.js";

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
