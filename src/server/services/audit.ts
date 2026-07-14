import { desc, eq, inArray } from "drizzle-orm";
import { db } from "../db/index.js";
import { auditLogs } from "../db/schema.js";
import { createId } from "./tokens.js";

export type AuditInput = {
  actor?: string;
  action: string;
  entityType: string;
  entityId?: string | null;
  projectId?: string | null;
  metadata?: Record<string, unknown>;
};

export async function recordAuditLog(input: AuditInput) {
  try {
    const [log] = await db
      .insert(auditLogs)
      .values({
        id: createId("aud"),
        actor: input.actor ?? "admin",
        action: input.action,
        entityType: input.entityType,
        entityId: input.entityId ?? null,
        projectId: input.projectId ?? null,
        metadata: input.metadata ?? {},
        createdAt: new Date()
      })
      .returning();
    return log;
  } catch {
    return undefined;
  }
}

export async function listAuditLogs(limit = 100, projectId?: string) {
  const safeLimit = Math.min(limit, 500);
  const query = db.select().from(auditLogs);
  if (projectId) {
    return query.where(eq(auditLogs.projectId, projectId)).orderBy(desc(auditLogs.createdAt)).limit(safeLimit);
  }
  return query.orderBy(desc(auditLogs.createdAt)).limit(safeLimit);
}

export async function listAuditLogsForProjects(projectIds: string[], limit = 100) {
  if (projectIds.length === 0) return [];
  return db.select().from(auditLogs).where(inArray(auditLogs.projectId, projectIds))
    .orderBy(desc(auditLogs.createdAt)).limit(Math.min(limit, 500));
}
