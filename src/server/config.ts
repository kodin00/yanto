import path from "node:path";

const requiredSecretFallback = "change-this-to-a-long-random-secret";

function configuredSecret(...values: Array<string | undefined>) {
  return values.find((value) => value && value !== requiredSecretFallback) ?? requiredSecretFallback;
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
  workerPollIntervalMs: Number(process.env.YANTO_WORKER_POLL_INTERVAL_MS ?? 3000),
  port: Number(process.env.PORT ?? "8080"),
  databaseUrl: process.env.DATABASE_URL ?? "postgres://yanto:yanto@localhost:5432/yanto",
  jwtSecret: process.env.JWT_SECRET ?? requiredSecretFallback,
  mcpTokenSecret: configuredSecret(process.env.MCP_TOKEN_SECRET, process.env.WORKER_TOKEN_SECRET, process.env.JWT_SECRET),
  mcpAllowedHosts: process.env.MCP_ALLOWED_HOSTS ?? "",
  mcpAllowedOrigins: process.env.MCP_ALLOWED_ORIGINS ?? "",
  adminUsername: process.env.ADMIN_USERNAME ?? "admin",
  adminPassword: process.env.ADMIN_PASSWORD ?? "admin",
  projectsRoot: path.resolve(process.env.PROJECTS_ROOT ?? "/projects"),
  backupsDir: path.resolve(process.env.BACKUPS_DIR ?? "/data/backups"),
  cloudflaredDir: path.resolve(process.env.CLOUDFLARED_DIR ?? "/data/cloudflared"),
  frpDataDir: path.resolve(process.env.FRP_DATA_DIR ?? "/data/frp"),
  frpTokenPath: path.resolve(process.env.FRP_TOKEN_PATH ?? "/data/frp/auth-token"),
  frpDashboardUrl: process.env.FRP_DASHBOARD_URL ?? "http://frps:7500",
  frpContainerName: process.env.FRP_CONTAINER_NAME ?? "yanto-frps",
  frpBindPort: Number(process.env.FRP_BIND_PORT ?? 7000),
  frpPortStart: Number(process.env.FRP_PORT_START ?? 25560),
  frpPortEnd: Number(process.env.FRP_PORT_END ?? 25600),
  hostProjectsRoot: process.env.HOST_PROJECTS_ROOT ?? "~/projects",
  sshKeysDir: path.resolve(process.env.SSH_KEYS_DIR ?? "/tmp/yanto-ssh"),
  sshPrivateKeyPath: process.env.SSH_PRIVATE_KEY_PATH ?? "/root/.ssh/id_ed25519",
  managedSshPrivateKeyPath: path.resolve(process.env.MANAGED_SSH_PRIVATE_KEY_PATH ?? path.join(process.env.SSH_KEYS_DIR ?? "/tmp/yanto-ssh", "id_ed25519")),
  appBaseUrl: process.env.APP_BASE_URL ?? `http://localhost:${process.env.PORT ?? "8080"}`,
  backupUploadMaxBytes: Number(process.env.BACKUP_UPLOAD_MAX_BYTES ?? 1024 * 1024 * 1024),
  commandTimeoutMs: Number(process.env.COMMAND_TIMEOUT_MS ?? 60 * 60 * 1000),
  commandOutputMaxBytes: Number(process.env.COMMAND_OUTPUT_MAX_BYTES ?? 2 * 1024 * 1024),
  deploymentLogMaxChars: Number(process.env.DEPLOYMENT_LOG_MAX_CHARS ?? 500_000),
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
    if (config.nodeEnv === "production") {
      throw new Error("FATAL: JWT_SECRET is using the default value. Set a strong secret before running in production.");
    }
    console.warn("JWT_SECRET is using the default value. Set a strong secret before exposing this app.");
  }
  if (config.nodeRole === "master" && config.adminPassword === "admin") {
    if (config.nodeEnv === "production") {
      throw new Error("FATAL: ADMIN_PASSWORD is using the default value. Set a strong admin password before running in production.");
    }
    console.warn("ADMIN_PASSWORD is using the default value. Set a strong admin password.");
  }
  if (config.nodeRole === "master" && config.mcpTokenSecret === requiredSecretFallback) {
    console.warn("MCP_TOKEN_SECRET/WORKER_TOKEN_SECRET/JWT_SECRET is using the default value. Set a stable strong secret before creating MCP tokens.");
  }
}
