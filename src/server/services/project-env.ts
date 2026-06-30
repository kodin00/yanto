import fs from "node:fs/promises";
import path from "node:path";
import type { EnvPreview, EnvPreviewEntry } from "../../shared/types.js";
import type { ProjectRow } from "../db/schema.js";
import { normalizeEnvFile, pathExists } from "./paths.js";

const defaultEnvFile = ".env";

function envPath(project: ProjectRow, envFile?: string) {
  return path.join(project.localPath, normalizeEnvFile(envFile ?? defaultEnvFile));
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
      maskedValue: value,
      comment: null
    };
  });

  return {
    envFile,
    entryCount: entries.filter((entry) => entry.key).length,
    entries
  };
}

export async function readProjectEnv(project: ProjectRow, envFile = defaultEnvFile) {
  const target = envPath(project, envFile);
  const normalized = normalizeEnvFile(envFile);
  if (!(await pathExists(target))) {
    return { envFile: normalized, content: "" };
  }
  const content = await fs.readFile(target, "utf8");
  return {
    envFile: normalized,
    content
  };
}

export async function readProjectEnvVariables(project: ProjectRow, envFile?: string) {
  const current = await readProjectEnv(project, envFile);
  return Array.from(parseEnvMap(current.content).entries()).map(([key, value]) => ({
    key,
    value
  }));
}

export async function previewProjectEnv(project: ProjectRow, envFile?: string) {
  const current = await readProjectEnv(project, envFile);
  return previewEnvContent(current.content, current.envFile);
}

export async function writeProjectEnv(project: ProjectRow, content: string, envFile = defaultEnvFile) {
  const normalized = normalizeEnvFile(envFile);
  const target = envPath(project, normalized);
  await fs.mkdir(path.dirname(target), { recursive: true });
  await fs.writeFile(target, content, { encoding: "utf8", mode: 0o600 });
  return previewEnvContent(content, normalized);
}

export async function writeProjectEnvVariables(project: ProjectRow, variables: { key: string; value?: string | null; masked?: boolean }[], envFile = defaultEnvFile) {
  const normalized = normalizeEnvFile(envFile);
  const content = variables
    .map((variable) => {
      const key = variable.key.trim();
      const value = variable.value ?? "";
      return `${key}=${value}`;
    })
    .filter((line) => line.split("=")[0])
    .join("\n");
  return writeProjectEnv(project, `${content}${content ? "\n" : ""}`, normalized);
}
