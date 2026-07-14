import { migrate, pool } from "../db/index.js";
import { recoverOwnerAccount } from "../services/accounts.js";

async function main() {
  await migrate();
  const resetUrl = await recoverOwnerAccount();
  process.stdout.write(`Owner sessions and password have been revoked.\nOpen this one-time link within 24 hours:\n${resetUrl}\n`);
}

main()
  .catch((error) => {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  })
  .finally(async () => {
    await pool.end();
  });
