import { Router } from "express";
import { z } from "zod";
import { requireAuth } from "../auth.js";
import { actor, asyncRoute, routeParam } from "../http-utils.js";
import { recordAuditLog } from "../services/audit.js";
import { addAiModel, createAiProvider, deleteAiModel, deleteAiProvider, discoverAiModels, listAiProviders, updateAiModel, updateAiProvider } from "../services/ai-providers.js";

const router = Router();
const protocol = z.enum(["openai_responses", "openai_chat", "anthropic_messages"]);
const providerInput = z.object({
  name: z.string().trim().min(1).max(100),
  protocol,
  baseUrl: z.string().trim().max(500).default(""),
  apiKey: z.string().max(10_000).optional(),
  enabled: z.boolean().optional()
});
const modelInput = z.object({ modelId: z.string().trim().min(1).max(250), displayName: z.string().trim().max(250).optional() });

router.get("/api/ai/providers", requireAuth, asyncRoute(async (_req, res) => { res.json(await listAiProviders()); }));

router.post("/api/ai/providers", requireAuth, asyncRoute(async (req, res) => {
  const provider = await createAiProvider(providerInput.parse(req.body));
  await recordAuditLog({ actor: actor(req), action: "ai_provider.create", entityType: "ai_provider", entityId: provider.id });
  res.status(201).json(provider);
}));

router.patch("/api/ai/providers/:id", requireAuth, asyncRoute(async (req, res) => {
  const provider = await updateAiProvider(routeParam(req, "id"), providerInput.partial().parse(req.body));
  if (!provider) { res.status(404).json({ message: "Provider not found." }); return; }
  await recordAuditLog({ actor: actor(req), action: "ai_provider.update", entityType: "ai_provider", entityId: provider.id });
  res.json(provider);
}));

router.delete("/api/ai/providers/:id", requireAuth, asyncRoute(async (req, res) => {
  const id = routeParam(req, "id");
  await deleteAiProvider(id);
  await recordAuditLog({ actor: actor(req), action: "ai_provider.delete", entityType: "ai_provider", entityId: id });
  res.status(204).end();
}));

router.post("/api/ai/providers/:id/discover", requireAuth, asyncRoute(async (req, res) => {
  const models = await discoverAiModels(routeParam(req, "id"));
  res.json({ ok: true, models });
}));

router.post("/api/ai/providers/:id/models", requireAuth, asyncRoute(async (req, res) => {
  const body = modelInput.parse(req.body);
  res.status(201).json(await addAiModel(routeParam(req, "id"), body.modelId, body.displayName));
}));

router.patch("/api/ai/models/:id", requireAuth, asyncRoute(async (req, res) => {
  const model = await updateAiModel(routeParam(req, "id"), z.object({ displayName: z.string().trim().min(1).max(250).optional(), enabled: z.boolean().optional() }).parse(req.body));
  if (!model) { res.status(404).json({ message: "Model not found." }); return; }
  res.json(model);
}));

router.delete("/api/ai/models/:id", requireAuth, asyncRoute(async (req, res) => {
  await deleteAiModel(routeParam(req, "id"));
  res.status(204).end();
}));

export default router;
