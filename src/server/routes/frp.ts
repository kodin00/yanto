import { Router } from "express";
import { z } from "zod";
import { requireOwner } from "../auth.js";
import { asyncRoute, actor, routeParam } from "../http-utils.js";
import { frpNodeAssignmentInput, frpServerInput, frpSettingsInput, frpTunnelInput, frpTunnelUpdateInput } from "../route-schemas.js";
import { recordAuditLog } from "../services/audit.js";
import {
  controlFrpServer,
  createFrpServer,
  createFrpTunnel,
  deleteFrpServer,
  deleteFrpTunnel,
  frpClientSetup,
  frpNodeClientSetup,
  frpOverview,
  listFrpNodeAssignments,
  listFrpServers,
  saveFrpNodeAssignment,
  saveFrpSettings,
  updateFrpServer,
  updateFrpTunnel
} from "../services/frp.js";

const router = Router();

const frpServerUpdateBody = z.object({
  name: z.string().trim().min(1).max(100).optional(),
  publicHost: z.string().trim().min(1).max(255).refine(
    (value) => !/[\s/:?#]/.test(value) || /^[0-9a-fA-F:]+$/.test(value),
    "Expected an IP address or hostname without a scheme, path, or port"
  ).optional(),
  bindPort: z.number().int().min(1).max(65_535).optional(),
  portStart: z.number().int().min(1).max(65_535).optional(),
  portEnd: z.number().int().min(1).max(65_535).optional(),
  authToken: z.string().trim().min(16).max(1000).optional()
}).refine((value) => Object.keys(value).length > 0, "At least one field is required.");

router.get("/api/frp/overview", requireOwner, asyncRoute(async (_req, res) => {
  res.json(await frpOverview());
}));

router.put("/api/frp/settings", requireOwner, asyncRoute(async (req, res) => {
  const body = frpSettingsInput.parse(req.body ?? {});
  const settings = await saveFrpSettings(body.publicHost);
  await recordAuditLog({ actor: actor(req), action: "frp.settings.save", entityType: "frp", metadata: { publicHost: settings.publicHost } });
  res.json(settings);
}));

router.get("/api/frp/client-setup", requireOwner, asyncRoute(async (req, res) => {
  const setup = await frpClientSetup();
  await recordAuditLog({ actor: actor(req), action: "frp.client_setup.reveal", entityType: "frp", metadata: { tunnelCount: setup.tunnelCount, tokenConfigured: setup.tokenConfigured } });
  res.json(setup);
}));

router.get("/api/frp/nodes/:nodeId/client-setup", requireOwner, asyncRoute(async (req, res) => {
  const setup = await frpNodeClientSetup(routeParam(req, "nodeId"));
  await recordAuditLog({ actor: actor(req), action: "frp.client_setup.reveal", entityType: "deployment_node", entityId: routeParam(req, "nodeId"), metadata: { tunnelCount: setup.tunnelCount, tokenConfigured: setup.tokenConfigured } });
  res.json(setup);
}));

router.get("/api/frp/servers", requireOwner, asyncRoute(async (_req, res) => {
  res.json(await listFrpServers());
}));

router.post("/api/frp/servers", requireOwner, asyncRoute(async (req, res) => {
  const server = await createFrpServer(frpServerInput.parse(req.body ?? {}));
  await recordAuditLog({ actor: actor(req), action: "frp.server.create", entityType: "frp_server", entityId: server.id, metadata: { nodeId: server.nodeId, publicHost: server.publicHost } });
  res.status(201).json(server);
}));

router.patch("/api/frp/servers/:id", requireOwner, asyncRoute(async (req, res) => {
  const server = await updateFrpServer(routeParam(req, "id"), frpServerUpdateBody.parse(req.body ?? {}));
  await recordAuditLog({ actor: actor(req), action: "frp.server.update", entityType: "frp_server", entityId: server.id, metadata: { nodeId: server.nodeId, publicHost: server.publicHost } });
  res.json(server);
}));

router.delete("/api/frp/servers/:id", requireOwner, asyncRoute(async (req, res) => {
  const server = await deleteFrpServer(routeParam(req, "id"));
  await recordAuditLog({ actor: actor(req), action: "frp.server.delete", entityType: "frp_server", entityId: server.id });
  res.status(204).end();
}));

router.get("/api/frp/node-assignments", requireOwner, asyncRoute(async (_req, res) => {
  res.json(await listFrpNodeAssignments());
}));

router.put("/api/frp/node-assignments/:nodeId", requireOwner, asyncRoute(async (req, res) => {
  const assignment = await saveFrpNodeAssignment(routeParam(req, "nodeId"), frpNodeAssignmentInput.parse(req.body ?? {}));
  await recordAuditLog({ actor: actor(req), action: "frp.node_assignment.save", entityType: "deployment_node", entityId: assignment.nodeId, metadata: { role: assignment.role, serverId: assignment.serverId } });
  res.json(assignment);
}));

for (const action of ["start", "stop", "restart"] as const) {
  router.post(`/api/frp/server/${action}`, requireOwner, asyncRoute(async (req, res) => {
    const server = await controlFrpServer(action);
    await recordAuditLog({ actor: actor(req), action: `frp.server.${action}`, entityType: "frp_server" });
    res.json(server);
  }));
}

router.post("/api/frp/tunnels", requireOwner, asyncRoute(async (req, res) => {
  const body = frpTunnelInput.parse(req.body ?? {});
  const tunnel = await createFrpTunnel(body);
  await recordAuditLog({ actor: actor(req), action: "frp.tunnel.create", entityType: "frp_tunnel", entityId: tunnel.id, metadata: { protocol: tunnel.protocol, remotePort: tunnel.remotePort } });
  res.status(201).json(tunnel);
}));

router.patch("/api/frp/tunnels/:id", requireOwner, asyncRoute(async (req, res) => {
  const body = frpTunnelUpdateInput.parse(req.body ?? {});
  const tunnel = await updateFrpTunnel(routeParam(req, "id"), body);
  await recordAuditLog({ actor: actor(req), action: "frp.tunnel.update", entityType: "frp_tunnel", entityId: tunnel.id, metadata: { enabled: tunnel.enabled, remotePort: tunnel.remotePort } });
  res.json(tunnel);
}));

router.delete("/api/frp/tunnels/:id", requireOwner, asyncRoute(async (req, res) => {
  const tunnel = await deleteFrpTunnel(routeParam(req, "id"));
  await recordAuditLog({ actor: actor(req), action: "frp.tunnel.delete", entityType: "frp_tunnel", entityId: tunnel.id });
  res.status(204).end();
}));

export default router;
