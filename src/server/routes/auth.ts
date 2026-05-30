import { Router } from "express";
import rateLimit from "express-rate-limit";
import { z } from "zod";
import { clearSessionCookie, currentUser, setSessionCookie, verifyAdminPassword } from "../auth.js";
import { config } from "../config.js";
import { asyncRoute, actor } from "../http-utils.js";
import { recordAuditLog } from "../services/audit.js";

const router = Router();

const loginLimiter = rateLimit({
  windowMs: 15 * 60 * 1000,
  limit: 5,
  standardHeaders: "draft-7",
  legacyHeaders: false,
  message: { message: "Too many login attempts. Try again later." }
});

router.post(
  "/api/auth/login",
  loginLimiter,
  asyncRoute(async (req, res) => {
    const body = z.object({ username: z.string(), password: z.string() }).parse(req.body);
    const ok = await verifyAdminPassword(body.username, body.password);
    if (!ok) {
      await recordAuditLog({ actor: body.username || "unknown", action: "auth.login.failed", entityType: "auth", metadata: { username: body.username } });
      res.status(401).json({ message: "Invalid username or password." });
      return;
    }
    setSessionCookie(res);
    await recordAuditLog({ actor: config.adminUsername, action: "auth.login.success", entityType: "auth", metadata: { username: config.adminUsername } });
    res.json({ username: config.adminUsername });
  })
);

router.post("/api/auth/logout", asyncRoute(async (req, res) => {
  await recordAuditLog({ actor: actor(req), action: "auth.logout", entityType: "auth" });
  clearSessionCookie(res);
  res.json({ ok: true });
}));

router.get("/api/auth/me", (req, res) => {
  const user = currentUser(req);
  if (!user) {
    res.status(401).json({ message: "Authentication required." });
    return;
  }
  res.json({ username: user.username });
});

export default router;
