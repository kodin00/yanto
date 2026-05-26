import path from "node:path";

const requiredSecretFallback = "change-this-to-a-long-random-secret";

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
  adminUsername: process.env.ADMIN_USERNAME ?? "admin",
  adminPassword: process.env.ADMIN_PASSWORD ?? "admin",
  projectsRoot: path.resolve(process.env.PROJECTS_ROOT ?? "/projects"),
  backupsDir: path.resolve(process.env.BACKUPS_DIR ?? "/data/backups"),
  hostProjectsRoot: process.env.HOST_PROJECTS_ROOT ?? "~/projects",
  sshKeysDir: path.resolve(process.env.SSH_KEYS_DIR ?? "/tmp/yanto-ssh"),
  sshPrivateKeyPath: process.env.SSH_PRIVATE_KEY_PATH ?? "/root/.ssh/id_ed25519",
  managedSshPrivateKeyPath: path.resolve(process.env.MANAGED_SSH_PRIVATE_KEY_PATH ?? path.join(process.env.SSH_KEYS_DIR ?? "/tmp/yanto-ssh", "id_ed25519")),
  appBaseUrl: process.env.APP_BASE_URL ?? `http://localhost:${process.env.PORT ?? "8080"}`,
  backupUploadMaxBytes: Number(process.env.BACKUP_UPLOAD_MAX_BYTES ?? 1024 * 1024 * 1024),
  commandTimeoutMs: Number(process.env.COMMAND_TIMEOUT_MS ?? 60 * 60 * 1000),
  commandOutputMaxBytes: Number(process.env.COMMAND_OUTPUT_MAX_BYTES ?? 2 * 1024 * 1024),
  deploymentLogMaxChars: Number(process.env.DEPLOYMENT_LOG_MAX_CHARS ?? 500_000),
  cookieSecure:
    process.env.COOKIE_SECURE === undefined
      ? (process.env.APP_BASE_URL ?? "").startsWith("https://")
      : process.env.COOKIE_SECURE === "true"
};

export function warnOnUnsafeDefaults() {
  if (!["master", "worker"].includes(config.nodeRole)) {
    console.warn(`YANTO_NODE_ROLE should be "master" or "worker"; got "${config.nodeRole}".`);
  }
  if (config.nodeEnv === "production" && config.jwtSecret === requiredSecretFallback) {
    console.warn("JWT_SECRET is using the default value. Set a strong secret before exposing this app.");
  }
  if (config.nodeEnv === "production" && config.adminPassword === "change-this-admin-password") {
    console.warn("ADMIN_PASSWORD is using the default value. Set a strong admin password.");
  }
}
