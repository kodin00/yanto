import crypto from "node:crypto";
import { and, desc, eq, isNull } from "drizzle-orm";
import { config } from "../config.js";
import { db } from "../db/index.js";
import { mcpAccessTokens, type McpAccessTokenRow } from "../db/schema.js";
import type { McpAccessLevel, McpAccessToken } from "../../shared/types.js";
import { constantTimeEqual, createId, hashToken } from "./tokens.js";

const TOKEN_PREFIX = "ymcp";
const accessRank: Record<McpAccessLevel, number> = {
  read: 0,
  write: 1,
  admin: 2
};

export type McpAuthContext = {
  tokenId: string;
  tokenName: string;
  accessLevel: McpAccessLevel;
  actor: string;
};

export function isMcpAccessLevel(value: unknown): value is McpAccessLevel {
  return value === "read" || value === "write" || value === "admin";
}

export function hasMcpAccess(actual: McpAccessLevel, required: McpAccessLevel) {
  return accessRank[actual] >= accessRank[required];
}

export function publicMcpAccessToken(row: McpAccessTokenRow): McpAccessToken {
  return {
    id: row.id,
    name: row.name,
    accessLevel: isMcpAccessLevel(row.accessLevel) ? row.accessLevel : "read",
    lastUsedAt: row.lastUsedAt?.toISOString() ?? null,
    revokedAt: row.revokedAt?.toISOString() ?? null,
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString()
  };
}

function createMcpSecret() {
  return crypto.randomBytes(32).toString("base64url");
}

function tokenHash(id: string, secret: string) {
  return hashToken(`${id}.${secret}`, config.mcpTokenSecret);
}

export function parseMcpToken(token: string) {
  const trimmed = token.trim();
  if (!trimmed.startsWith(`${TOKEN_PREFIX}_`)) {
    return null;
  }
  const parts = trimmed.split("_");
  if (parts.length < 3) {
    return null;
  }
  const [, idPrefix, ...secretParts] = parts;
  const id = `${TOKEN_PREFIX}_${idPrefix}`;
  const secret = secretParts.join("_");
  if (!idPrefix || !secret) {
    return null;
  }
  return { id, secret };
}

export async function listMcpAccessTokens() {
  const rows = await db.select().from(mcpAccessTokens).orderBy(desc(mcpAccessTokens.createdAt));
  return rows.map(publicMcpAccessToken);
}

export async function createMcpAccessToken(input: { name: string; accessLevel: McpAccessLevel }) {
  const id = createId(TOKEN_PREFIX);
  const secret = createMcpSecret();
  const now = new Date();
  const [row] = await db
    .insert(mcpAccessTokens)
    .values({
      id,
      name: input.name.trim(),
      accessLevel: input.accessLevel,
      tokenHash: tokenHash(id, secret),
      createdAt: now,
      updatedAt: now
    })
    .returning();

  return {
    token: `${id}_${secret}`,
    accessToken: publicMcpAccessToken(row)
  };
}

export async function revokeMcpAccessToken(id: string) {
  const now = new Date();
  const [row] = await db
    .update(mcpAccessTokens)
    .set({ revokedAt: now, updatedAt: now })
    .where(eq(mcpAccessTokens.id, id))
    .returning();
  return row ? publicMcpAccessToken(row) : undefined;
}

export async function authenticateMcpToken(token: string): Promise<McpAuthContext | null> {
  const parsed = parseMcpToken(token);
  if (!parsed) {
    return null;
  }

  const [row] = await db
    .select()
    .from(mcpAccessTokens)
    .where(and(eq(mcpAccessTokens.id, parsed.id), isNull(mcpAccessTokens.revokedAt)))
    .limit(1);

  if (!row || !constantTimeEqual(row.tokenHash, tokenHash(parsed.id, parsed.secret)) || !isMcpAccessLevel(row.accessLevel)) {
    return null;
  }

  await db.update(mcpAccessTokens).set({ lastUsedAt: new Date(), updatedAt: new Date() }).where(eq(mcpAccessTokens.id, row.id));

  return {
    tokenId: row.id,
    tokenName: row.name,
    accessLevel: row.accessLevel,
    actor: `mcp:${row.name}`
  };
}

export function bearerTokenFromHeader(header: string | undefined) {
  if (!header) {
    return null;
  }
  const match = header.match(/^Bearer\s+(.+)$/i);
  return match?.[1]?.trim() || null;
}
