import { z } from "zod";

export const projectInput = z.object({
  name: z.string().min(1),
  gitUrl: z.string().optional(),
  branch: z.string().optional().default("master"),
  folderName: z.string().optional().default(""),
  composeFile: z.string().min(1).optional(),
  composeContent: z.string().optional(),
  envFile: z.string().min(1).optional(),
  autoStart: z.boolean().optional().default(true)
});

export const deploymentInput = z.object({
  targetRef: z.string().optional()
});

export const rollbackInput = z.object({
  deploymentId: z.string().optional(),
  targetRef: z.string().optional()
});

export const envInput = z.object({
  envFile: z.string().min(1).optional(),
  content: z.string()
});

export const envVariablesInput = z.object({
  envFile: z.string().min(1).optional(),
  variables: z.array(z.object({ key: z.string(), value: z.string().nullable().optional(), masked: z.boolean().optional() }))
});

export const backupInput = z.object({
  containerId: z.string().min(1).optional()
});

export const r2SettingsInput = z.object({
  enabled: z.boolean().optional().default(false),
  accountId: z.string().optional().default(""),
  bucket: z.string().optional().default(""),
  accessKeyId: z.string().optional().default(""),
  secretAccessKey: z.string().optional().default(""),
  prefix: z.string().optional().default("postgres-dumps")
});
