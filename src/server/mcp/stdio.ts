import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { config, warnOnUnsafeDefaults } from "../config.js";
import { migrate, pool } from "../db/index.js";
import { ensureLocalMasterNode } from "../services/nodes.js";
import { authenticateMcpToken } from "../services/mcp-tokens.js";
import { createYantoMcpServer } from "./registry.js";

async function main() {
  warnOnUnsafeDefaults();
  if (config.nodeRole !== "master") {
    throw new Error("Yanto MCP stdio is only available for the master role.");
  }
  const token = process.env.YANTO_MCP_TOKEN ?? "";
  if (!token) {
    throw new Error("Set YANTO_MCP_TOKEN to a valid MCP access token.");
  }

  await migrate();
  await ensureLocalMasterNode();
  const auth = await authenticateMcpToken(token);
  if (!auth) {
    throw new Error("Invalid MCP token.");
  }

  const server = createYantoMcpServer(auth);
  const transport = new StdioServerTransport();
  await server.connect(transport);

  const shutdown = async () => {
    await server.close();
    await pool.end();
    process.exit(0);
  };
  process.on("SIGINT", () => void shutdown());
  process.on("SIGTERM", () => void shutdown());
}

main().catch(async (error) => {
  console.error(error instanceof Error ? error.message : String(error));
  await pool.end().catch(() => undefined);
  process.exit(1);
});
