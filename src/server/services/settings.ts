import { eq } from "drizzle-orm";
import { db } from "../db/index.js";
import { appSettings } from "../db/schema.js";
import { config } from "../config.js";
import { createWorkerJoinToken } from "./tokens.js";

export type R2SettingsInput = {
  enabled?: boolean;
  accountId?: string;
  bucket?: string;
  accessKeyId?: string;
  secretAccessKey?: string;
  prefix?: string;
};

export type StoredR2Settings = Required<R2SettingsInput>;

const r2SettingsKey = "cloudflare.r2";
const workerJoinTokenKey = "worker.join_token";
const setupWizardKey = "setup.wizard";
const emptyR2Settings: StoredR2Settings = {
  enabled: false,
  accountId: "",
  bucket: "",
  accessKeyId: "",
  secretAccessKey: "",
  prefix: "postgres-dumps"
};

type SetupWizardSettings = {
  completedAt: string | null;
  dismissedAt: string | null;
  updatedAt: string | null;
};

const emptySetupWizardSettings: SetupWizardSettings = {
  completedAt: null,
  dismissedAt: null,
  updatedAt: null
};

function normalizeString(value: unknown) {
  return typeof value === "string" ? value.trim() : "";
}

function parseR2Settings(value: string | undefined): StoredR2Settings {
  if (!value) return emptyR2Settings;
  try {
    const parsed = JSON.parse(value) as Partial<StoredR2Settings>;
    return {
      enabled: Boolean(parsed.enabled),
      accountId: normalizeString(parsed.accountId),
      bucket: normalizeString(parsed.bucket),
      accessKeyId: normalizeString(parsed.accessKeyId),
      secretAccessKey: normalizeString(parsed.secretAccessKey),
      prefix: normalizeString(parsed.prefix) || emptyR2Settings.prefix
    };
  } catch {
    return emptyR2Settings;
  }
}

export async function getStoredR2Settings() {
  const [row] = await db.select().from(appSettings).where(eq(appSettings.key, r2SettingsKey)).limit(1);
  return parseR2Settings(row?.value);
}

function maskCredential(value: string): string {
  if (!value) return "";
  const tail = value.slice(-4);
  return `****${tail}`;
}

export async function publicR2Settings() {
  const settings = await getStoredR2Settings();
  return {
    enabled: settings.enabled,
    accountId: settings.accountId,
    bucket: settings.bucket,
    maskedAccessKeyId: settings.accessKeyId ? maskCredential(settings.accessKeyId) : "",
    hasAccessKeyId: Boolean(settings.accessKeyId),
    hasSecretAccessKey: Boolean(settings.secretAccessKey),
    prefix: settings.prefix
  };
}

export async function saveR2Settings(input: R2SettingsInput) {
  const current = await getStoredR2Settings();
  const nextAccessKeyId = normalizeString(input.accessKeyId);
  const next: StoredR2Settings = {
    enabled: Boolean(input.enabled),
    accountId: normalizeString(input.accountId),
    bucket: normalizeString(input.bucket),
    accessKeyId: nextAccessKeyId && nextAccessKeyId !== maskCredential(current.accessKeyId) ? nextAccessKeyId : current.accessKeyId,
    secretAccessKey: normalizeString(input.secretAccessKey) || current.secretAccessKey,
    prefix: normalizeString(input.prefix) || emptyR2Settings.prefix
  };

  await db
    .insert(appSettings)
    .values({ key: r2SettingsKey, value: JSON.stringify(next), updatedAt: new Date() })
    .onConflictDoUpdate({
      target: appSettings.key,
      set: { value: JSON.stringify(next), updatedAt: new Date() }
    });

  return publicR2Settings();
}

function parseSetupWizardSettings(value: string | undefined): SetupWizardSettings {
  if (!value) return emptySetupWizardSettings;
  try {
    const parsed = JSON.parse(value) as Partial<SetupWizardSettings>;
    return {
      completedAt: normalizeString(parsed.completedAt) || null,
      dismissedAt: normalizeString(parsed.dismissedAt) || null,
      updatedAt: normalizeString(parsed.updatedAt) || null
    };
  } catch {
    return emptySetupWizardSettings;
  }
}

export async function publicSetupWizardSettings() {
  const [row] = await db.select().from(appSettings).where(eq(appSettings.key, setupWizardKey)).limit(1);
  return parseSetupWizardSettings(row?.value);
}

export async function saveSetupWizardSettings(action: "completed" | "dismissed") {
  const current = await publicSetupWizardSettings();
  const now = new Date().toISOString();
  const next: SetupWizardSettings = {
    completedAt: action === "completed" ? now : current.completedAt,
    dismissedAt: action === "dismissed" ? now : current.dismissedAt,
    updatedAt: now
  };

  await db
    .insert(appSettings)
    .values({ key: setupWizardKey, value: JSON.stringify(next), updatedAt: new Date() })
    .onConflictDoUpdate({
      target: appSettings.key,
      set: { value: JSON.stringify(next), updatedAt: new Date() }
    });

  return next;
}

export async function getWorkerJoinToken() {
  if (config.workerJoinToken) {
    return config.workerJoinToken;
  }

  const [row] = await db.select().from(appSettings).where(eq(appSettings.key, workerJoinTokenKey)).limit(1);
  return row?.value ?? "";
}

export async function ensureWorkerJoinToken() {
  const existing = await getWorkerJoinToken();
  if (existing) {
    return existing;
  }

  const token = createWorkerJoinToken();
  const [row] = await db
    .insert(appSettings)
    .values({ key: workerJoinTokenKey, value: token, updatedAt: new Date() })
    .onConflictDoUpdate({
      target: appSettings.key,
      set: { updatedAt: new Date() }
    })
    .returning();

  return row.value;
}
