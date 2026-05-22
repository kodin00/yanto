import path from "node:path";

const requiredSecretFallback = "change-this-to-a-long-random-secret";

export const config = {
  nodeEnv: process.env.NODE_ENV ?? "development",
  port: Number(process.env.PORT ?? "8080"),
  databaseUrl: process.env.DATABASE_URL ?? "postgres://yanto:yanto@localhost:5432/yanto",
  jwtSecret: process.env.JWT_SECRET ?? requiredSecretFallback,
  adminUsername: process.env.ADMIN_USERNAME ?? "admin",
  adminPassword: process.env.ADMIN_PASSWORD ?? "admin",
  projectsRoot: path.resolve(process.env.PROJECTS_ROOT ?? "/projects"),
  hostProjectsRoot: process.env.HOST_PROJECTS_ROOT ?? "~/projects",
  sshKeysDir: path.resolve(process.env.SSH_KEYS_DIR ?? "/tmp/yanto-ssh"),
  appBaseUrl: process.env.APP_BASE_URL ?? `http://localhost:${process.env.PORT ?? "8080"}`,
  cookieSecure:
    process.env.COOKIE_SECURE === undefined
      ? (process.env.APP_BASE_URL ?? "").startsWith("https://")
      : process.env.COOKIE_SECURE === "true"
};

export function warnOnUnsafeDefaults() {
  if (config.nodeEnv === "production" && config.jwtSecret === requiredSecretFallback) {
    console.warn("JWT_SECRET is using the default value. Set a strong secret before exposing this app.");
  }
  if (config.nodeEnv === "production" && config.adminPassword === "change-this-admin-password") {
    console.warn("ADMIN_PASSWORD is using the default value. Set a strong admin password.");
  }
}
