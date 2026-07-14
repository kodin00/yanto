import { Router } from "express";
import { z } from "zod";
import { requireOwner } from "../auth.js";
import { actor, asyncRoute, routeParam } from "../http-utils.js";
import { recordAuditLog } from "../services/audit.js";
import { cancelCodexLogin, getCodexAccountStatus, listCodexModels, logoutCodexAccount, startCodexLogin } from "../services/codex-auth.js";
import { addAiModel, createAiProvider, deleteAiModel, deleteAiProvider, discoverAiModels, listAiProviders, setCodexProviderEnabled, syncCodexProvider, updateAiModel, updateAiProvider } from "../services/ai-providers.js";

const router = Router();
const protocol = z.enum(["openai_responses", "openai_chat", "anthropic_messages"]);
const providerInput = z.object({
  name: z.string().trim().min(1).max(100),
  protocol,
  baseUrl: z.string().trim().max(500).default(""),
  apiKey: z.string().max(10_000).optional(),
  enabled: z.boolean().optional(),
  defaultModelId: z.string().trim().min(1).max(100).nullable().optional()
});
const modelInput = z.object({ modelId: z.string().trim().min(1).max(250), displayName: z.string().trim().max(250).optional() });

router.get("/api/ai/providers", requireOwner, asyncRoute(async (_req, res) => {
  res.json(await listAiProviders());
}));

router.get("/api/ai/models/available", requireOwner, asyncRoute(async (_req, res) => {
  const providers = await listAiProviders();
  res.json(providers.filter((provider) => provider.enabled).map((provider) => ({
    id: provider.id,
    name: provider.name,
    protocol: provider.protocol,
    baseUrl: "",
    hasApiKey: false,
    enabled: true,
    defaultModelId: provider.defaultModelId,
    models: provider.models.filter((model) => model.enabled).map((model) => ({
      id: model.id,
      providerId: model.providerId,
      modelId: model.modelId,
      displayName: model.displayName,
      enabled: true
    }))
  })));
}));

router.get("/api/ai/codex/status", requireOwner, asyncRoute(async (_req, res) => {
  res.json(await getCodexAccountStatus());
}));

router.post("/api/ai/codex/login/start", requireOwner, asyncRoute(async (req, res) => {
  const login = await startCodexLogin();
  await recordAuditLog({ actor: actor(req), action: "codex.login.start", entityType: "ai_provider" });
  res.json(login);
}));

router.post("/api/ai/codex/login/cancel", requireOwner, asyncRoute(async (_req, res) => {
  await cancelCodexLogin();
  res.json({ ok: true });
}));

router.post("/api/ai/codex/logout", requireOwner, asyncRoute(async (req, res) => {
  await logoutCodexAccount();
  await setCodexProviderEnabled(false);
  await recordAuditLog({ actor: actor(req), action: "codex.logout", entityType: "ai_provider" });
  res.json({ ok: true });
}));

router.post("/api/ai/codex/models/refresh", requireOwner, asyncRoute(async (_req, res) => {
  const status = await getCodexAccountStatus();
  if (!status.connected) { res.status(409).json({ message: "Sign in with Codex first." }); return; }
  const models = await listCodexModels();
  await syncCodexProvider(models, true);
  res.json({ ok: true, models });
}));

router.post("/api/ai/providers", requireOwner, asyncRoute(async (req, res) => {
  const provider = await createAiProvider(providerInput.parse(req.body));
  await recordAuditLog({ actor: actor(req), action: "ai_provider.create", entityType: "ai_provider", entityId: provider.id });
  res.status(201).json(provider);
}));

router.patch("/api/ai/providers/:id", requireOwner, asyncRoute(async (req, res) => {
  const provider = await updateAiProvider(routeParam(req, "id"), providerInput.partial().parse(req.body));
  if (!provider) { res.status(404).json({ message: "Provider not found." }); return; }
  await recordAuditLog({ actor: actor(req), action: "ai_provider.update", entityType: "ai_provider", entityId: provider.id });
  res.json(provider);
}));

router.delete("/api/ai/providers/:id", requireOwner, asyncRoute(async (req, res) => {
  const id = routeParam(req, "id");
  await deleteAiProvider(id);
  await recordAuditLog({ actor: actor(req), action: "ai_provider.delete", entityType: "ai_provider", entityId: id });
  res.status(204).end();
}));

router.post("/api/ai/providers/:id/discover", requireOwner, asyncRoute(async (req, res) => {
  const models = await discoverAiModels(routeParam(req, "id"));
  res.json({ ok: true, models });
}));

router.post("/api/ai/providers/:id/models", requireOwner, asyncRoute(async (req, res) => {
  const body = modelInput.parse(req.body);
  res.status(201).json(await addAiModel(routeParam(req, "id"), body.modelId, body.displayName));
}));

router.patch("/api/ai/models/:id", requireOwner, asyncRoute(async (req, res) => {
  const model = await updateAiModel(routeParam(req, "id"), z.object({ displayName: z.string().trim().min(1).max(250).optional(), enabled: z.boolean().optional() }).parse(req.body));
  if (!model) { res.status(404).json({ message: "Model not found." }); return; }
  res.json(model);
}));

router.delete("/api/ai/models/:id", requireOwner, asyncRoute(async (req, res) => {
  await deleteAiModel(routeParam(req, "id"));
  res.status(204).end();
}));

export default router;
