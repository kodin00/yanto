import Anthropic from "@anthropic-ai/sdk";
import OpenAI from "openai";
import { asc, eq } from "drizzle-orm";
import { db } from "../db/index.js";
import { aiModels, aiProviders, type AiProviderRow } from "../db/schema.js";
import { HttpError } from "../http-utils.js";
import { decrypt, encrypt } from "./crypto.js";
import { createId } from "./tokens.js";

export type AiProviderProtocol = "openai_responses" | "openai_chat" | "anthropic_messages" | "codex_account";
export const CODEX_PROVIDER_ID = "aip_codex_account";

export type AiProviderInput = {
  name: string;
  protocol: AiProviderProtocol;
  baseUrl: string;
  apiKey?: string;
  enabled?: boolean;
  defaultModelId?: string | null;
};

function normalizeBaseUrl(value: string, protocol: AiProviderProtocol) {
  if (protocol === "codex_account") return "";
  const fallback = protocol === "anthropic_messages" ? "https://api.anthropic.com" : "https://api.openai.com/v1";
  const raw = value.trim() || fallback;
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    throw new HttpError(400, "Provider base URL must be a valid HTTP or HTTPS URL.");
  }
  if (!['http:', 'https:'].includes(url.protocol)) {
    throw new HttpError(400, "Provider base URL must use HTTP or HTTPS.");
  }
  return raw.replace(/\/$/, "");
}

export function publicProvider(row: AiProviderRow, models: Array<typeof aiModels.$inferSelect> = []) {
  return {
    id: row.id,
    name: row.name,
    protocol: row.protocol as AiProviderProtocol,
    baseUrl: row.baseUrl,
    hasApiKey: Boolean(row.apiKey),
    enabled: row.enabled,
    defaultModelId: row.defaultModelId ?? null,
    models,
    createdAt: row.createdAt,
    updatedAt: row.updatedAt
  };
}

export async function listAiProviders() {
  const [providers, models] = await Promise.all([
    db.select().from(aiProviders).orderBy(asc(aiProviders.name)),
    db.select().from(aiModels).orderBy(asc(aiModels.displayName))
  ]);
  return providers.map((provider) => publicProvider(provider, models.filter((model) => model.providerId === provider.id)));
}

export async function getAiProvider(id: string) {
  const [provider] = await db.select().from(aiProviders).where(eq(aiProviders.id, id)).limit(1);
  return provider;
}

export async function createAiProvider(input: AiProviderInput) {
  if (input.protocol === "codex_account") throw new HttpError(400, "Use Sign in with Codex to register a Codex account.");
  if (!input.apiKey?.trim()) throw new HttpError(400, "API key is required.");
  const [provider] = await db.insert(aiProviders).values({
    id: createId("aip"),
    name: input.name.trim(),
    protocol: input.protocol,
    baseUrl: normalizeBaseUrl(input.baseUrl, input.protocol),
    apiKey: encrypt(input.apiKey.trim()),
    enabled: input.enabled ?? true,
    createdAt: new Date(),
    updatedAt: new Date()
  }).returning();
  return publicProvider(provider);
}

export async function updateAiProvider(id: string, input: Partial<AiProviderInput>) {
  const current = await getAiProvider(id);
  if (!current) return undefined;
  const editsCoreFields = input.name !== undefined || input.protocol !== undefined || input.baseUrl !== undefined || input.apiKey !== undefined || input.enabled !== undefined;
  if (current.protocol === "codex_account" && editsCoreFields) throw new HttpError(400, "Manage this provider with the Codex account controls.");
  if (input.defaultModelId) {
    const [model] = await db.select().from(aiModels).where(eq(aiModels.id, input.defaultModelId)).limit(1);
    if (!model || model.providerId !== id) throw new HttpError(400, "Default model must belong to this provider.");
  }
  const protocol = input.protocol ?? current.protocol as AiProviderProtocol;
  const [provider] = await db.update(aiProviders).set({
    ...(input.name !== undefined ? { name: input.name.trim() } : {}),
    ...(input.protocol !== undefined ? { protocol: input.protocol } : {}),
    ...(input.baseUrl !== undefined ? { baseUrl: normalizeBaseUrl(input.baseUrl, protocol) } : {}),
    ...(input.apiKey?.trim() ? { apiKey: encrypt(input.apiKey.trim()) } : {}),
    ...(input.enabled !== undefined ? { enabled: input.enabled } : {}),
    ...(input.defaultModelId !== undefined ? { defaultModelId: input.defaultModelId } : {}),
    updatedAt: new Date()
  }).where(eq(aiProviders.id, id)).returning();
  const models = await db.select().from(aiModels).where(eq(aiModels.providerId, id));
  return publicProvider(provider, models);
}

export async function deleteAiProvider(id: string) {
  if (id === CODEX_PROVIDER_ID) throw new HttpError(400, "Sign out from the Codex account controls instead.");
  try {
    await db.delete(aiProviders).where(eq(aiProviders.id, id));
  } catch (error) {
    if ((error as { code?: string }).code === "23503") {
      throw new HttpError(409, "Provider is used by one or more tasks. Disable it instead.");
    }
    throw error;
  }
}

export async function addAiModel(providerId: string, modelId: string, displayName?: string) {
  if (!await getAiProvider(providerId)) throw new HttpError(404, "Provider not found.");
  const normalized = modelId.trim();
  if (!normalized) throw new HttpError(400, "Model ID is required.");
  const [model] = await db.insert(aiModels).values({
    id: createId("aim"),
    providerId,
    modelId: normalized,
    displayName: displayName?.trim() || normalized,
    enabled: true,
    createdAt: new Date(),
    updatedAt: new Date()
  }).onConflictDoUpdate({
    target: [aiModels.providerId, aiModels.modelId],
    set: { displayName: displayName?.trim() || normalized, enabled: true, updatedAt: new Date() }
  }).returning();
  return model;
}

export async function updateAiModel(id: string, input: { displayName?: string; enabled?: boolean }) {
  const [model] = await db.update(aiModels).set({
    ...(input.displayName !== undefined ? { displayName: input.displayName.trim() } : {}),
    ...(input.enabled !== undefined ? { enabled: input.enabled } : {}),
    updatedAt: new Date()
  }).where(eq(aiModels.id, id)).returning();
  return model;
}

export async function deleteAiModel(id: string) {
  try {
    await db.delete(aiModels).where(eq(aiModels.id, id));
  } catch (error) {
    if ((error as { code?: string }).code === "23503") throw new HttpError(409, "Model is used by one or more tasks.");
    throw error;
  }
}

export async function resolveProviderModel(modelId: string) {
  const [row] = await db.select({ provider: aiProviders, model: aiModels })
    .from(aiModels)
    .innerJoin(aiProviders, eq(aiModels.providerId, aiProviders.id))
    .where(eq(aiModels.id, modelId)).limit(1);
  if (!row || !row.provider.enabled || !row.model.enabled) throw new HttpError(400, "Selected provider model is unavailable.");
  return { ...row, apiKey: row.provider.protocol === "codex_account" ? "" : decrypt(row.provider.apiKey) };
}

export async function syncCodexProvider(models: Array<{ id: string; name: string }>, enabled: boolean) {
  const now = new Date();
  await db.insert(aiProviders).values({
    id: CODEX_PROVIDER_ID, name: "Codex account", protocol: "codex_account", baseUrl: "", apiKey: "", enabled,
    createdAt: now, updatedAt: now
  }).onConflictDoUpdate({ target: aiProviders.id, set: { name: "Codex account", enabled, updatedAt: now } });
  for (const model of models.length ? models : [{ id: "default", name: "Codex default" }]) {
    await addAiModel(CODEX_PROVIDER_ID, model.id, model.name);
  }
  return CODEX_PROVIDER_ID;
}

export async function setCodexProviderEnabled(enabled: boolean) {
  await db.update(aiProviders).set({ enabled, updatedAt: new Date() }).where(eq(aiProviders.id, CODEX_PROVIDER_ID));
}

export async function discoverAiModels(providerId: string) {
  const provider = await getAiProvider(providerId);
  if (!provider) throw new HttpError(404, "Provider not found.");
  if (provider.protocol === "codex_account") throw new HttpError(400, "Refresh Codex models from the Codex account card.");
  const apiKey = decrypt(provider.apiKey);
  const discovered: Array<{ id: string; name: string }> = [];
  if (provider.protocol === "anthropic_messages") {
    const client = new Anthropic({ apiKey, baseURL: provider.baseUrl, timeout: 30_000, maxRetries: 0 });
    for await (const model of client.models.list({ limit: 100 })) discovered.push({ id: model.id, name: model.display_name || model.id });
  } else {
    const client = new OpenAI({ apiKey, baseURL: provider.baseUrl, timeout: 30_000, maxRetries: 0 });
    for await (const model of client.models.list()) discovered.push({ id: model.id, name: model.id });
  }
  for (const model of discovered) await addAiModel(provider.id, model.id, model.name);
  return discovered;
}
