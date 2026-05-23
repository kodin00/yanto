import { beforeEach, describe, expect, it, vi } from "vitest";

const dbMocks = vi.hoisted(() => ({
  update: vi.fn(),
  set: vi.fn(),
  where: vi.fn(),
  returning: vi.fn()
}));

vi.mock("../src/server/db/index.js", () => ({
  db: {
    update: dbMocks.update
  }
}));

vi.mock("../src/server/logger.js", () => ({
  logger: {
    error: vi.fn(),
    info: vi.fn(),
    warn: vi.fn()
  }
}));

import { recoverInterruptedDeployments } from "../src/server/services/deployments.js";

describe("deployment recovery", () => {
  beforeEach(() => {
    dbMocks.update.mockReset();
    dbMocks.set.mockReset();
    dbMocks.where.mockReset();
    dbMocks.returning.mockReset();

    dbMocks.update.mockReturnValue({ set: dbMocks.set });
    dbMocks.set.mockReturnValue({ where: dbMocks.where });
    dbMocks.where.mockReturnValue({ returning: dbMocks.returning });
  });

  it("marks running deployments as failed after a server restart", async () => {
    dbMocks.returning.mockResolvedValue([{ id: "dep_1" }, { id: "dep_2" }]);

    await expect(recoverInterruptedDeployments()).resolves.toBe(2);

    expect(dbMocks.update).toHaveBeenCalledTimes(1);
    expect(dbMocks.set).toHaveBeenCalledWith(
      expect.objectContaining({
        status: "failed",
        exitCode: 1,
        finishedAt: expect.any(Date)
      })
    );
    expect(dbMocks.where).toHaveBeenCalledTimes(1);
    expect(dbMocks.returning).toHaveBeenCalledTimes(1);
  });
});
