import { Router } from "express";
import { z } from "zod";
import { requireOwner } from "../auth.js";
import { asyncRoute, actor, routeParam } from "../http-utils.js";
import {
  PROJECT_PERMISSIONS,
  createMember,
  createResetLink,
  listManagedUsers,
  replaceProjectAccess,
  setUserStatus
} from "../services/accounts.js";
import { recordAuditLog } from "../services/audit.js";

const router = Router();
const permission = z.enum(PROJECT_PERMISSIONS);
const projectAccess = z.array(z.object({
  projectId: z.string().min(1).max(200),
  permissions: z.array(permission)
})).max(10_000);

router.get("/api/users", requireOwner, asyncRoute(async (_req, res) => {
  res.json(await listManagedUsers());
}));

router.post("/api/users", requireOwner, asyncRoute(async (req, res) => {
  const body = z.object({
    username: z.string().trim().min(1).max(200),
    projectAccess
  }).parse(req.body);
  const result = await createMember(body);
  await recordAuditLog({ actor: actor(req), action: "user.create", entityType: "user", entityId: result.user.id, metadata: { username: result.user.username } });
  res.status(201).json(result);
}));

router.put("/api/users/:id/access", requireOwner, asyncRoute(async (req, res) => {
  const body = z.object({ projectAccess }).parse(req.body);
  const id = routeParam(req, "id");
  const user = await replaceProjectAccess(id, body.projectAccess);
  await recordAuditLog({ actor: actor(req), action: "user.access.replace", entityType: "user", entityId: id, metadata: { projectCount: user.projectAccess.length } });
  res.json(user);
}));

router.patch("/api/users/:id/status", requireOwner, asyncRoute(async (req, res) => {
  const body = z.object({ status: z.enum(["active", "disabled"]) }).parse(req.body);
  const id = routeParam(req, "id");
  const user = await setUserStatus(id, body.status);
  await recordAuditLog({ actor: actor(req), action: `user.${body.status}`, entityType: "user", entityId: id });
  res.json(user);
}));

router.post("/api/users/:id/reset-link", requireOwner, asyncRoute(async (req, res) => {
  const id = routeParam(req, "id");
  const resetUrl = await createResetLink(id);
  await recordAuditLog({ actor: actor(req), action: "user.reset_link.create", entityType: "user", entityId: id });
  res.json({ resetUrl });
}));

export default router;
