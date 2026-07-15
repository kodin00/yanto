import crypto from "node:crypto";
import { asc, eq, sql } from "drizzle-orm";
import type { AppView, ManagedUser, ProjectPermission, SessionUser, UserRole, UserStatus } from "../../shared/types.js";
import type { AuthPrincipal, ProjectAccessInput } from "../account-types.js";
import { config } from "../config.js";
import { db, pool } from "../db/index.js";
import { projects, userProjectAccess, users } from "../db/schema.js";
import { HttpError } from "../http-utils.js";
import { logger } from "../logger.js";
import { hashPassword, passwordHashNeedsUpgrade, verifyPassword } from "./passwords.js";
import { constantTimeEqual, createId, hashToken } from "./tokens.js";

export const PROJECT_PERMISSIONS = ["deploy", "runtime", "config", "secrets", "backups", "hostnames"] as const satisfies readonly ProjectPermission[];
const PROJECT_PERMISSION_SET = new Set<string>(PROJECT_PERMISSIONS);
const ACCOUNT_TOKEN_TTL_MS = 24 * 60 * 60 * 1_000;
const generatedSetupCode = crypto.randomBytes(18).toString("base64url");
const setupCode = config.initialSetupCode || generatedSetupCode;
let setupCodeLogged = false;

const ALL_VIEWS: AppView[] = ["dashboard", "projects", "tasks", "deployments", "containers", "nodes", "backups", "hostnames", "frp", "dns", "audit", "settings"];
const MEMBER_BASE_VIEWS: AppView[] = ["dashboard", "projects", "deployments", "containers", "audit"];

function validRole(value: string): value is UserRole {
  return value === "owner" || value === "member";
}

function validStatus(value: string): value is UserStatus {
  return value === "invited" || value === "active" || value === "disabled";
}

export function sanitizePermissions(values: unknown): ProjectPermission[] {
  if (!Array.isArray(values)) return [];
  return [...new Set(values.filter((value): value is ProjectPermission => typeof value === "string" && PROJECT_PERMISSION_SET.has(value)))];
}

function normalizeUsername(username: string) {
  const normalized = username.trim();
  if (!normalized) throw new HttpError(400, "Username cannot be empty.");
  return normalized;
}

function accountUrl(token: string) {
  return `${config.appBaseUrl.replace(/\/$/, "")}/account/setup#token=${encodeURIComponent(token)}`;
}

function createAccountSecret() {
  return crypto.randomBytes(32).toString("base64url");
}

function accountTokenHash(token: string) {
  return hashToken(token, config.jwtSecret);
}

function allowedViews(principal: AuthPrincipal): AppView[] {
  if (principal.role === "owner") return ALL_VIEWS;
  const permissions = new Set(principal.projectAccess.flatMap((access) => access.permissions));
  const views = [...MEMBER_BASE_VIEWS];
  if (permissions.has("backups")) views.push("backups");
  if (permissions.has("hostnames")) views.push("hostnames");
  return views;
}

export function toSessionUser(principal: AuthPrincipal): SessionUser {
  return {
    id: principal.id,
    username: principal.username,
    role: principal.role,
    status: principal.status,
    projectAccess: principal.projectAccess,
    allowedViews: allowedViews(principal),
    appBaseUrl: config.appBaseUrl,
    localNodeId: config.localNodeId
  };
}

export async function loadPrincipalByUserId(userId: string): Promise<AuthPrincipal | null> {
  const [user] = await db.select().from(users).where(eq(users.id, userId)).limit(1);
  if (!user || !validRole(user.role) || !validStatus(user.status)) return null;
  const grants = await db
    .select({ projectId: userProjectAccess.projectId, projectName: projects.name, permissions: userProjectAccess.permissions })
    .from(userProjectAccess)
    .innerJoin(projects, eq(projects.id, userProjectAccess.projectId))
    .where(eq(userProjectAccess.userId, userId))
    .orderBy(asc(projects.name));
  return {
    id: user.id,
    username: user.username,
    role: user.role,
    status: user.status,
    sessionVersion: user.sessionVersion,
    projectAccess: grants.map((grant) => ({ ...grant, permissions: sanitizePermissions(grant.permissions) }))
  };
}

export async function authenticateUser(username: string, password: string) {
  const normalized = username.trim();
  if (!normalized || password.length === 0) return null;
  const [user] = await db.select().from(users).where(sql`lower(${users.username}) = lower(${normalized})`).limit(1);
  if (!user || user.status !== "active" || !user.passwordHash || !(await verifyPassword(password, user.passwordHash))) return null;
  const now = new Date();
  const passwordHash = passwordHashNeedsUpgrade(user.passwordHash) ? await hashPassword(password) : user.passwordHash;
  await db.update(users).set({ passwordHash, lastLoginAt: now, updatedAt: now }).where(eq(users.id, user.id));
  return loadPrincipalByUserId(user.id);
}

export async function setupStatus() {
  const [row] = await db.select({ count: sql<number>`count(*)::int` }).from(users);
  return { needsSetup: (row?.count ?? 0) === 0 };
}

export async function logSetupCodeIfNeeded() {
  if (setupCodeLogged || !(await setupStatus()).needsSetup) return;
  setupCodeLogged = true;
  logger.warn("initial owner setup is required", { setupCode, setupUrl: `${config.appBaseUrl.replace(/\/$/, "")}/setup` });
}

export async function createInitialOwner(input: { username: string; password: string; setupCode: string }) {
  if (!constantTimeEqual(input.setupCode, setupCode)) throw new HttpError(403, "Invalid setup code.");
  const username = normalizeUsername(input.username);
  const passwordHash = await hashPassword(input.password);
  const client = await pool.connect();
  const id = createId("usr");
  try {
    await client.query("BEGIN");
    await client.query(`SELECT pg_advisory_xact_lock(hashtext('yanto.initial-owner'))`);
    const existing = await client.query(`SELECT 1 FROM users LIMIT 1`);
    if (existing.rowCount) throw new HttpError(409, "Owner setup has already been completed.");
    await client.query(
      `INSERT INTO users (id, username, role, status, password_hash) VALUES ($1, $2, 'owner', 'active', $3)`,
      [id, username, passwordHash]
    );
    await client.query("COMMIT");
  } catch (error) {
    await client.query("ROLLBACK");
    if ((error as { code?: string }).code === "23505") throw new HttpError(409, "Username is already in use.");
    throw error;
  } finally {
    client.release();
  }
  return (await loadPrincipalByUserId(id))!;
}

async function issueAccountToken(userId: string, purpose: "invite" | "reset", clearPassword: boolean) {
  const token = createAccountSecret();
  const now = new Date();
  const expiresAt = new Date(now.getTime() + ACCOUNT_TOKEN_TTL_MS);
  const client = await pool.connect();
  try {
    await client.query("BEGIN");
    const user = await client.query<{ id: string }>(`SELECT id FROM users WHERE id = $1 FOR UPDATE`, [userId]);
    if (!user.rowCount) throw new HttpError(404, "User not found.");
    await client.query(`UPDATE account_tokens SET used_at = $2 WHERE user_id = $1 AND used_at IS NULL`, [userId, now]);
    if (clearPassword) {
      await client.query(`UPDATE users SET password_hash = NULL, status = 'invited', session_version = session_version + 1, updated_at = $2 WHERE id = $1`, [userId, now]);
    }
    await client.query(
      `INSERT INTO account_tokens (id, user_id, purpose, token_hash, expires_at, created_at) VALUES ($1, $2, $3, $4, $5, $6)`,
      [createId("act"), userId, purpose, accountTokenHash(token), expiresAt, now]
    );
    await client.query("COMMIT");
    return accountUrl(token);
  } catch (error) {
    await client.query("ROLLBACK");
    throw error;
  } finally {
    client.release();
  }
}

function validateProjectAccess(access: ProjectAccessInput[]) {
  const seen = new Set<string>();
  return access.map((grant) => {
    if (seen.has(grant.projectId)) throw new HttpError(400, "Each project may only be assigned once.");
    seen.add(grant.projectId);
    return { projectId: grant.projectId, permissions: sanitizePermissions(grant.permissions) };
  });
}

async function replaceProjectAccessWithClient(client: import("pg").PoolClient, userId: string, access: ProjectAccessInput[]) {
  const validated = validateProjectAccess(access);
  await client.query(`DELETE FROM user_project_access WHERE user_id = $1`, [userId]);
  for (const grant of validated) {
    await client.query(
      `INSERT INTO user_project_access (user_id, project_id, permissions) VALUES ($1, $2, $3::jsonb)`,
      [userId, grant.projectId, JSON.stringify(grant.permissions)]
    );
  }
}

export async function createMember(input: { username: string; projectAccess: ProjectAccessInput[] }) {
  const username = normalizeUsername(input.username);
  const id = createId("usr");
  const now = new Date();
  const token = createAccountSecret();
  const client = await pool.connect();
  try {
    await client.query("BEGIN");
    await client.query(
      `INSERT INTO users (id, username, role, status, password_hash, created_at, updated_at) VALUES ($1, $2, 'member', 'invited', NULL, $3, $3)`,
      [id, username, now]
    );
    await replaceProjectAccessWithClient(client, id, input.projectAccess);
    await client.query(
      `INSERT INTO account_tokens (id, user_id, purpose, token_hash, expires_at, created_at) VALUES ($1, $2, 'invite', $3, $4, $5)`,
      [createId("act"), id, accountTokenHash(token), new Date(now.getTime() + ACCOUNT_TOKEN_TTL_MS), now]
    );
    await client.query("COMMIT");
  } catch (error) {
    await client.query("ROLLBACK");
    if ((error as { code?: string }).code === "23505") throw new HttpError(409, "Username is already in use.");
    if ((error as { code?: string }).code === "23503") throw new HttpError(400, "One or more projects do not exist.");
    throw error;
  } finally {
    client.release();
  }
  return { user: (await managedUser(id))!, setupUrl: accountUrl(token) };
}

export async function replaceProjectAccess(userId: string, access: ProjectAccessInput[]) {
  const client = await pool.connect();
  try {
    await client.query("BEGIN");
    const found = await client.query<{ role: string }>(`SELECT role FROM users WHERE id = $1 FOR UPDATE`, [userId]);
    if (!found.rows[0]) throw new HttpError(404, "User not found.");
    if (found.rows[0].role === "owner") throw new HttpError(400, "The owner has access to every project.");
    await replaceProjectAccessWithClient(client, userId, access);
    await client.query(`UPDATE users SET updated_at = now() WHERE id = $1`, [userId]);
    await client.query("COMMIT");
  } catch (error) {
    await client.query("ROLLBACK");
    if ((error as { code?: string }).code === "23503") throw new HttpError(400, "One or more projects do not exist.");
    throw error;
  } finally {
    client.release();
  }
  return (await managedUser(userId))!;
}

export async function setUserStatus(userId: string, status: "active" | "disabled") {
  const client = await pool.connect();
  try {
    await client.query("BEGIN");
    const found = await client.query<{ role: string; password_hash: string | null }>(`SELECT role, password_hash FROM users WHERE id = $1 FOR UPDATE`, [userId]);
    const user = found.rows[0];
    if (!user) throw new HttpError(404, "User not found.");
    if (user.role === "owner") throw new HttpError(400, "The owner cannot be disabled.");
    if (status === "active" && !user.password_hash) throw new HttpError(409, "The user must complete account setup before being enabled.");
    await client.query(`UPDATE users SET status = $2, session_version = session_version + 1, updated_at = now() WHERE id = $1`, [userId, status]);
    if (status === "disabled") {
      await client.query(`UPDATE account_tokens SET used_at = now() WHERE user_id = $1 AND used_at IS NULL`, [userId]);
    }
    await client.query("COMMIT");
  } catch (error) {
    await client.query("ROLLBACK");
    throw error;
  } finally {
    client.release();
  }
  return (await managedUser(userId))!;
}

export async function createResetLink(userId: string) {
  return issueAccountToken(userId, "reset", true);
}

export async function accountSetupDetails(token: string) {
  if (!token) throw new HttpError(400, "Account token is required.");
  const result = await pool.query<{ username: string }>(
    `SELECT u.username
     FROM account_tokens t
     INNER JOIN users u ON u.id = t.user_id
     WHERE t.token_hash = $1
       AND t.used_at IS NULL
       AND t.expires_at > now()
       AND u.status <> 'disabled'
     LIMIT 1`,
    [accountTokenHash(token)]
  );
  const account = result.rows[0];
  if (!account) throw new HttpError(400, "Account token is invalid or expired.");
  return { username: account.username };
}

export async function completeAccountSetup(token: string, password: string) {
  if (!token) throw new HttpError(400, "Account token is required.");
  const passwordHash = await hashPassword(password);
  const now = new Date();
  const client = await pool.connect();
  let userId: string;
  try {
    await client.query("BEGIN");
    const tokenIdentity = await client.query<{ user_id: string }>(
      `SELECT user_id FROM account_tokens WHERE token_hash = $1`,
      [accountTokenHash(token)]
    );
    userId = tokenIdentity.rows[0]?.user_id;
    if (!userId) throw new HttpError(400, "Account token is invalid or expired.");
    const userResult = await client.query<{ status: string }>(`SELECT status FROM users WHERE id = $1 FOR UPDATE`, [userId]);
    if (!userResult.rows[0] || userResult.rows[0].status === "disabled") {
      throw new HttpError(400, "Account token is invalid or expired.");
    }
    const result = await client.query<{ id: string; user_id: string; expires_at: Date; used_at: Date | null }>(
      `SELECT id, user_id, expires_at, used_at FROM account_tokens WHERE token_hash = $1 FOR UPDATE`,
      [accountTokenHash(token)]
    );
    const accountToken = result.rows[0];
    if (!accountToken || accountToken.used_at || accountToken.expires_at.getTime() <= now.getTime()) {
      throw new HttpError(400, "Account token is invalid or expired.");
    }
    await client.query(`UPDATE account_tokens SET used_at = $2 WHERE id = $1`, [accountToken.id, now]);
    await client.query(`UPDATE account_tokens SET used_at = $2 WHERE user_id = $1 AND used_at IS NULL`, [userId, now]);
    await client.query(
      `UPDATE users SET password_hash = $2, status = 'active', session_version = session_version + 1, updated_at = $3 WHERE id = $1`,
      [userId, passwordHash, now]
    );
    await client.query("COMMIT");
  } catch (error) {
    await client.query("ROLLBACK");
    throw error;
  } finally {
    client.release();
  }
  return (await loadPrincipalByUserId(userId!))!;
}

async function managedUser(userId: string): Promise<ManagedUser | null> {
  const [row] = await db.select().from(users).where(eq(users.id, userId)).limit(1);
  const principal = await loadPrincipalByUserId(userId);
  if (!row || !principal) return null;
  return {
    id: principal.id,
    username: principal.username,
    role: principal.role,
    status: principal.status,
    projectAccess: principal.projectAccess,
    lastLoginAt: row.lastLoginAt?.toISOString() ?? null,
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString()
  };
}

export async function listManagedUsers() {
  const rows = await db.select({ id: users.id }).from(users).orderBy(asc(users.createdAt));
  return (await Promise.all(rows.map((row) => managedUser(row.id)))).filter((user): user is ManagedUser => Boolean(user));
}

export async function deleteMember(userId: string) {
  const client = await pool.connect();
  try {
    await client.query("BEGIN");
    const found = await client.query<{ role: string; username: string }>(
      `SELECT role, username FROM users WHERE id = $1 FOR UPDATE`,
      [userId]
    );
    const user = found.rows[0];
    if (!user) throw new HttpError(404, "User not found.");
    if (user.role === "owner") throw new HttpError(400, "The owner cannot be removed.");
    await client.query(`DELETE FROM users WHERE id = $1`, [userId]);
    await client.query("COMMIT");
    return { id: userId, username: user.username };
  } catch (error) {
    await client.query("ROLLBACK");
    throw error;
  } finally {
    client.release();
  }
}

export async function recoverOwnerAccount() {
  const [owner] = await db.select({ id: users.id }).from(users).where(eq(users.role, "owner")).limit(1);
  if (!owner) throw new Error("No owner account exists. Complete initial setup first.");
  return createResetLink(owner.id);
}
