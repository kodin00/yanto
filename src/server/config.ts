import path from "node:path";

const requiredSecretFallback = "change-this-to-a-long-random-secret";
const unsafeSecretValues = new Set([requiredSecretFallback, "change-this-worker-token-secret"]);
const unsafeLegacyAdminPasswords = new Set(["admin", "change-this-admin-password"]);
const unsafeWorkerJoinTokens = new Set(["change-this-worker-join-token"]);
const projectsRoot = path.resolve(process.env.PROJECTS_ROOT ?? "/projects");
const agentWorktreesRoot = path.resolve(process.env.AGENT_WORKTREES_ROOT ?? path.join(projectsRoot, ".yanto-worktrees"));
const hostProjectsRoot = process.env.HOST_PROJECTS_ROOT ?? "~/projects";

function configuredSecret(...values: Array<string | undefined>) {
  return values.find((value) => value && !unsafeSecretValues.has(value)) ?? requiredSecretFallback;
}

function integerEnv(name: string, fallback: number, min: number, max = Number.MAX_SAFE_INTEGER) {
  const raw = process.env[name];
  if (raw === undefined || raw.trim() === "") return fallback;
  const value = Number(raw);
  if (!Number.isInteger(value) || value < min || value > max) {
    throw new Error(`${name} must be an integer between ${min} and ${max}.`);
  }
  return value;
}

export const config = {
  nodeEnv: process.env.NODE_ENV ?? "development",
  nodeRole: process.env.YANTO_NODE_ROLE ?? "master",
  localNodeId: process.env.YANTO_LOCAL_NODE_ID ?? "node_master_local",
  workerJoinToken: process.env.WORKER_JOIN_TOKEN ?? "",
  workerTokenSecret: process.env.WORKER_TOKEN_SECRET ?? process.env.JWT_SECRET ?? requiredSecretFallback,
  workerName: process.env.YANTO_WORKER_NAME ?? "",
  workerToken: process.env.YANTO_WORKER_TOKEN ?? "",
  workerTokenPath: process.env.YANTO_WORKER_TOKEN_PATH ?? "/data/worker-token",
  masterUrl: process.env.YANTO_MASTER_URL ?? "",
  workerPollIntervalMs: integerEnv("YANTO_WORKER_POLL_INTERVAL_MS", 3_000, 250, 5 * 60_000),
  port: integerEnv("PORT", 8_080, 1, 65_535),
  databaseUrl: process.env.DATABASE_URL ?? "postgres://yanto:yanto@localhost:5432/yanto",
  jwtSecret: process.env.JWT_SECRET ?? requiredSecretFallback,
  turnstileSiteKey: process.env.TURNSTILE_SITE_KEY?.trim() ?? "",
  turnstileSecretKey: process.env.TURNSTILE_SECRET_KEY ?? "",
  mcpTokenSecret: configuredSecret(process.env.MCP_TOKEN_SECRET, process.env.WORKER_TOKEN_SECRET, process.env.JWT_SECRET),
  mcpAllowedHosts: process.env.MCP_ALLOWED_HOSTS ?? "",
  mcpAllowedOrigins: process.env.MCP_ALLOWED_ORIGINS ?? "",
  initialSetupCode: process.env.YANTO_SETUP_CODE ?? "",
  legacyAdminUsername: process.env.ADMIN_USERNAME?.trim() ?? "",
  legacyAdminPassword: process.env.ADMIN_PASSWORD ?? "",
  projectsRoot,
  agentWorktreesRoot,
  hostAgentWorktreesRoot: process.env.HOST_AGENT_WORKTREES_ROOT ?? path.join(hostProjectsRoot, path.relative(projectsRoot, agentWorktreesRoot)),
  backupsDir: path.resolve(process.env.BACKUPS_DIR ?? "/data/backups"),
  cloudflaredDir: path.resolve(process.env.CLOUDFLARED_DIR ?? "/data/cloudflared"),
  frpDataDir: path.resolve(process.env.FRP_DATA_DIR ?? "/data/frp"),
  frpTokenPath: path.resolve(process.env.FRP_TOKEN_PATH ?? "/data/frp/auth-token"),
  frpDashboardUrl: process.env.FRP_DASHBOARD_URL ?? "http://frps:7500",
  frpContainerName: process.env.FRP_CONTAINER_NAME ?? "yanto-frps",
  frpBindPort: integerEnv("FRP_BIND_PORT", 7_000, 1, 65_535),
  frpPortStart: integerEnv("FRP_PORT_START", 25_560, 1, 65_535),
  frpPortEnd: integerEnv("FRP_PORT_END", 25_600, 1, 65_535),
  hostProjectsRoot,
  sshKeysDir: path.resolve(process.env.SSH_KEYS_DIR ?? "/tmp/yanto-ssh"),
  sshPrivateKeyPath: process.env.SSH_PRIVATE_KEY_PATH ?? "/root/.ssh/id_ed25519",
  managedSshPrivateKeyPath: path.resolve(process.env.MANAGED_SSH_PRIVATE_KEY_PATH ?? path.join(process.env.SSH_KEYS_DIR ?? "/tmp/yanto-ssh", "id_ed25519")),
  appBaseUrl: process.env.APP_BASE_URL ?? `http://localhost:${process.env.PORT ?? "8080"}`,
  backupUploadMaxBytes: integerEnv("BACKUP_UPLOAD_MAX_BYTES", 1024 * 1024 * 1024, 1),
  commandTimeoutMs: integerEnv("COMMAND_TIMEOUT_MS", 60 * 60 * 1_000, 1_000),
  commandOutputMaxBytes: integerEnv("COMMAND_OUTPUT_MAX_BYTES", 2 * 1024 * 1024, 16_384),
  deploymentLogMaxChars: integerEnv("DEPLOYMENT_LOG_MAX_CHARS", 500_000, 10_000),
  codexHome: path.resolve(process.env.CODEX_HOME ?? "/data/codex"),
  agentMaxConcurrentRuns: integerEnv("AGENT_MAX_CONCURRENT_RUNS", 2, 1, 1_000),
  agentMaxTurns: integerEnv("AGENT_MAX_TURNS", 40, 1, 10_000),
  agentRunTimeoutMs: integerEnv("AGENT_RUN_TIMEOUT_MS", 60 * 60 * 1_000, 60_000),
  agentShutdownTimeoutMs: integerEnv("AGENT_SHUTDOWN_TIMEOUT_MS", 30_000, 1_000),
  agentCommandTimeoutMs: integerEnv("AGENT_COMMAND_TIMEOUT_MS", 10 * 60 * 1_000, 1_000),
  agentCommandOutputMaxBytes: integerEnv("AGENT_COMMAND_OUTPUT_MAX_BYTES", 512 * 1024, 16_384),
  agentPersistedToolPayloadMaxBytes: integerEnv("AGENT_PERSISTED_TOOL_PAYLOAD_MAX_BYTES", 32 * 1024, 1_024),
  sshStrictHostKeyChecking: process.env.SSH_STRICT_HOST_KEY_CHECKING ?? "accept-new",
  cookieSecure:
    process.env.COOKIE_SECURE === undefined
      ? (process.env.NODE_ENV ?? "development") === "production" || (process.env.APP_BASE_URL ?? "").startsWith("https://")
      : process.env.COOKIE_SECURE === "true"
};

export function warnOnUnsafeDefaults() {
  if (!["master", "worker"].includes(config.nodeRole)) {
    console.warn(`YANTO_NODE_ROLE should be "master" or "worker"; got "${config.nodeRole}".`);
  }
  if (!Number.isInteger(config.frpBindPort) || config.frpBindPort < 1 || config.frpBindPort > 65535) {
    throw new Error("FRP_BIND_PORT must be an integer between 1 and 65535.");
  }
  if (!Number.isInteger(config.frpPortStart) || !Number.isInteger(config.frpPortEnd) || config.frpPortStart < 1 || config.frpPortEnd > 65535 || config.frpPortStart > config.frpPortEnd) {
    throw new Error("FRP_PORT_START and FRP_PORT_END must define a valid port range.");
  }
  if (config.nodeRole === "master" && config.jwtSecret === requiredSecretFallback) {
    console.warn("JWT_SECRET is using the default value. Set a strong secret before exposing this app.");
  }
  if (config.nodeRole === "master" && config.legacyAdminPassword && unsafeLegacyAdminPasswords.has(config.legacyAdminPassword)) {
    console.warn("Legacy ADMIN_PASSWORD is using a known placeholder value. Change it before database-owner migration.");
  }
  if (config.nodeRole === "master" && unsafeSecretValues.has(config.workerTokenSecret)) {
    console.warn("WORKER_TOKEN_SECRET is using the default value. Set a strong secret before enrolling workers.");
  }
  if (config.nodeRole === "master" && unsafeWorkerJoinTokens.has(config.workerJoinToken)) {
    console.warn("WORKER_JOIN_TOKEN is using a known placeholder value. Set a strong token before enrolling workers.");
  }
  if (config.nodeRole === "master" && config.mcpTokenSecret === requiredSecretFallback) {
    console.warn("MCP_TOKEN_SECRET/WORKER_TOKEN_SECRET/JWT_SECRET is using the default value. Set a stable strong secret before creating MCP tokens.");
  }
  if (config.nodeRole === "master" && Boolean(config.turnstileSiteKey) !== Boolean(config.turnstileSecretKey)) {
    console.warn("Turnstile is disabled because TURNSTILE_SITE_KEY and TURNSTILE_SECRET_KEY must both be set.");
  }
  if (config.appBaseUrl.startsWith("https://") && !config.cookieSecure) {
    console.warn("COOKIE_SECURE is disabled even though APP_BASE_URL uses HTTPS. Session cookies may cross the network without the Secure attribute.");
  }
}
