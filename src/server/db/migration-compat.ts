export const AUTHORITATIVE_BASELINE_AT = 1779550654157;

type QueryResult<Row> = { rows: Row[] };
export type MigrationQueryable = {
  query<Row extends Record<string, unknown> = Record<string, unknown>>(text: string, values?: unknown[]): Promise<QueryResult<Row>>;
};

/**
 * Handwritten-era databases have the schema but no Drizzle history. Mark only the
 * consolidated baseline as applied, allowing the checked-in compatibility bridge
 * and every future migration to run normally.
 */
export async function prepareLegacyMigrationHistory(client: MigrationQueryable) {
  await client.query(`CREATE SCHEMA IF NOT EXISTS drizzle`);
  await client.query(`
    CREATE TABLE IF NOT EXISTS drizzle.__drizzle_migrations (
      id serial PRIMARY KEY,
      hash text NOT NULL,
      created_at bigint
    )
  `);
  const result = await client.query<{ projects_table: string | null; has_history: boolean }>(`
    SELECT
      to_regclass('public.projects')::text AS projects_table,
      EXISTS (SELECT 1 FROM drizzle.__drizzle_migrations) AS has_history
  `);
  const state = result.rows[0];
  if (state?.projects_table && !state.has_history) {
    await client.query(
      `INSERT INTO drizzle.__drizzle_migrations (hash, created_at) VALUES ($1, $2)`,
      ["legacy-handwritten-schema", AUTHORITATIVE_BASELINE_AT]
    );
  }
}
