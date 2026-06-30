import { beforeEach, describe, expect, it, vi } from "vitest";

const dbMocks = vi.hoisted(() => ({
  select: vi.fn(),
  insert: vi.fn(),
  update: vi.fn(),
  from: vi.fn(),
  where: vi.fn(),
  limit: vi.fn(),
  values: vi.fn(),
  set: vi.fn(),
  returning: vi.fn()
}));

vi.mock("../src/server/db/index.js", () => ({
  db: {
    select: dbMocks.select,
    insert: dbMocks.insert,
    update: dbMocks.update
  }
}));

import { authenticateMcpToken, createMcpAccessToken, hasMcpAccess, parseMcpToken, revokeMcpAccessToken } from "../src/server/services/mcp-tokens.js";

describe("mcp access tokens", () => {
  beforeEach(() => {
    for (const mock of Object.values(dbMocks)) {
      mock.mockReset();
    }
    dbMocks.insert.mockReturnValue({ values: dbMocks.values });
    dbMocks.values.mockReturnValue({ returning: dbMocks.returning });
    dbMocks.select.mockReturnValue({ from: dbMocks.from });
    dbMocks.from.mockReturnValue({ where: dbMocks.where });
    dbMocks.where.mockReturnValue({ limit: dbMocks.limit, returning: dbMocks.returning });
    dbMocks.update.mockReturnValue({ set: dbMocks.set });
    dbMocks.set.mockReturnValue({ where: dbMocks.where });
  });

  it("creates copy-once tokens and stores only a hash", async () => {
    dbMocks.returning.mockImplementation(async () => {
      const stored = dbMocks.values.mock.calls.at(-1)?.[0];
      return [{ ...stored }];
    });

    const result = await createMcpAccessToken({ name: "Codex", accessLevel: "write" });

    expect(result.token).toMatch(/^ymcp_[a-f0-9]{24}_[A-Za-z0-9_-]+$/);
    expect(result.accessToken).toMatchObject({ name: "Codex", accessLevel: "write", revokedAt: null, lastUsedAt: null });
    const stored = dbMocks.values.mock.calls[0][0] as { tokenHash: string };
    expect(stored.tokenHash).not.toContain(result.token);
    expect(result).not.toHaveProperty("tokenHash");
  });

  it("authenticates by id and hash, updates last use, and returns an audit actor", async () => {
    dbMocks.returning.mockImplementation(async () => [{ ...dbMocks.values.mock.calls.at(-1)?.[0] }]);
    const created = await createMcpAccessToken({ name: "Agent", accessLevel: "admin" });
    const stored = dbMocks.values.mock.calls[0][0];
    dbMocks.limit.mockResolvedValueOnce([{ ...stored }]);

    await expect(authenticateMcpToken(created.token)).resolves.toEqual({
      tokenId: stored.id,
      tokenName: "Agent",
      accessLevel: "admin",
      actor: "mcp:Agent"
    });
    expect(dbMocks.update).toHaveBeenCalled();
  });

  it("rejects revoked or malformed tokens", async () => {
    expect(parseMcpToken("not-a-token")).toBeNull();
    dbMocks.limit.mockResolvedValueOnce([]);
    await expect(authenticateMcpToken("ymcp_deadbeef_secret")).resolves.toBeNull();
  });

  it("revokes by timestamp and preserves hierarchical access checks", async () => {
    dbMocks.returning.mockResolvedValueOnce([
      { id: "ymcp_one", name: "One", accessLevel: "read", tokenHash: "hash", lastUsedAt: null, revokedAt: new Date(), createdAt: new Date(), updatedAt: new Date() }
    ]);

    await expect(revokeMcpAccessToken("ymcp_one")).resolves.toMatchObject({ id: "ymcp_one", revokedAt: expect.any(String) });
    expect(hasMcpAccess("write", "read")).toBe(true);
    expect(hasMcpAccess("read", "write")).toBe(false);
    expect(hasMcpAccess("admin", "write")).toBe(true);
  });
});
