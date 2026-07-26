import { describe, expect, it, vi } from "vitest";
import { AUTHORITATIVE_BASELINE_AT, prepareLegacyMigrationHistory, type MigrationQueryable } from "../src/server/db/migration-compat.js";

function queryable(state: { projects_table: string | null; has_history: boolean }) {
  const query = vi.fn(async (sql: string) => {
    if (sql.includes("to_regclass")) return { rows: [state] };
    return { rows: [] };
  });
  return { query } as unknown as MigrationQueryable & { query: typeof query };
}

describe("legacy migration history compatibility", () => {
  it("baselines an existing handwritten-era database so only later migrations run", async () => {
    const client = queryable({ projects_table: "projects", has_history: false });

    await prepareLegacyMigrationHistory(client);

    expect(client.query).toHaveBeenCalledWith(
      expect.stringContaining("INSERT INTO drizzle.__drizzle_migrations"),
      ["legacy-handwritten-schema", AUTHORITATIVE_BASELINE_AT]
    );
  });

  it("leaves a clean database unmarked so the authoritative baseline runs", async () => {
    const client = queryable({ projects_table: null, has_history: false });

    await prepareLegacyMigrationHistory(client);

    expect(client.query.mock.calls.some(([sql]) => String(sql).includes("INSERT INTO drizzle.__drizzle_migrations"))).toBe(false);
  });

  it("preserves an existing Drizzle migration history", async () => {
    const client = queryable({ projects_table: "projects", has_history: true });

    await prepareLegacyMigrationHistory(client);

    expect(client.query.mock.calls.some(([sql]) => String(sql).includes("INSERT INTO drizzle.__drizzle_migrations"))).toBe(false);
  });
});
