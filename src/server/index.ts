import cookieParser from "cookie-parser";
import express from "express";
import fs from "node:fs";
import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { z } from "zod";
import { clearSessionCookie, currentUser, requireAuth, setSessionCookie, verifyAdminPassword } from "./auth.js";
import { config, warnOnUnsafeDefaults } from "./config.js";
import { db, migrate, pool } from "./db/index.js";
import { projects } from "./db/schema.js";
import { logger } from "./logger.js";
import { cleanupDocker, containerLogs, listContainers, restartContainer, stopContainer } from "./services/docker.js";
import { findDeployment, latestDeployments, startDeployment } from "./services/deployments.js";
import { createProject, deleteProject, getProject, listProjects, updateProject } from "./services/projects.js";
import { managedSshKeyStatus, saveManagedSshPrivateKey } from "./services/ssh.js";
import { systemUsage } from "./services/system.js";
import { constantTimeEqual } from "./services/tokens.js";

const app = express();
app.use(express.json({ limit: "1mb" }));
app.use(cookieParser());

const projectInput = z.object({
  name: z.string().min(1),
  gitUrl: z.string().optional(),
  branch: z.string().optional().default("master"),
  folderName: z.string().optional().default(""),
  composeFile: z.string().min(1).optional(),
  composeContent: z.string().optional()
});

function asyncRoute(handler: (req: express.Request, res: express.Response) => Promise<void>) {
  return (req: express.Request, res: express.Response, next: express.NextFunction) => {
    handler(req, res).catch(next);
  };
}

function routeParam(req: express.Request, name: string) {
  const value = req.params[name];
  return Array.isArray(value) ? value[0] : value;
}

function startEventStream(res: express.Response) {
  res.writeHead(200, {
    "Content-Type": "text/event-stream",
    "Cache-Control": "no-cache, no-transform",
    Connection: "keep-alive",
    "X-Accel-Buffering": "no"
  });
}

function sendStreamEvent(res: express.Response, payload: unknown) {
  res.write(`data: ${JSON.stringify(payload)}\n\n`);
}

app.post(
  "/api/auth/login",
  asyncRoute(async (req, res) => {
    const body = z.object({ username: z.string(), password: z.string() }).parse(req.body);
    const ok = await verifyAdminPassword(body.username, body.password);
    if (!ok) {
      res.status(401).json({ message: "Invalid username or password." });
      return;
    }
    setSessionCookie(res);
    res.json({ username: config.adminUsername });
  })
);

app.post("/api/auth/logout", (_req, res) => {
  clearSessionCookie(res);
  res.json({ ok: true });
});

app.get("/api/auth/me", (req, res) => {
  const user = currentUser(req);
  if (!user) {
    res.status(401).json({ message: "Authentication required." });
    return;
  }
  res.json({ username: user.username });
});

app.get(
  "/api/projects",
  requireAuth,
  asyncRoute(async (_req, res) => {
    res.json(await listProjects());
  })
);

app.post(
  "/api/projects",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = projectInput.parse(req.body);
    const project = await createProject(body);
    res.status(201).json(project);
  })
);

app.patch(
  "/api/projects/:id",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = projectInput.partial().parse(req.body);
    const project = await updateProject(routeParam(req, "id"), body);
    if (!project) {
      res.status(404).json({ message: "Project not found." });
      return;
    }
    res.json(project);
  })
);

app.delete(
  "/api/projects/:id",
  requireAuth,
  asyncRoute(async (req, res) => {
    await deleteProject(routeParam(req, "id"));
    res.status(204).end();
  })
);

app.post(
  "/api/projects/:id/deploy",
  requireAuth,
  asyncRoute(async (req, res) => {
    const result = await startDeployment(routeParam(req, "id"), "manual");
    res.status(result.reused ? 200 : 202).json(result);
  })
);

app.post(
  "/deploy",
  asyncRoute(async (req, res) => {
    const id = String(req.query.id ?? "");
    const header = req.header("authorization") ?? "";
    const token = header.startsWith("Bearer ") ? header.slice("Bearer ".length) : "";
    const project = await getProject(id);
    if (!project || !token || !constantTimeEqual(project.deployToken, token)) {
      res.status(401).json({ message: "Invalid deployment token." });
      return;
    }
    const result = await startDeployment(id, "webhook");
    res.status(result.reused ? 200 : 202).json(result);
  })
);

app.get(
  "/api/deployments",
  requireAuth,
  asyncRoute(async (_req, res) => {
    res.json(await latestDeployments());
  })
);

app.get(
  "/api/deployments/:id",
  requireAuth,
  asyncRoute(async (req, res) => {
    const deployment = await findDeployment(routeParam(req, "id"));
    if (!deployment) {
      res.status(404).json({ message: "Deployment not found." });
      return;
    }
    res.json(deployment);
  })
);

app.get(
  "/api/deployments/:id/logs",
  requireAuth,
  asyncRoute(async (req, res) => {
    const deployment = await findDeployment(routeParam(req, "id"));
    if (!deployment) {
      res.status(404).json({ message: "Deployment not found." });
      return;
    }
    res.type("text/plain").send(deployment.logs);
  })
);

app.get(
  "/api/deployments/:id/logs/stream",
  requireAuth,
  asyncRoute(async (req, res) => {
    const id = routeParam(req, "id");
    const initial = await findDeployment(id);
    if (!initial) {
      res.status(404).json({ message: "Deployment not found." });
      return;
    }

    startEventStream(res);
    let previousLogs = "";
    let closed = false;
    let timer: NodeJS.Timeout | null = null;

    const pushLatest = async () => {
      if (closed) return;
      const deployment = await findDeployment(id);
      if (!deployment) {
        sendStreamEvent(res, { logs: previousLogs, status: "missing", done: true });
        res.end();
        return;
      }
      if (deployment.logs !== previousLogs || deployment.status !== "running") {
        previousLogs = deployment.logs;
        sendStreamEvent(res, {
          logs: deployment.logs,
          status: deployment.status,
          done: deployment.status !== "running"
        });
      }
      if (deployment.status !== "running") {
        closed = true;
        if (timer) {
          clearInterval(timer);
        }
        res.end();
      }
    };

    await pushLatest();
    if (!closed) {
      timer = setInterval(() => {
        void pushLatest().catch((error) => {
          sendStreamEvent(res, { error: error instanceof Error ? error.message : "Unable to stream deployment logs.", done: true });
          res.end();
        });
      }, 700);
    }

    req.on("close", () => {
      closed = true;
      if (timer) {
        clearInterval(timer);
      }
    });
  })
);

app.get(
  "/api/containers",
  requireAuth,
  asyncRoute(async (_req, res) => {
    res.json(await listContainers());
  })
);

app.get(
  "/api/containers/:id/logs",
  requireAuth,
  asyncRoute(async (req, res) => {
    res.type("text/plain").send(await containerLogs(routeParam(req, "id")));
  })
);

app.get(
  "/api/containers/:id/logs/stream",
  requireAuth,
  asyncRoute(async (req, res) => {
    startEventStream(res);
    let closed = false;
    const child = spawn("docker", ["logs", "--tail", "500", "--follow", routeParam(req, "id")], {
      env: process.env,
      shell: false
    });

    const sendChunk = (buffer: Buffer) => {
      if (closed) return;
      sendStreamEvent(res, { chunk: buffer.toString() });
    };

    child.stdout.on("data", sendChunk);
    child.stderr.on("data", sendChunk);
    child.on("error", (error) => {
      if (closed) return;
      sendStreamEvent(res, { error: error.message, done: true });
      closed = true;
      res.end();
    });
    child.on("close", (exitCode) => {
      if (closed) return;
      sendStreamEvent(res, { chunk: `\nLog stream closed${exitCode ? ` with exit code ${exitCode}` : ""}.\n`, done: true });
      closed = true;
      res.end();
    });

    req.on("close", () => {
      closed = true;
      child.kill("SIGTERM");
    });
  })
);

app.post(
  "/api/containers/:id/stop",
  requireAuth,
  asyncRoute(async (req, res) => {
    await stopContainer(routeParam(req, "id"));
    res.json({ ok: true });
  })
);

app.post(
  "/api/containers/:id/restart",
  requireAuth,
  asyncRoute(async (req, res) => {
    await restartContainer(routeParam(req, "id"));
    res.json({ ok: true });
  })
);

app.get(
  "/api/system/usage",
  requireAuth,
  asyncRoute(async (_req, res) => {
    res.json(await systemUsage());
  })
);

app.post(
  "/api/system/cleanup",
  requireAuth,
  asyncRoute(async (_req, res) => {
    res.json({ logs: await cleanupDocker() });
  })
);

app.get(
  "/api/settings",
  requireAuth,
  asyncRoute(async (_req, res) => {
    const count = await db.select().from(projects);
    const sshKey = await managedSshKeyStatus();
    res.json({
      projectsRoot: config.projectsRoot,
      hostProjectsRoot: config.hostProjectsRoot,
      sshKeysDir: config.sshKeysDir,
      appBaseUrl: config.appBaseUrl,
      projectCount: count.length,
      sshKey
    });
  })
);

app.post(
  "/api/settings/ssh-key",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = z.object({ privateKey: z.string().min(1) }).parse(req.body);
    const sshKey = await saveManagedSshPrivateKey(body.privateKey);
    res.json({ ok: true, sshKey });
  })
);

app.use((error: unknown, _req: express.Request, res: express.Response, next: express.NextFunction) => {
  void next;
  const message = error instanceof Error ? error.message : "Unexpected server error.";
  logger.error("request failed", { error: message });
  res.status(400).json({ message });
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
  app.listen(config.port, () => {
    logger.info("server started", { port: config.port });
  });
}

process.on("SIGINT", async () => {
  await pool.end();
  process.exit(0);
});

main().catch((error) => {
  logger.error("server failed to start", { error: error instanceof Error ? error.message : String(error) });
  process.exit(1);
});
