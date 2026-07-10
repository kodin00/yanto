import fs from "node:fs/promises";
import path from "node:path";
import { config } from "../config.js";

const safeFolderPattern = /^[a-zA-Z0-9._-]+$/;
const safeComposeFilePattern = /^[a-zA-Z0-9._/-]+\.ya?ml$/;
const safeEnvFilePattern = /^[a-zA-Z0-9._/-]+$/;

export function slugifyFolderName(input: string) {
  const slug = input
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 64);
  return slug || "project";
}

export function normalizeFolderName(input: string) {
  const trimmed = input.trim();
  if (!trimmed || trimmed.includes("..") || path.isAbsolute(trimmed) || !safeFolderPattern.test(trimmed)) {
    throw new Error("Folder name may only contain letters, numbers, dots, underscores, and dashes.");
  }
  return trimmed;
}

export function projectPath(folderName: string) {
  const safeName = normalizeFolderName(folderName);
  const resolved = path.resolve(config.projectsRoot, safeName);
  const rootWithSeparator = `${config.projectsRoot}${path.sep}`;
  if (resolved !== config.projectsRoot && !resolved.startsWith(rootWithSeparator)) {
    throw new Error("Project path must stay inside the configured projects root.");
  }
  return resolved;
}

export function normalizeComposeFile(input: string) {
  const trimmed = input.trim() || "docker-compose.yml";
  if (path.isAbsolute(trimmed) || trimmed.includes("..") || !safeComposeFilePattern.test(trimmed)) {
    throw new Error("Compose file must be a relative .yml/.yaml path inside the project folder.");
  }
  return trimmed;
}

export function normalizeEnvFile(input: string) {
  const trimmed = input.trim() || ".env";
  const baseName = path.basename(trimmed);
  if (path.isAbsolute(trimmed) || trimmed.includes("..") || !safeEnvFilePattern.test(trimmed) || !baseName.startsWith(".env")) {
    throw new Error("Env file must be a relative .env file path inside the project folder.");
  }
  return trimmed;
}

export async function pathExists(target: string) {
  try {
    await fs.access(target);
    return true;
  } catch {
    return false;
  }
}

export async function ensureProjectsRoot() {
  await fs.mkdir(config.projectsRoot, { recursive: true });
}

export async function removeProjectDirectory(folderName: string) {
  await fs.rm(projectPath(folderName), { recursive: true, force: true });
}

export async function removeProjectWorktreeDirectory(folderName: string) {
  const safeName = normalizeFolderName(folderName);
  const worktreesRoot = path.resolve(config.projectsRoot, ".yanto-worktrees");
  const target = path.resolve(worktreesRoot, safeName);
  if (!target.startsWith(`${worktreesRoot}${path.sep}`)) {
    throw new Error("Project worktree path must stay inside the configured worktree root.");
  }
  await fs.rmdir(target).catch((error: unknown) => {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  });
}
