import { eq } from "drizzle-orm";
import { db } from "../db/index.js";
import { projects } from "../db/schema.js";
import { createDeployToken, createId } from "./tokens.js";
import { listContainers } from "./docker.js";
import { ensureProjectsRoot, normalizeComposeFile, normalizeEnvFile, projectPath, slugifyFolderName } from "./paths.js";

export type CreateProjectInput = {
  name: string;
  gitUrl?: string;
  branch?: string;
  folderName: string;
  composeFile?: string;
  composeContent?: string;
  envFile?: string;
  autoStart?: boolean;
};

export async function listProjects() {
  return db.select().from(projects).orderBy(projects.createdAt);
}

export async function listProjectsWithContainerCounts() {
  const rows = await listProjects();
  let containers: Awaited<ReturnType<typeof listContainers>> = [];
  try {
    containers = await listContainers();
  } catch {
    return rows.map((project) => ({ ...project, containerCount: 0 }));
  }

  return rows.map((project) => ({
    ...project,
    containerCount: containers.filter((container) => container.composeProject === project.folderName).length
  }));
}

export async function createProject(input: CreateProjectInput) {
  await ensureProjectsRoot();
  const id = createId("prj");
  const folderName = input.folderName.trim() || slugifyFolderName(input.name);
  const localPath = projectPath(folderName);
  const gitUrl = input.gitUrl?.trim() || null;

  const [project] = await db
    .insert(projects)
    .values({
      id,
      name: input.name.trim(),
      gitUrl,
      branch: input.branch?.trim() || "master",
      folderName,
      localPath,
      composeFile: normalizeComposeFile(input.composeFile ?? "docker-compose.yml"),
      composeContent: input.composeContent?.trim() || null,
      envFile: normalizeEnvFile(input.envFile ?? ".env"),
      autoStart: input.autoStart ?? false,
      deployToken: createDeployToken(),
      sshPrivateKeyPath: null,
      sshPublicKey: null,
      createdAt: new Date(),
      updatedAt: new Date()
    })
    .returning();

  return project;
}

export async function updateProject(id: string, input: Partial<CreateProjectInput>) {
  const [current] = await db.select().from(projects).where(eq(projects.id, id)).limit(1);
  if (!current) {
    return undefined;
  }

  const patch: Record<string, string | boolean | Date | null> = {
    updatedAt: new Date()
  };
  if (input.name !== undefined) patch.name = input.name.trim();
  if (input.gitUrl !== undefined) {
    const gitUrl = input.gitUrl.trim() || null;
    patch.gitUrl = gitUrl;
  }
  if (input.branch !== undefined) patch.branch = input.branch.trim() || "master";
  if (input.composeFile !== undefined) patch.composeFile = normalizeComposeFile(input.composeFile);
  if (input.composeContent !== undefined) patch.composeContent = input.composeContent.trim();
  if (input.envFile !== undefined) patch.envFile = normalizeEnvFile(input.envFile);
  if (input.autoStart !== undefined) patch.autoStart = input.autoStart;
  if (input.folderName !== undefined) {
    const folderName = input.folderName.trim() || (input.name ? slugifyFolderName(input.name) : "");
    if (folderName) {
      patch.folderName = folderName;
      patch.localPath = projectPath(folderName);
    }
  }

  const [project] = await db.update(projects).set(patch).where(eq(projects.id, id)).returning();
  return project;
}

export async function deleteProject(id: string) {
  await db.delete(projects).where(eq(projects.id, id));
}

export async function getProject(id: string) {
  const [project] = await db.select().from(projects).where(eq(projects.id, id)).limit(1);
  return project;
}
