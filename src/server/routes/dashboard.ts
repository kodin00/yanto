import { Router } from "express";
import { requireAuth } from "../auth.js";
import { asyncRoute } from "../http-utils.js";
import { listProjectsWithRoutes, publicProject } from "../services/projects.js";
import { latestDeployments } from "../services/deployments.js";
import { listNodes } from "../services/nodes.js";
import { listContainersSummary } from "../services/docker.js";
import { systemUsage } from "../services/system.js";
import { config } from "../config.js";
import { db } from "../db/index.js";
import { projects } from "../db/schema.js";
import { managedSshKeyStatus } from "../services/ssh.js";
import { publicR2Settings, publicSetupWizardSettings } from "../services/settings.js";
import { publicCloudflareSettings } from "../services/cloudflare.js";

const router = Router();

router.get(
  "/api/dashboard",
  requireAuth,
  asyncRoute(async (_req, res) => {
    const [projectRows, deployments, nodes, containers, usage, count, sshKey, r2, cf, setupWizard] = await Promise.all([
      listProjectsWithRoutes(),
      latestDeployments(),
      listNodes(),
      listContainersSummary(),
      systemUsage().catch(() => null),
      db.select().from(projects),
      managedSshKeyStatus(),
      publicR2Settings(),
      publicCloudflareSettings(),
      publicSetupWizardSettings()
    ]);

    res.json({
      projects: projectRows.map(publicProject),
      deployments,
      nodes,
      containers,
      usage,
      settings: {
        projectsRoot: config.projectsRoot,
        hostProjectsRoot: config.hostProjectsRoot,
        sshKeysDir: config.sshKeysDir,
        appBaseUrl: config.appBaseUrl,
        projectCount: count.length,
        sshKey,
        r2,
        cf,
        setupWizard
      }
    });
  })
);

export default router;
