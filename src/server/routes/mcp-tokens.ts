import { Router } from "express";
import { requireAuth } from "../auth.js";
import { asyncRoute, actor, routeParam } from "../http-utils.js";
import { mcpTokenCreateInput } from "../route-schemas.js";
import { createMcpAccessToken, listMcpAccessTokens, revokeMcpAccessToken } from "../services/mcp-tokens.js";
import { recordAuditLog } from "../services/audit.js";

const router = Router();

router.get(
  "/api/mcp-tokens",
  requireAuth,
  asyncRoute(async (_req, res) => {
    res.json(await listMcpAccessTokens());
  })
);

router.post(
  "/api/mcp-tokens",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = mcpTokenCreateInput.parse(req.body ?? {});
    const result = await createMcpAccessToken(body);
    await recordAuditLog({
      actor: actor(req),
      action: "mcp_token.create",
      entityType: "mcp_access_token",
      entityId: result.accessToken.id,
      metadata: { name: result.accessToken.name, accessLevel: result.accessToken.accessLevel }
    });
    res.status(201).json(result);
  })
);

router.delete(
  "/api/mcp-tokens/:id",
  requireAuth,
  asyncRoute(async (req, res) => {
    const id = routeParam(req, "id");
    const token = await revokeMcpAccessToken(id);
    if (!token) {
      res.status(404).json({ message: "MCP token not found." });
      return;
    }
    await recordAuditLog({
      actor: actor(req),
      action: "mcp_token.revoke",
      entityType: "mcp_access_token",
      entityId: token.id,
      metadata: { name: token.name, accessLevel: token.accessLevel }
    });
    res.status(204).end();
  })
);

export default router;
