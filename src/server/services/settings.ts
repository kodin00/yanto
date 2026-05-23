import { eq } from "drizzle-orm";
import { db } from "../db/index.js";
import { appSettings } from "../db/schema.js";

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
const emptyR2Settings: StoredR2Settings = {
  enabled: false,
  accountId: "",
  bucket: "",
  accessKeyId: "",
  secretAccessKey: "",
  prefix: "postgres-dumps"
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

export async function publicR2Settings() {
  const settings = await getStoredR2Settings();
  return {
    enabled: settings.enabled,
    accountId: settings.accountId,
    bucket: settings.bucket,
    accessKeyId: settings.accessKeyId,
    hasSecretAccessKey: Boolean(settings.secretAccessKey),
    prefix: settings.prefix
  };
}

export async function saveR2Settings(input: R2SettingsInput) {
  const current = await getStoredR2Settings();
  const next: StoredR2Settings = {
    enabled: Boolean(input.enabled),
    accountId: normalizeString(input.accountId),
    bucket: normalizeString(input.bucket),
    accessKeyId: normalizeString(input.accessKeyId),
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
