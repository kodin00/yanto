import fs from "node:fs/promises";
import path from "node:path";
import { and, desc, eq, inArray, ne } from "drizzle-orm";
import { db } from "../db/index.js";
import { agentTasks, cloudflareRoutes, projects } from "../db/schema.js";
import { createDeployToken, createId } from "./tokens.js";
import { listContainers } from "./docker.js";
import { ensureProjectsRoot, normalizeComposeFile, normalizeEnvFile, pathExists, projectPath, removeProjectDirectory, removeProjectWorktreeDirectory, slugifyFolderName } from "./paths.js";
import { config } from "../config.js";
import { assertDeployableNode } from "./nodes.js";
import { assertComposePortsAvailable } from "./compose.js";
import { HttpError } from "../http-utils.js";
import { cleanupTaskWorktree, pruneTaskWorktrees } from "./agent-worktrees.js";
import { agentProjectLifecycleKey, withAgentLifecycleLock } from "./agent-lifecycle.js";
import { dockerImageFromInput } from "../../shared/docker-images.js";

export function publicProject<T extends { deployToken: string; sshPrivateKeyPath?: string | null }>(project: T): Omit<T, "deployToken" | "sshPrivateKeyPath"> {
  // eslint-disable-next-line @typescript-eslint/no-unused-vars -- destructured to exclude from output
  const { deployToken, sshPrivateKeyPath, ...rest } = project;
  return rest;
}

export type CreateProjectInput = {
  name: string;
  source?: string;
  gitUrl?: string;
  dockerImage?: string;
  branch?: string;
  folderName: string;
  composeFile?: string;
  composeContent?: string;
  envFile?: string;
  autoStart?: boolean;
  manualDeployEnabled?: boolean;
  githubWebhookEnabled?: boolean;
  targetNodeId?: string;
  agentImage?: string;
};

function projectSource(input: Pick<CreateProjectInput, "source" | "gitUrl" | "dockerImage">) {
  if (input.source !== undefined) {
    const source = input.source.trim();
    if (!source) return { gitUrl: null, dockerImage: "" };
    const image = dockerImageFromInput(source);
    if (image) return { gitUrl: null, dockerImage: image };
    if (/^docker\b/i.test(source)) throw new HttpError(400, "Use a Docker pull command such as: docker pull ghcr.io/owner/image:tag");
    return { gitUrl: source, dockerImage: "" };
  }

  if (input.dockerImage !== undefined) {
    const image = dockerImageFromInput(input.dockerImage);
    if (image === null) throw new HttpError(400, "Invalid Docker image reference.");
    return { gitUrl: image ? null : input.gitUrl?.trim() || null, dockerImage: image };
  }

  return { gitUrl: input.gitUrl?.trim() || null, dockerImage: "" };
}

export async function listProjects() {
  return db.select().from(projects).orderBy(desc(projects.createdAt));
}

export async function listProjectsWithContainerCounts() {
  const rows = await listProjects();
  const routeRows = rows.length
    ? await db.select().from(cloudflareRoutes).where(inArray(cloudflareRoutes.projectId, rows.map((project) => project.id)))
    : [];
  const routesByProject = new Map<string, typeof routeRows>();
  for (const route of routeRows) {
    if (!route.projectId) continue;
    routesByProject.set(route.projectId, [...(routesByProject.get(route.projectId) ?? []), route]);
  }
  let containers: Awaited<ReturnType<typeof listContainers>> = [];
  try {
    containers = await listContainers();
  } catch {
    return rows.map((project) => ({ ...project, containerCount: 0, cloudflareRoutes: routesByProject.get(project.id) ?? [] }));
  }

  return rows.map((project) => ({
    ...project,
    containerCount: containers.filter((container) => container.composeProject === project.folderName).length,
    cloudflareRoutes: routesByProject.get(project.id) ?? []
  }));
}

export async function createProject(input: CreateProjectInput) {
  await ensureProjectsRoot();
  const targetNode = await assertDeployableNode(input.targetNodeId?.trim() || config.localNodeId);
  const id = createId("prj");
  const folderName = input.folderName.trim() || slugifyFolderName(input.name);
  const [folderConflict] = await db.select({ id: projects.id }).from(projects).where(eq(projects.folderName, folderName)).limit(1);
  if (folderConflict) throw new HttpError(409, "That project folder is already in use.");
  const localPath = projectPath(folderName);
  const { gitUrl, dockerImage } = projectSource(input);
  const composeFile = normalizeComposeFile(input.composeFile ?? "docker-compose.yml");
  const composeContent = input.composeContent?.trim() || null;

  if (composeContent) {
    await assertComposePortsAvailable(composeContent, { ignoreComposeProject: folderName });
  } else {
    const composePath = path.join(localPath, composeFile);
    if (await pathExists(composePath)) {
      await assertComposePortsAvailable(await fs.readFile(composePath, "utf8"), { ignoreComposeProject: folderName });
    }
  }

  const [project] = await db
    .insert(projects)
    .values({
      id,
      name: input.name.trim(),
      gitUrl,
      dockerImage,
      branch: input.branch?.trim() || "master",
      folderName,
      localPath,
      composeFile,
      composeContent,
      envFile: normalizeEnvFile(input.envFile ?? ".env"),
      autoStart: input.autoStart ?? false,
      manualDeployEnabled: input.manualDeployEnabled ?? true,
      githubWebhookEnabled: input.githubWebhookEnabled ?? !dockerImage,
      targetNodeId: targetNode.id,
      deployToken: createDeployToken(),
      sshPrivateKeyPath: null,
      sshPublicKey: null,
      agentImage: input.agentImage?.trim() || "",
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
  if (input.source !== undefined || input.gitUrl !== undefined || input.dockerImage !== undefined) {
    const source = projectSource(input);
    patch.gitUrl = source.gitUrl;
    patch.dockerImage = source.dockerImage;
  }
  if (input.branch !== undefined) patch.branch = input.branch.trim() || "master";
  if (input.composeFile !== undefined) patch.composeFile = normalizeComposeFile(input.composeFile);
  if (input.composeContent !== undefined) patch.composeContent = input.composeContent.trim() || null;
  if (input.envFile !== undefined) patch.envFile = normalizeEnvFile(input.envFile);
  if (input.autoStart !== undefined) patch.autoStart = input.autoStart;
  if (input.manualDeployEnabled !== undefined) patch.manualDeployEnabled = input.manualDeployEnabled;
  if (input.githubWebhookEnabled !== undefined) patch.githubWebhookEnabled = input.githubWebhookEnabled;
  if (input.agentImage !== undefined) patch.agentImage = input.agentImage.trim();
  if (input.targetNodeId !== undefined) {
    const targetNode = await assertDeployableNode(input.targetNodeId.trim() || config.localNodeId);
    patch.targetNodeId = targetNode.id;
  }
  if (input.folderName !== undefined) {
    const folderName = input.folderName.trim() || (input.name ? slugifyFolderName(input.name) : "");
    if (folderName) {
      const [folderConflict] = await db.select({ id: projects.id }).from(projects)
        .where(and(eq(projects.folderName, folderName), ne(projects.id, id))).limit(1);
      if (folderConflict) throw new HttpError(409, "That project folder is already in use.");
      patch.folderName = folderName;
      patch.localPath = projectPath(folderName);
    }
  }

  const [project] = await db.update(projects).set(patch).where(eq(projects.id, id)).returning();
  return project;
}

export async function deleteProject(id: string) {
  const folderName = await withAgentLifecycleLock([agentProjectLifecycleKey(id)], async () => {
    const current = await getProject(id);
    if (!current) return undefined;
    const tasks = await db.select().from(agentTasks).where(eq(agentTasks.projectId, id));
    if (tasks.some((task) => task.status === "running")) {
      throw new HttpError(409, "Stop all active agent runs before deleting the project.");
    }
    for (const task of tasks) {
      await cleanupTaskWorktree(current, task);
    }
    await pruneTaskWorktrees(current);
    await db.delete(projects).where(eq(projects.id, id));
    return current.folderName;
  });
  if (!folderName) return;
  await removeProjectDirectory(folderName);
  await removeProjectWorktreeDirectory(folderName);
}

export async function getProject(id: string) {
  const [project] = await db.select().from(projects).where(eq(projects.id, id)).limit(1);
  return project;
}

export async function getProjectDeployToken(id: string) {
  const [project] = await db.select({ deployToken: projects.deployToken }).from(projects).where(eq(projects.id, id)).limit(1);
  return project?.deployToken;
}
