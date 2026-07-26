import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { drizzle } from "drizzle-orm/node-postgres";
import { migrate as runDrizzleMigrations } from "drizzle-orm/node-postgres/migrator";
import pg from "pg";
import { config } from "../config.js";
import { encrypt, isEncrypted } from "../services/crypto.js";
import { hashPassword } from "../services/passwords.js";
import { createId } from "../services/tokens.js";
import { prepareLegacyMigrationHistory } from "./migration-compat.js";
import * as schema from "./schema.js";

export const pool = new pg.Pool({
  connectionString: config.databaseUrl,
  max: 20,
  idleTimeoutMillis: 30_000,
  connectionTimeoutMillis: 5_000
});

export const db = drizzle(pool, { schema });

function migrationsFolder() {
  const candidates = [
    fileURLToPath(new URL("../../../drizzle", import.meta.url)),
    fileURLToPath(new URL("../../../../drizzle", import.meta.url)),
    path.resolve(process.cwd(), "drizzle")
  ];
  const folder = candidates.find((candidate) => fs.existsSync(path.join(candidate, "meta", "_journal.json")));
  if (!folder) throw new Error("Unable to locate the checked-in Drizzle migrations folder.");
  return folder;
}

async function runDataBackfills(client: pg.PoolClient) {
  const legacySetting = await client.query<{ value: string }>(`SELECT value FROM app_settings WHERE key = 'cloudflare.tunnel' LIMIT 1`);
  if (legacySetting.rows[0]) {
    try {
      const legacy = JSON.parse(legacySetting.rows[0].value) as { accountId?: string; zoneId?: string; apiToken?: string };
      if (legacy.accountId && legacy.apiToken) {
        const token = isEncrypted(legacy.apiToken) ? legacy.apiToken : encrypt(legacy.apiToken);
        await client.query(
          `INSERT INTO cloudflare_clients (id, name, account_id, zone_id, api_token) VALUES ('cfc_legacy', 'Default Cloudflare', $1, $2, $3) ON CONFLICT (account_id) DO UPDATE SET zone_id = COALESCE(NULLIF(cloudflare_clients.zone_id, ''), EXCLUDED.zone_id)`,
          [legacy.accountId, legacy.zoneId ?? "", token]
        );
      }
    } catch {
      // Keep malformed legacy settings untouched for manual recovery.
    }
  }

  if (config.legacyAdminUsername && config.legacyAdminPassword) {
    const existing = await client.query<{ count: string }>(`SELECT count(*)::text AS count FROM users`);
    if (existing.rows[0]?.count === "0") {
      const passwordHash = config.legacyAdminPassword.startsWith("$2")
        ? config.legacyAdminPassword
        : await hashPassword(config.legacyAdminPassword);
      await client.query(
        `INSERT INTO users (id, username, role, status, password_hash) VALUES ($1, $2, 'owner', 'active', $3) ON CONFLICT DO NOTHING`,
        [createId("usr"), config.legacyAdminUsername, passwordHash]
      );
    }
  }
}

export async function migrate() {
  const client = await pool.connect();
  try {
    await client.query(`SELECT pg_advisory_lock(hashtext('yanto.schema.migrations'))`);
    await prepareLegacyMigrationHistory(client);
    await runDrizzleMigrations(drizzle(client, { schema }), { migrationsFolder: migrationsFolder() });
    await runDataBackfills(client);
  } finally {
    await client.query(`SELECT pg_advisory_unlock(hashtext('yanto.schema.migrations'))`).catch(() => undefined);
    client.release();
  }
}
