import { eq } from "drizzle-orm";
import { db } from "../db/index.js";
import { projects } from "../db/schema.js";
import { ensureSshKey } from "./ssh.js";
import { createDeployToken, createId } from "./tokens.js";
import { ensureProjectsRoot, normalizeComposeFile, projectPath, slugifyFolderName } from "./paths.js";

export type CreateProjectInput = {
  name: string;
  gitUrl?: string;
  branch?: string;
  folderName: string;
  composeFile?: string;
  composeContent?: string;
};

export async function listProjects() {
  return db.select().from(projects).orderBy(projects.createdAt);
}

export async function createProject(input: CreateProjectInput) {
  await ensureProjectsRoot();
  const id = createId("prj");
  const folderName = input.folderName.trim() || slugifyFolderName(input.name);
  const localPath = projectPath(folderName);
  const gitUrl = input.gitUrl?.trim() || null;
  const sshKey = gitUrl ? await ensureSshKey(id) : null;

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
      deployToken: createDeployToken(),
      sshPrivateKeyPath: sshKey?.privateKeyPath ?? null,
      sshPublicKey: sshKey?.publicKey ?? null,
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

  const patch: Record<string, string | Date | null> = {
    updatedAt: new Date()
  };
  if (input.name !== undefined) patch.name = input.name.trim();
  if (input.gitUrl !== undefined) {
    const gitUrl = input.gitUrl.trim() || null;
    patch.gitUrl = gitUrl;
    if (gitUrl && !current.sshPrivateKeyPath) {
      const sshKey = await ensureSshKey(id);
      patch.sshPrivateKeyPath = sshKey.privateKeyPath;
      patch.sshPublicKey = sshKey.publicKey;
    }
  }
  if (input.branch !== undefined) patch.branch = input.branch.trim() || "master";
  if (input.composeFile !== undefined) patch.composeFile = normalizeComposeFile(input.composeFile);
  if (input.composeContent !== undefined) patch.composeContent = input.composeContent.trim();
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
