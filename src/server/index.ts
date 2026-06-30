import cookieParser from "cookie-parser";
import crypto from "node:crypto";
import express from "express";
import helmet from "helmet";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { z } from "zod";
import { HttpError } from "./http-utils.js";
import { config, warnOnUnsafeDefaults } from "./config.js";
import { migrate, pool } from "./db/index.js";
import { logger } from "./logger.js";
import { ensureLocalMasterNode } from "./services/nodes.js";
import { recoverInterruptedDeployments } from "./services/deployments.js";
import { ensureEnabledCloudflaredConnectors, reconcileTunnelAssignments } from "./services/cloudflare.js";

import authRouter from "./routes/auth.js";
import projectsRouter from "./routes/projects.js";
import deploymentsRouter from "./routes/deployments.js";
import containersRouter from "./routes/containers.js";
import backupsRouter from "./routes/backups.js";
import workersRouter from "./routes/workers.js";
import settingsRouter from "./routes/settings.js";
import cloudflareRouter from "./routes/cloudflare.js";
import systemRouter from "./routes/system.js";
import frpRouter from "./routes/frp.js";
import mcpTokensRouter from "./routes/mcp-tokens.js";
import mcpRouter from "./mcp/http.js";

const app = express();

type RawBodyRequest = express.Request & { rawBody?: Buffer };

function captureRawBody(req: express.Request, _res: express.Response, buffer: Buffer) {
  (req as RawBodyRequest).rawBody = Buffer.from(buffer);
}

app.use(express.json({ limit: "1mb", verify: captureRawBody }));
app.use(express.urlencoded({ extended: false, limit: "1mb", verify: captureRawBody }));
app.use(cookieParser());
app.use(helmet({
  contentSecurityPolicy: {
    directives: {
      defaultSrc: ["'self'"],
      scriptSrc: ["'self'", "'unsafe-inline'"],
      styleSrc: ["'self'", "'unsafe-inline'"],
      imgSrc: ["'self'", "data:"],
      connectSrc: ["'self'"]
    }
  }
}));

const CSRF_COOKIE = "yanto_csrf";
const CSRF_SKIP_PATHS = ["/api/auth/login", "/api/workers/", "/deploy", "/api/webhooks/", "/webhooks/", "/mcp"];

function csrfProtection(req: express.Request, res: express.Response, next: express.NextFunction) {
  if (req.method === "GET" || req.method === "HEAD" || req.method === "OPTIONS") {
    if (!req.cookies[CSRF_COOKIE]) {
      res.cookie(CSRF_COOKIE, crypto.randomBytes(24).toString("base64url"), {
        httpOnly: false,
        sameSite: "lax",
        secure: config.cookieSecure
      });
    }
    next();
    return;
  }

  if (CSRF_SKIP_PATHS.some((p) => req.path.startsWith(p))) {
    next();
    return;
  }

  const cookieToken = req.cookies[CSRF_COOKIE];
  const headerToken = req.header("x-csrf-token");
  if (!cookieToken || !headerToken || cookieToken !== headerToken) {
    res.status(403).json({ message: "Invalid CSRF token." });
    return;
  }

  // Rotate CSRF token after successful validation to limit token reuse window
  const newToken = crypto.randomBytes(24).toString("base64url");
  res.cookie(CSRF_COOKIE, newToken, {
    httpOnly: false,
    sameSite: "lax",
    secure: config.cookieSecure
  });
  res.setHeader("x-csrf-token", newToken);

  next();
}

app.use(csrfProtection);

app.use(mcpRouter);
app.use(authRouter);
app.use(projectsRouter);
app.use(deploymentsRouter);
app.use(containersRouter);
app.use(backupsRouter);
app.use(workersRouter);
app.use(settingsRouter);
app.use(cloudflareRouter);
app.use(systemRouter);
app.use(frpRouter);
app.use(mcpTokensRouter);

app.use((error: unknown, _req: express.Request, res: express.Response, next: express.NextFunction) => {
  void next;
  const message = error instanceof Error ? error.message : "Unexpected server error.";
  logger.error("request failed", { error: message });
  const status = error instanceof z.ZodError ? 400 : error instanceof HttpError ? error.status : 500;
  const publicMessage = config.nodeEnv === "production" && status >= 500 && !(error instanceof HttpError) ? "Internal server error." : message;
  res.status(status).json({ message: publicMessage });
});

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const clientDir =
  [path.resolve(__dirname, "../client"), path.resolve(__dirname, "../../client"), path.resolve(__dirname, "../../dist/client")].find((candidate) =>
    fs.existsSync(path.join(candidate, "index.html"))
  ) ?? path.resolve(__dirname, "../client");
app.use(express.static(clientDir));
app.get(/.*/, (_req, res) => {
  res.sendFile(path.join(clientDir, "index.html"));
});

async function main() {
  warnOnUnsafeDefaults();
  await migrate();
  await ensureLocalMasterNode();
  await recoverInterruptedDeployments();
  const server = app.listen(config.port, () => {
    logger.info("server started", { port: config.port });
    void ensureEnabledCloudflaredConnectors()
      .then(async (result) => {
        if (result.started.length || result.failed.length) {
          logger.info("cloudflared connector reconciliation completed", result);
        }
        const networks = await reconcileTunnelAssignments();
        logger.info("cloudflared network reconciliation completed", networks);
      })
      .catch((error) => {
        logger.error("cloudflared connector reconciliation failed", { error: error instanceof Error ? error.message : String(error) });
      });
  });

  const shutdown = async (signal: string) => {
    logger.info("shutdown starting", { signal });
    server.close();
    await new Promise((resolve) => setTimeout(resolve, 5_000));
    await pool.end();
    process.exit(0);
  };

  process.on("SIGINT", () => void shutdown("SIGINT"));
  process.on("SIGTERM", () => void shutdown("SIGTERM"));
}

main().catch((error) => {
  logger.error("server failed to start", { error: error instanceof Error ? error.message : String(error) });
  process.exit(1);
});
