import path from "node:path";
import { fileURLToPath } from "node:url";
import { readMigrationFiles } from "drizzle-orm/migrator";
import { describe, expect, it } from "vitest";

const migrationsFolder = fileURLToPath(new URL("../drizzle", import.meta.url));

describe("authoritative Drizzle migration artifacts", () => {
  it("loads a complete clean-install baseline followed by the legacy bridge", () => {
    const migrations = readMigrationFiles({ migrationsFolder });

    expect(migrations).toHaveLength(2);
    expect(migrations[0].folderMillis).toBeLessThan(migrations[1].folderMillis);
    const baseline = migrations[0].sql.join("\n");
    for (const table of [
      "projects",
      "deployment_nodes",
      "deployments",
      "cloudflare_clients",
      "cloudflare_tunnels",
      "cloudflare_routes",
      "users",
      "agent_tasks"
    ]) {
      expect(baseline).toContain(`CREATE TABLE "${table}"`);
    }
  });

  it("keeps the upgrade bridge idempotent for handwritten-era schemas", () => {
    const compatibility = readMigrationFiles({ migrationsFolder })[1].sql.join("\n");

    expect(compatibility).toContain("CREATE TABLE IF NOT EXISTS deployment_nodes");
    expect(compatibility).toContain("ALTER TABLE projects ADD COLUMN IF NOT EXISTS docker_image");
    expect(compatibility).toContain("ALTER TABLE cloudflare_routes ADD COLUMN IF NOT EXISTS sync_status");
    expect(compatibility).toContain("CREATE TABLE IF NOT EXISTS users");
    expect(path.basename(migrationsFolder)).toBe("drizzle");
  });
});
