import { Router } from "express";
import rateLimit from "express-rate-limit";
import { z } from "zod";
import { clearSessionCookie, requireAuth, setSessionCookie } from "../auth.js";
import { asyncRoute, actor } from "../http-utils.js";
import {
  authenticateUser,
  completeAccountSetup,
  createInitialOwner,
  setupStatus,
  toSessionUser
} from "../services/accounts.js";
import { recordAuditLog } from "../services/audit.js";

const router = Router();

const loginLimiter = rateLimit({
  windowMs: 15 * 60 * 1000,
  limit: 5,
  standardHeaders: "draft-7",
  legacyHeaders: false,
  skipSuccessfulRequests: true,
  message: { message: "Too many login attempts. Try again later." }
});

const accountSetupLimiter = rateLimit({
  windowMs: 15 * 60 * 1000,
  limit: 10,
  standardHeaders: "draft-7",
  legacyHeaders: false,
  skipSuccessfulRequests: true,
  message: { message: "Too many account setup attempts. Try again later." }
});

const password = z.string().min(1, "Password cannot be empty.");

router.get("/api/setup/status", asyncRoute(async (_req, res) => {
  res.json(await setupStatus());
}));

router.post("/api/setup/owner", accountSetupLimiter, asyncRoute(async (req, res) => {
  const body = z.object({
    username: z.string().trim().min(1).max(200),
    password,
    setupCode: z.string().min(1).max(1_000)
  }).parse(req.body);
  const principal = await createInitialOwner(body);
  setSessionCookie(res, principal);
  await recordAuditLog({ actor: principal.username, action: "auth.setup.owner", entityType: "user", entityId: principal.id });
  res.status(201).json(toSessionUser(principal));
}));

router.post(
  "/api/auth/login",
  loginLimiter,
  asyncRoute(async (req, res) => {
    const body = z.object({
      username: z.string().max(200),
      password
    }).parse(req.body);
    const principal = await authenticateUser(body.username, body.password);
    if (!principal) {
      await recordAuditLog({ actor: body.username || "unknown", action: "auth.login.failed", entityType: "auth", metadata: { username: body.username } });
      res.status(401).json({ message: "Invalid username or password." });
      return;
    }
    setSessionCookie(res, principal);
    await recordAuditLog({ actor: principal.username, action: "auth.login.success", entityType: "auth", entityId: principal.id, metadata: { username: principal.username } });
    res.json(toSessionUser(principal));
  })
);

router.post("/api/auth/account/setup", accountSetupLimiter, asyncRoute(async (req, res) => {
  const body = z.object({ token: z.string().min(1).max(2_000), password }).parse(req.body);
  const principal = await completeAccountSetup(body.token, body.password);
  setSessionCookie(res, principal);
  await recordAuditLog({ actor: principal.username, action: "auth.account_setup.complete", entityType: "user", entityId: principal.id });
  res.json(toSessionUser(principal));
}));

router.post("/api/auth/logout", asyncRoute(async (req, res) => {
  await recordAuditLog({ actor: actor(req), action: "auth.logout", entityType: "auth" });
  clearSessionCookie(res);
  res.json({ ok: true });
}));

router.get("/api/auth/me", requireAuth, (req, res) => {
  const principal = req.yantoAuth;
  if (!principal) {
    res.status(401).json({ message: "Authentication required." });
    return;
  }
  res.json(toSessionUser(principal));
});

export default router;
