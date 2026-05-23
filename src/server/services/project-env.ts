import fs from "node:fs/promises";
import path from "node:path";
import type { EnvPreview, EnvPreviewEntry } from "../../shared/types.js";
import type { ProjectRow } from "../db/schema.js";
import { normalizeEnvFile, pathExists } from "./paths.js";

const secretPattern = /(secret|token|password|passwd|pwd|key|credential|auth|jwt)/i;
const maskedSentinel = "********";

function envPath(project: ProjectRow, envFile?: string) {
  return path.join(project.localPath, normalizeEnvFile(envFile ?? project.envFile ?? ".env"));
}

function maskValue(key: string, value: string) {
  if (!value) return "";
  const shouldMask = secretPattern.test(key) || value.length > 16;
  if (!shouldMask) return value;
  if (value.length <= 4) return "****";
  return `${value.slice(0, 2)}****${value.slice(-2)}`;
}

function parseEnvMap(content: string) {
  const values = new Map<string, string>();
  for (const line of content.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const separator = line.indexOf("=");
    if (separator <= 0) continue;
    values.set(line.slice(0, separator).trim(), line.slice(separator + 1));
  }
  return values;
}

export function previewEnvContent(content: string, envFile = ".env"): EnvPreview {
  const entries: EnvPreviewEntry[] = content.split(/\r?\n/).map((line, index) => {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) {
      return {
        line: index + 1,
        key: null,
        hasValue: false,
        maskedValue: null,
        comment: trimmed.startsWith("#") ? trimmed : null
      };
    }

    const separator = line.indexOf("=");
    if (separator <= 0) {
      return {
        line: index + 1,
        key: null,
        hasValue: false,
        maskedValue: null,
        comment: null
      };
    }

    const key = line.slice(0, separator).trim();
    const value = line.slice(separator + 1);
    return {
      line: index + 1,
      key,
      hasValue: value.length > 0,
      maskedValue: maskValue(key, value),
      comment: null
    };
  });

  return {
    envFile,
    entryCount: entries.filter((entry) => entry.key).length,
    entries
  };
}

export async function readProjectEnv(project: ProjectRow, envFile?: string) {
  const target = envPath(project, envFile);
  const normalized = normalizeEnvFile(envFile ?? project.envFile ?? ".env");
  if (!(await pathExists(target))) {
    return { envFile: normalized, content: "" };
  }
  const content = await fs.readFile(target, "utf8");
  const maskedContent = content
    .split(/\r?\n/)
    .map((line) => {
      const separator = line.indexOf("=");
      if (separator <= 0) return line;
      const key = line.slice(0, separator).trim();
      const value = line.slice(separator + 1);
      return secretPattern.test(key) || value.length > 16 ? `${line.slice(0, separator + 1)}${maskedSentinel}` : line;
    })
    .join("\n");
  return {
    envFile: normalized,
    content: maskedContent
  };
}

export async function readProjectEnvVariables(project: ProjectRow, envFile?: string) {
  const current = await readProjectEnv(project, envFile);
  return Array.from(parseEnvMap(current.content).entries()).map(([key, value]) => ({
    key,
    value,
    masked: value === maskedSentinel
  }));
}

export async function previewProjectEnv(project: ProjectRow, envFile?: string) {
  const current = await readProjectEnv(project, envFile);
  return previewEnvContent(current.content, current.envFile);
}

export async function writeProjectEnv(project: ProjectRow, content: string, envFile?: string) {
  const normalized = normalizeEnvFile(envFile ?? project.envFile ?? ".env");
  const target = envPath(project, normalized);
  const existing = (await pathExists(target)) ? await fs.readFile(target, "utf8") : "";
  const existingValues = parseEnvMap(existing);
  const resolvedContent = content
    .split(/\r?\n/)
    .map((line) => {
      const separator = line.indexOf("=");
      if (separator <= 0) return line;
      const key = line.slice(0, separator).trim();
      const value = line.slice(separator + 1);
      if (value === maskedSentinel && existingValues.has(key)) {
        return `${line.slice(0, separator + 1)}${existingValues.get(key) ?? ""}`;
      }
      return line;
    })
    .join("\n");
  await fs.mkdir(path.dirname(target), { recursive: true });
  await fs.writeFile(target, resolvedContent, { encoding: "utf8", mode: 0o600 });
  return previewEnvContent(resolvedContent, normalized);
}

export async function writeProjectEnvVariables(project: ProjectRow, variables: { key: string; value?: string | null; masked?: boolean }[], envFile?: string) {
  const normalized = normalizeEnvFile(envFile ?? project.envFile ?? ".env");
  const currentPath = envPath(project, normalized);
  const existing = (await pathExists(currentPath)) ? await fs.readFile(currentPath, "utf8") : "";
  const existingValues = parseEnvMap(existing);
  const content = variables
    .map((variable) => {
      const key = variable.key.trim();
      const value = variable.value == null && variable.masked ? existingValues.get(key) ?? "" : variable.value ?? "";
      return `${key}=${value}`;
    })
    .filter((line) => line.split("=")[0])
    .join("\n");
  return writeProjectEnv(project, `${content}${content ? "\n" : ""}`, normalized);
}
