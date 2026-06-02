import { Router } from "express";
import { z } from "zod";
import { requireAuth } from "../auth.js";
import { config } from "../config.js";
import { db } from "../db/index.js";
import { projects } from "../db/schema.js";
import { asyncRoute, actor } from "../http-utils.js";
import { cloudflareSettingsInput, multiNodeSettingsInput, r2SettingsInput, setupWizardInput } from "../route-schemas.js";
import { recordAuditLog } from "../services/audit.js";
import { publicCloudflareSettings, saveCloudflareSettings, validateCloudflareSettings } from "../services/cloudflare.js";
import { publicMultiNodeSettings, publicR2Settings, publicSetupWizardSettings, saveMultiNodeSettings, saveR2Settings, saveSetupWizardSettings } from "../services/settings.js";
import { generateManagedSshPrivateKey, managedSshKeyStatus, saveManagedSshPrivateKey } from "../services/ssh.js";

const router = Router();

router.get(
  "/api/settings",
  requireAuth,
  asyncRoute(async (_req, res) => {
    const [count, sshKey, r2, cf, setupWizard, multiNode] = await Promise.all([
      db.select().from(projects),
      managedSshKeyStatus(),
      publicR2Settings(),
      publicCloudflareSettings(),
      publicSetupWizardSettings(),
      publicMultiNodeSettings()
    ]);
    res.json({
      projectsRoot: config.projectsRoot,
      hostProjectsRoot: config.hostProjectsRoot,
      sshKeysDir: config.sshKeysDir,
      appBaseUrl: config.appBaseUrl,
      projectCount: count.length,
      sshKey,
      r2,
      cf,
      setupWizard,
      multiNode
    });
  })
);

router.post(
  "/api/settings/r2",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = r2SettingsInput.parse(req.body ?? {});
    const r2 = await saveR2Settings(body);
    await recordAuditLog({ actor: actor(req), action: "settings.r2.save", entityType: "settings", metadata: { enabled: r2.enabled, bucket: r2.bucket, prefix: r2.prefix } });
    res.json({ ok: true, r2 });
  })
);

router.post(
  "/api/settings/cloudflare",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = cloudflareSettingsInput.parse(req.body ?? {});
    const cf = await saveCloudflareSettings(body);
    await recordAuditLog({ actor: actor(req), action: "settings.cloudflare.save", entityType: "settings", metadata: { accountId: cf.accountId, zoneId: cf.zoneId } });
    res.json({ ok: true, cf });
  })
);

router.post(
  "/api/settings/cloudflare/validate",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = cloudflareSettingsInput.parse(req.body ?? {});
    const result = await validateCloudflareSettings(body);
    await recordAuditLog({ actor: actor(req), action: "settings.cloudflare.validate", entityType: "settings" });
    res.json(result);
  })
);

router.post(
  "/api/settings/ssh-key",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = z.object({ privateKey: z.string().min(1) }).parse(req.body);
    const sshKey = await saveManagedSshPrivateKey(body.privateKey);
    await recordAuditLog({ actor: actor(req), action: "settings.ssh_key.save", entityType: "settings" });
    res.json({ ok: true, sshKey });
  })
);

router.post(
  "/api/settings/ssh-key/generate",
  requireAuth,
  asyncRoute(async (req, res) => {
    const sshKey = await generateManagedSshPrivateKey();
    await recordAuditLog({ actor: actor(req), action: "settings.ssh_key.generate", entityType: "settings" });
    res.json({ ok: true, sshKey });
  })
);

router.post(
  "/api/settings/setup-wizard",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = setupWizardInput.parse(req.body ?? {});
    const setupWizard = await saveSetupWizardSettings(body.action);
    await recordAuditLog({ actor: actor(req), action: `settings.setup_wizard.${body.action}`, entityType: "settings" });
    res.json({ ok: true, setupWizard });
  })
);

router.post(
  "/api/settings/multi-node",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = multiNodeSettingsInput.parse(req.body ?? {});
    const multiNode = await saveMultiNodeSettings(body);
    await recordAuditLog({ actor: actor(req), action: "settings.multi_node.save", entityType: "settings", metadata: { enabled: multiNode.enabled, releaseStage: multiNode.releaseStage } });
    res.json({ ok: true, multiNode });
  })
);

export default router;
