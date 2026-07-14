import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => {
  const client = { query: vi.fn(), release: vi.fn() };
  const userRow = {
    id: "usr_member",
    username: "member",
    role: "member",
    status: "disabled",
    passwordHash: "hash",
    sessionVersion: 2,
    lastLoginAt: null,
    createdAt: new Date("2026-01-01T00:00:00Z"),
    updatedAt: new Date("2026-01-01T00:00:00Z")
  };
  const grantBuilder: Record<string, unknown> = {};
  grantBuilder.where = vi.fn(() => grantBuilder);
  grantBuilder.orderBy = vi.fn(async () => []);
  const userBuilder: Record<string, unknown> = {};
  userBuilder.from = vi.fn(() => userBuilder);
  userBuilder.where = vi.fn(() => userBuilder);
  userBuilder.limit = vi.fn(async () => [userRow]);
  userBuilder.innerJoin = vi.fn(() => grantBuilder);
  return {
    client,
    userRow,
    pool: { connect: vi.fn(async () => client) },
    db: { select: vi.fn(() => userBuilder) }
  };
});

vi.mock("../src/server/db/index.js", () => ({ db: mocks.db, pool: mocks.pool }));
vi.mock("../src/server/services/passwords.js", () => ({
  hashPassword: vi.fn(async () => "scrypt$test"),
  verifyPassword: vi.fn(async () => true),
  passwordHashNeedsUpgrade: vi.fn(() => false)
}));
vi.mock("../src/server/logger.js", () => ({ logger: { warn: vi.fn() } }));

import { completeAccountSetup, setUserStatus } from "../src/server/services/accounts.js";

describe("disabled account token revocation", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.userRow.status = "disabled";
  });

  it("revokes every unused setup/reset token when a member is disabled", async () => {
    mocks.client.query.mockImplementation(async (query: string) => {
      if (query.includes("SELECT role, password_hash")) return { rows: [{ role: "member", password_hash: "hash" }] };
      return { rows: [], rowCount: 0 };
    });

    await setUserStatus("usr_member", "disabled");

    expect(mocks.client.query).toHaveBeenCalledWith(
      expect.stringContaining("UPDATE account_tokens SET used_at = now()"),
      ["usr_member"]
    );
  });

  it("does not let an old link reactivate a disabled member", async () => {
    mocks.client.query.mockImplementation(async (query: string) => {
      if (query.includes("SELECT user_id FROM account_tokens")) return { rows: [{ user_id: "usr_member" }] };
      if (query.includes("SELECT status FROM users")) return { rows: [{ status: "disabled" }] };
      return { rows: [], rowCount: 0 };
    });

    await expect(completeAccountSetup("old-reset-link", "new password")).rejects.toMatchObject({ status: 400 });

    expect(mocks.client.query).toHaveBeenCalledWith("ROLLBACK");
    expect(mocks.client.query).not.toHaveBeenCalledWith(expect.stringContaining("UPDATE users SET password_hash"), expect.anything());
  });
});
