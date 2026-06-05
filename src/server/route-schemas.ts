import { z } from "zod";

export const projectInput = z.object({
  name: z.string().min(1),
  gitUrl: z.string().optional(),
  branch: z.string().optional().default("master"),
  folderName: z.string().optional().default(""),
  composeFile: z.string().min(1).optional(),
  composeContent: z.string().optional(),
  envFile: z.string().min(1).optional(),
  autoStart: z.boolean().optional().default(true),
  manualDeployEnabled: z.boolean().optional().default(true),
  githubWebhookEnabled: z.boolean().optional().default(true),
  targetNodeId: z.string().min(1).optional()
});

export const deploymentInput = z.object({
  targetRef: z.string().optional(),
  envContent: z.string().optional(),
  envVariables: z.array(z.object({ key: z.string(), value: z.string().nullable().optional(), masked: z.boolean().optional() })).optional(),
  envFile: z.string().min(1).optional()
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

export const cloudflareSettingsInput = z.object({
  accountId: z.string().optional().default(""),
  zoneId: z.string().optional().default(""),
  apiToken: z.string().optional().default("")
});

export const setupWizardInput = z.object({
  action: z.enum(["completed", "dismissed"])
});

export const multiNodeSettingsInput = z.object({
  enabled: z.boolean().optional().default(false)
});

export const cloudflareRouteInput = z.object({
  hostname: z.string().min(1).regex(/^[a-zA-Z0-9]([a-zA-Z0-9.-]*[a-zA-Z0-9])?$/, "Invalid hostname format"),
  serviceTarget: z.string().min(1).regex(/^(https?|tcp|ssh):\/\/[a-zA-Z0-9._-]+:\d+$/, "Invalid service target (expected scheme://hostname:port)"),
  noTlsVerify: z.boolean().optional().default(false),
  nodeId: z.string().min(1).optional()
});

export const cloudflareDnsRecordInput = z.object({
  type: z.enum(["A", "AAAA", "CNAME", "TXT", "MX", "NS"]),
  name: z.string().min(1).max(255),
  content: z.string().min(1).max(4096),
  ttl: z.number().int().min(1).max(2_147_483_647).optional().default(1),
  proxied: z.boolean().optional().default(false),
  priority: z.number().int().min(0).max(65_535).nullable().optional(),
  comment: z.string().max(500).nullable().optional()
});

export const workerRegisterInput = z.object({
  joinToken: z.string().min(1),
  name: z.string().optional(),
  dockerVersion: z.string().optional().nullable(),
  labels: z.record(z.string(), z.unknown()).optional()
});

export const workerHeartbeatInput = z.object({
  name: z.string().optional(),
  dockerVersion: z.string().optional().nullable(),
  labels: z.record(z.string(), z.unknown()).optional()
});

export const workerLogInput = z.object({
  chunk: z.string()
});

export const workerDeploymentUpdateInput = z.object({
  status: z.enum(["success", "failed"]).optional(),
  exitCode: z.number().int().nullable().optional(),
  commitSha: z.string().nullable().optional(),
  commitMessage: z.string().nullable().optional(),
  targetRef: z.string().nullable().optional()
});
