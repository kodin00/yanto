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
  targetNodeId: z.string().min(1).optional(),
  agentImage: z.string().max(500).optional().default("")
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
  containerId: z.string().min(1).optional(),
  sourceNodeId: z.string().min(1).optional()
});

export const backupPolicyInput = z.object({
  name: z.string().trim().min(1).max(120),
  sourceNodeId: z.string().min(1),
  targetContainerId: z.string().min(1).nullable().optional(),
  enabled: z.boolean().optional().default(true),
  hourlyAtMinute: z.number().int().min(0).max(59).optional().default(0),
  hourlyRetention: z.number().int().min(1).max(744).optional().default(24),
  dailyRetention: z.number().int().min(1).max(3650).optional().default(30),
  destinationNodeIds: z.array(z.string().min(1)).max(64).optional().default([])
});

export const backupPolicyUpdateInput = backupPolicyInput.partial().refine((value) => Object.keys(value).length > 0, "At least one field is required.");

export const r2SettingsInput = z.object({
  enabled: z.boolean().optional().default(false),
  accountId: z.string().trim().refine((value) => !value || /^[a-fA-F0-9]{32}$/.test(value), "Cloudflare account ID must be 32 hexadecimal characters").optional().default(""),
  bucket: z.string().trim().refine((value) => !value || /^[a-z0-9][a-z0-9-]{1,61}[a-z0-9]$/.test(value), "R2 bucket must be a 3-63 character lowercase bucket name").optional().default(""),
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

export const backupDestinationInput = z.object({
  sshHost: z.string().trim().max(255).optional().default(""),
  sshPort: z.number().int().min(1).max(65535).optional().default(22),
  sshUser: z.string().trim().max(100).optional().default(""),
  directory: z.string().trim().max(1000).optional().default(""),
  privateKeyPath: z.string().trim().max(1000).optional().default("")
});

export const cloudflareRouteInput = z.object({
  hostname: z.string().min(1).regex(/^[a-zA-Z0-9]([a-zA-Z0-9.-]*[a-zA-Z0-9])?$/, "Invalid hostname format"),
  serviceTarget: z.string().min(1).regex(/^(https?|tcp|ssh):\/\/[a-zA-Z0-9._-]+:\d+$/, "Invalid service target (expected scheme://hostname:port)"),
  noTlsVerify: z.boolean().optional().default(false),
  nodeId: z.string().min(1).optional()
});

export const cloudflareClientInput = z.object({
  name: z.string().min(1).max(100),
  accountId: z.string().min(1).max(64),
  zoneId: z.string().min(1).max(64),
  apiToken: z.string().min(1).optional()
});

export const cloudflareTunnelInput = z.object({ clientId: z.string().min(1), name: z.string().min(1).max(100) });

export const cloudflareAssignmentInput = z.object({
  tunnelId: z.string().min(1),
  projectId: z.string().min(1).optional(),
  composeProject: z.string().optional(),
  composeService: z.string().optional(),
  containerName: z.string().optional()
});

export const cloudflareHostnameInput = z.object({
  tunnelId: z.string().min(1),
  assignmentId: z.string().min(1),
  zoneId: z.string().min(1),
  hostname: z.string().min(1).regex(/^[a-zA-Z0-9]([a-zA-Z0-9.-]*[a-zA-Z0-9])?$/, "Invalid hostname format"),
  protocol: z.enum(["http", "https"]),
  port: z.number().int().min(1).max(65535),
  noTlsVerify: z.boolean().optional().default(false)
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
  joinToken: z.string().min(1).max(1_000),
  name: z.string().trim().max(200).optional(),
  dockerVersion: z.string().trim().max(200).optional().nullable(),
  labels: z.record(
    z.string().max(100),
    z.union([z.string().max(64 * 1024), z.number(), z.boolean(), z.null()])
  ).refine((value) => Object.keys(value).length <= 64, "Worker labels are limited to 64 entries").optional()
});

export const workerHeartbeatInput = z.object({
  name: z.string().trim().max(200).optional(),
  dockerVersion: z.string().trim().max(200).optional().nullable(),
  labels: z.record(
    z.string().max(100),
    z.union([z.string().max(64 * 1024), z.number(), z.boolean(), z.null()])
  ).refine((value) => Object.keys(value).length <= 64, "Worker labels are limited to 64 entries").optional()
});

export const workerLogInput = z.object({
  chunk: z.string().max(256 * 1024)
});

export const workerDeploymentUpdateInput = z.object({
  status: z.enum(["success", "failed"]).optional(),
  exitCode: z.number().int().nullable().optional(),
  commitSha: z.string().max(128).nullable().optional(),
  commitMessage: z.string().max(2_000).nullable().optional(),
  targetRef: z.string().max(1_000).nullable().optional()
});

export const workerBackupCompletionInput = z.object({
  status: z.enum(["success", "failed"]),
  filePath: z.string().max(4000).optional(),
  fileSizeBytes: z.number().int().nonnegative().optional(),
  checksum: z.string().regex(/^[a-f0-9]{64}$/i).optional(),
  error: z.string().max(4000).optional(),
  replicas: z.array(z.object({
    destinationNodeId: z.string().min(1),
    status: z.enum(["success", "failed"]),
    filePath: z.string().max(4000).optional(),
    checksum: z.string().regex(/^[a-f0-9]{64}$/i).optional(),
    error: z.string().max(4000).optional(),
    attempts: z.number().int().min(1).max(10).optional()
  })).max(64).optional()
});

const frpHost = z.string().trim().min(1).max(255).refine(
  (value) => !/[\s/:?#]/.test(value) || /^[0-9a-fA-F:]+$/.test(value),
  "Expected an IP address or hostname without a scheme, path, or port"
);

export const frpSettingsInput = z.object({
  publicHost: frpHost.or(z.literal(""))
});

export const frpTunnelInput = z.object({
  name: z.string().trim().min(1).max(100),
  nodeId: z.string().min(1).nullable().optional(),
  clientNodeId: z.string().min(1).nullable().optional(),
  serverId: z.string().min(1).nullable().optional(),
  protocol: z.enum(["tcp", "udp"]),
  localHost: frpHost,
  localPort: z.number().int().min(1).max(65535),
  remotePort: z.number().int().min(1).max(65535),
  enabled: z.boolean().optional().default(true)
});

export const frpTunnelUpdateInput = frpTunnelInput.partial().refine((value) => Object.keys(value).length > 0, "At least one field is required.");

export const frpServerInput = z.object({
  nodeId: z.string().min(1),
  name: z.string().trim().min(1).max(100),
  publicHost: frpHost,
  bindPort: z.number().int().min(1).max(65535).optional().default(7000),
  portStart: z.number().int().min(1).max(65535).optional().default(25560),
  portEnd: z.number().int().min(1).max(65535).optional().default(25600),
  authToken: z.string().min(16).max(1000).optional()
}).refine((value) => value.portStart <= value.portEnd, "FRP port start must be less than or equal to port end.");

export const frpNodeAssignmentInput = z.object({
  role: z.enum(["disabled", "client", "server", "both"]),
  serverId: z.string().min(1).nullable().optional()
});

export const frpNodeStatusInput = z.object({
  revision: z.number().int().min(0),
  status: z.enum(["applying", "online", "error", "disabled"]),
  error: z.string().max(4000).nullable().optional()
});

export const mcpAccessLevelInput = z.enum(["read", "write", "admin"]);

export const mcpTokenCreateInput = z.object({
  name: z.string().trim().min(1).max(100),
  accessLevel: mcpAccessLevelInput
});
