import { Router } from "express";
import type express from "express";
import { StreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/streamableHttp.js";
import { config } from "../config.js";
import { logger } from "../logger.js";
import { authenticateMcpToken, bearerTokenFromHeader } from "../services/mcp-tokens.js";
import { createYantoMcpServer } from "./registry.js";

const router = Router();

function splitCsv(value: string) {
  return value
    .split(",")
    .map((entry) => entry.trim())
    .filter(Boolean);
}

function appBaseOrigin() {
  try {
    return new URL(config.appBaseUrl).origin;
  } catch {
    return "";
  }
}

function appBaseHost() {
  try {
    return new URL(config.appBaseUrl).host;
  } catch {
    return "";
  }
}

function allowedHosts() {
  return new Set(["localhost", "localhost:8080", "127.0.0.1", "127.0.0.1:8080", "[::1]", "[::1]:8080", appBaseHost(), ...splitCsv(config.mcpAllowedHosts)].filter(Boolean));
}

function allowedOrigins() {
  return new Set([appBaseOrigin(), "http://localhost:8080", "http://127.0.0.1:8080", ...splitCsv(config.mcpAllowedOrigins)].filter(Boolean));
}

function validateHostAndOrigin(req: express.Request) {
  const host = req.header("host") ?? "";
  if (!allowedHosts().has(host)) {
    return false;
  }
  const origin = req.header("origin");
  if (origin && !allowedOrigins().has(origin)) {
    return false;
  }
  return true;
}

router.post("/mcp", async (req, res) => {
  try {
    if (config.nodeRole !== "master") {
      res.status(404).json({ message: "MCP is only available on the master process." });
      return;
    }
    if (!validateHostAndOrigin(req)) {
      res.status(403).json({ message: "MCP host/origin rejected." });
      return;
    }
    const bearerToken = bearerTokenFromHeader(req.header("authorization"));
    if (!bearerToken) {
      res.status(401).json({ message: "MCP requires Authorization: Bearer <token>." });
      return;
    }
    const auth = await authenticateMcpToken(bearerToken);
    if (!auth) {
      res.status(401).json({ message: "Invalid MCP token." });
      return;
    }

    const server = createYantoMcpServer(auth);
    const transport = new StreamableHTTPServerTransport({
      sessionIdGenerator: undefined,
      enableJsonResponse: true
    });
    try {
      await server.connect(transport);
      await transport.handleRequest(req, res, req.body);
    } finally {
      await server.close();
    }
  } catch (error) {
    logger.error("mcp request failed", { error: error instanceof Error ? error.message : String(error) });
    if (!res.headersSent) {
      res.status(500).json({ message: config.nodeEnv === "production" ? "MCP request failed." : error instanceof Error ? error.message : "MCP request failed." });
    }
  }
});

router.all("/mcp", (_req, res) => {
  res.status(405).json({ message: "Use POST for stateless Streamable HTTP MCP requests." });
});

export default router;
