import { Router } from "express";
import { requireAuth } from "../auth.js";
import { asyncRoute, actor, routeParam } from "../http-utils.js";
import { frpSettingsInput, frpTunnelInput, frpTunnelUpdateInput } from "../route-schemas.js";
import { recordAuditLog } from "../services/audit.js";
import { controlFrpServer, createFrpTunnel, deleteFrpTunnel, frpOverview, saveFrpSettings, updateFrpTunnel } from "../services/frp.js";

const router = Router();

router.get("/api/frp/overview", requireAuth, asyncRoute(async (_req, res) => {
  res.json(await frpOverview());
}));

router.put("/api/frp/settings", requireAuth, asyncRoute(async (req, res) => {
  const body = frpSettingsInput.parse(req.body ?? {});
  const settings = await saveFrpSettings(body.publicHost);
  await recordAuditLog({ actor: actor(req), action: "frp.settings.save", entityType: "frp", metadata: { publicHost: settings.publicHost } });
  res.json(settings);
}));

for (const action of ["start", "stop", "restart"] as const) {
  router.post(`/api/frp/server/${action}`, requireAuth, asyncRoute(async (req, res) => {
    const server = await controlFrpServer(action);
    await recordAuditLog({ actor: actor(req), action: `frp.server.${action}`, entityType: "frp_server" });
    res.json(server);
  }));
}

router.post("/api/frp/tunnels", requireAuth, asyncRoute(async (req, res) => {
  const body = frpTunnelInput.parse(req.body ?? {});
  const tunnel = await createFrpTunnel(body);
  await recordAuditLog({ actor: actor(req), action: "frp.tunnel.create", entityType: "frp_tunnel", entityId: tunnel.id, metadata: { nodeId: tunnel.nodeId, protocol: tunnel.protocol, remotePort: tunnel.remotePort } });
  res.status(201).json(tunnel);
}));

router.patch("/api/frp/tunnels/:id", requireAuth, asyncRoute(async (req, res) => {
  const body = frpTunnelUpdateInput.parse(req.body ?? {});
  const tunnel = await updateFrpTunnel(routeParam(req, "id"), body);
  await recordAuditLog({ actor: actor(req), action: "frp.tunnel.update", entityType: "frp_tunnel", entityId: tunnel.id, metadata: { enabled: tunnel.enabled, remotePort: tunnel.remotePort } });
  res.json(tunnel);
}));

router.delete("/api/frp/tunnels/:id", requireAuth, asyncRoute(async (req, res) => {
  const tunnel = await deleteFrpTunnel(routeParam(req, "id"));
  await recordAuditLog({ actor: actor(req), action: "frp.tunnel.delete", entityType: "frp_tunnel", entityId: tunnel.id });
  res.status(204).end();
}));

export default router;
