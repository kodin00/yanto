import { Router } from "express";
import { requireAuth } from "../auth.js";
import { asyncRoute, actor, routeParam } from "../http-utils.js";
import { cloudflareAssignmentInput, cloudflareClientInput, cloudflareDnsRecordInput, cloudflareHostnameInput, cloudflareRouteInput, cloudflareTunnelInput } from "../route-schemas.js";
import { recordAuditLog } from "../services/audit.js";
import {
  createDnsRecord,
  createCloudflareClient,
  createManagedHostname,
  createManagedTunnel,
  createTunnelAssignment,
  deleteDnsRecord,
  deleteCloudflareClient,
  deleteManagedHostname,
  deleteManagedTunnel,
  deleteTunnelAssignment,
  deleteProjectRoute,
  disableProjectRoute,
  enableProjectRoute,
  getCloudflaredStatus,
  getTunnelForNode,
  getTunnelHealth,
  listDnsRecords,
  listCloudflareClients,
  listCloudflareZones,
  listManagedHostnames,
  listPublicTunnels,
  listRouteDiagnostics,
  listRoutesForProject,
  listTunnelAssignments,
  publishProjectRoute,
  publicTunnel,
  restartCloudflared,
  retryManagedHostname,
  startCloudflared,
  stopCloudflared,
  updateDnsRecord
  ,updateCloudflareClient
  ,validateCloudflareClient
} from "../services/cloudflare.js";

const router = Router();

router.get(
  "/api/cloudflare/tunnels",
  requireAuth,
  asyncRoute(async (_req, res) => {
    res.json(await listPublicTunnels());
  })
);

router.get("/api/cloudflare/clients", requireAuth, asyncRoute(async (_req, res) => { res.json(await listCloudflareClients()); }));
router.post("/api/cloudflare/clients/validate", requireAuth, asyncRoute(async (req, res) => {
  const body = cloudflareClientInput.parse(req.body);
  if (!body.apiToken) throw new Error("API token is required.");
  res.json(await validateCloudflareClient({ accountId: body.accountId, zoneId: body.zoneId, apiToken: body.apiToken }));
}));
router.post("/api/cloudflare/clients", requireAuth, asyncRoute(async (req, res) => {
  const body = cloudflareClientInput.parse(req.body);
  if (!body.apiToken) throw new Error("API token is required.");
  const client = await createCloudflareClient({ ...body, apiToken: body.apiToken });
  await recordAuditLog({ actor: actor(req), action: "cloudflare.client.create", entityType: "cloudflare_client", entityId: client.id });
  res.status(201).json(client);
}));
router.patch("/api/cloudflare/clients/:id", requireAuth, asyncRoute(async (req, res) => {
  const body = cloudflareClientInput.partial().parse(req.body);
  const client = await updateCloudflareClient(routeParam(req, "id"), body);
  await recordAuditLog({ actor: actor(req), action: "cloudflare.client.update", entityType: "cloudflare_client", entityId: client.id });
  res.json(client);
}));
router.delete("/api/cloudflare/clients/:id", requireAuth, asyncRoute(async (req, res) => {
  const id = routeParam(req, "id"); await deleteCloudflareClient(id);
  await recordAuditLog({ actor: actor(req), action: "cloudflare.client.delete", entityType: "cloudflare_client", entityId: id });
  res.status(204).end();
}));
router.get("/api/cloudflare/clients/:id/zones", requireAuth, asyncRoute(async (req, res) => { res.json(await listCloudflareZones(routeParam(req, "id"))); }));

router.post("/api/cloudflare/tunnels", requireAuth, asyncRoute(async (req, res) => {
  const tunnel = await createManagedTunnel(cloudflareTunnelInput.parse(req.body));
  await recordAuditLog({ actor: actor(req), action: "cloudflare.tunnel.create", entityType: "cloudflare_tunnel", entityId: tunnel.id });
  res.status(201).json(publicTunnel(tunnel));
}));
router.delete("/api/cloudflare/tunnels/:id", requireAuth, asyncRoute(async (req, res) => {
  const id = routeParam(req, "id"); const force = req.query.force === "true"; await deleteManagedTunnel(id, force);
  await recordAuditLog({ actor: actor(req), action: force ? "cloudflare.tunnel.force_delete" : "cloudflare.tunnel.delete", entityType: "cloudflare_tunnel", entityId: id });
  res.status(204).end();
}));
router.get("/api/cloudflare/assignments", requireAuth, asyncRoute(async (req, res) => { res.json(await listTunnelAssignments(typeof req.query.tunnelId === "string" ? req.query.tunnelId : undefined)); }));
router.post("/api/cloudflare/assignments", requireAuth, asyncRoute(async (req, res) => {
  const assignment = await createTunnelAssignment(cloudflareAssignmentInput.parse(req.body));
  await recordAuditLog({ actor: actor(req), action: "cloudflare.assignment.create", entityType: "cloudflare_assignment", entityId: assignment.id });
  res.status(201).json(assignment);
}));
router.delete("/api/cloudflare/assignments/:id", requireAuth, asyncRoute(async (req, res) => {
  const id = routeParam(req, "id"); await deleteTunnelAssignment(id);
  await recordAuditLog({ actor: actor(req), action: "cloudflare.assignment.delete", entityType: "cloudflare_assignment", entityId: id });
  res.status(204).end();
}));
router.get("/api/cloudflare/hostnames", requireAuth, asyncRoute(async (_req, res) => { res.json(await listManagedHostnames()); }));
router.post("/api/cloudflare/hostnames", requireAuth, asyncRoute(async (req, res) => {
  const hostname = await createManagedHostname(cloudflareHostnameInput.parse(req.body));
  await recordAuditLog({ actor: actor(req), action: "cloudflare.hostname.create", entityType: "cloudflare_route", entityId: hostname.id, projectId: hostname.projectId });
  res.status(201).json(hostname);
}));
router.delete("/api/cloudflare/hostnames/:id", requireAuth, asyncRoute(async (req, res) => {
  const id = routeParam(req, "id"); const result = await deleteManagedHostname(id);
  await recordAuditLog({ actor: actor(req), action: "cloudflare.hostname.delete", entityType: "cloudflare_route", entityId: id, metadata: result.warnings.length ? { warnings: result.warnings } : undefined });
  if (result.warnings.length) {
    res.json({ ok: true, warnings: result.warnings });
    return;
  }
  res.status(204).end();
}));
router.post("/api/cloudflare/hostnames/:id/retry", requireAuth, asyncRoute(async (req, res) => {
  const hostname = await retryManagedHostname(routeParam(req, "id"));
  await recordAuditLog({ actor: actor(req), action: "cloudflare.hostname.retry", entityType: "cloudflare_route", entityId: hostname.id, projectId: hostname.projectId });
  res.json(hostname);
}));

router.get(
  "/api/cloudflare/dns-records",
  requireAuth,
  asyncRoute(async (_req, res) => {
    res.json(await listDnsRecords());
  })
);

router.post(
  "/api/cloudflare/dns-records",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = cloudflareDnsRecordInput.parse(req.body);
    const record = await createDnsRecord(body);
    await recordAuditLog({ actor: actor(req), action: "cloudflare.dns.create", entityType: "cloudflare_dns_record", entityId: record.id, metadata: { type: record.type, name: record.name } });
    res.status(201).json(record);
  })
);

router.patch(
  "/api/cloudflare/dns-records/:id",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = cloudflareDnsRecordInput.parse(req.body);
    const record = await updateDnsRecord(routeParam(req, "id"), body);
    await recordAuditLog({ actor: actor(req), action: "cloudflare.dns.update", entityType: "cloudflare_dns_record", entityId: record.id, metadata: { type: record.type, name: record.name } });
    res.json(record);
  })
);

router.delete(
  "/api/cloudflare/dns-records/:id",
  requireAuth,
  asyncRoute(async (req, res) => {
    const id = routeParam(req, "id");
    await deleteDnsRecord(id);
    await recordAuditLog({ actor: actor(req), action: "cloudflare.dns.delete", entityType: "cloudflare_dns_record", entityId: id });
    res.status(204).end();
  })
);

router.get(
  "/api/cloudflare/routes/diagnostics",
  requireAuth,
  asyncRoute(async (_req, res) => {
    res.json(await listRouteDiagnostics());
  })
);

router.get(
  "/api/cloudflare/tunnels/node/:nodeId",
  requireAuth,
  asyncRoute(async (req, res) => {
    const nodeId = routeParam(req, "nodeId");
    const tunnel = await getTunnelForNode(nodeId);
    if (!tunnel) {
      res.status(404).json({ message: "No tunnel found. Create one in Hostnames → Tunnels & Networks." });
      return;
    }
    const [runtime, health] = await Promise.all([getCloudflaredStatus(nodeId), getTunnelHealth(tunnel)]);
    res.json({ tunnel: publicTunnel(tunnel), runtime, health });
  })
);

router.post(
  "/api/cloudflare/tunnels/node/:nodeId/start",
  requireAuth,
  asyncRoute(async (req, res) => {
    const nodeId = routeParam(req, "nodeId");
    const tunnel = await getTunnelForNode(nodeId);
    if (!tunnel) {
      res.status(404).json({ message: "No tunnel found. Create a tunnel in Hostnames → Tunnels & Networks first." });
      return;
    }
    await startCloudflared(tunnel);
    await recordAuditLog({ actor: actor(req), action: "cloudflare.tunnel.start", entityType: "cloudflare_tunnel", entityId: tunnel.id, metadata: { nodeId } });
    res.json({ ok: true });
  })
);

router.post(
  "/api/cloudflare/tunnels/node/:nodeId/stop",
  requireAuth,
  asyncRoute(async (req, res) => {
    const nodeId = routeParam(req, "nodeId");
    await stopCloudflared(nodeId);
    await recordAuditLog({ actor: actor(req), action: "cloudflare.tunnel.stop", entityType: "cloudflare_tunnel", metadata: { nodeId } });
    res.json({ ok: true });
  })
);

router.post(
  "/api/cloudflare/tunnels/node/:nodeId/restart",
  requireAuth,
  asyncRoute(async (req, res) => {
    const nodeId = routeParam(req, "nodeId");
    await restartCloudflared(nodeId);
    await recordAuditLog({ actor: actor(req), action: "cloudflare.tunnel.restart", entityType: "cloudflare_tunnel", metadata: { nodeId } });
    res.json({ ok: true });
  })
);

router.get(
  "/api/projects/:id/cf-routes",
  requireAuth,
  asyncRoute(async (req, res) => {
    res.json(await listRoutesForProject(routeParam(req, "id")));
  })
);

router.post(
  "/api/projects/:id/cf-routes",
  requireAuth,
  asyncRoute(async (req, res) => {
    const body = cloudflareRouteInput.parse(req.body);
    const projectId = routeParam(req, "id");
    const route = await publishProjectRoute(projectId, body.hostname, body.serviceTarget, body.noTlsVerify, body.nodeId);
    await recordAuditLog({
      actor: actor(req),
      action: "cloudflare.route.publish",
      entityType: "cloudflare_route",
      entityId: route.id,
      projectId,
      metadata: { hostname: route.hostname, serviceTarget: route.serviceTarget, noTlsVerify: route.noTlsVerify }
    });
    res.status(201).json(route);
  })
);

router.patch(
  "/api/cloudflare/routes/:id/enable",
  requireAuth,
  asyncRoute(async (req, res) => {
    const route = await enableProjectRoute(routeParam(req, "id"));
    await recordAuditLog({ actor: actor(req), action: "cloudflare.route.enable", entityType: "cloudflare_route", entityId: route.id, projectId: route.projectId });
    res.json(route);
  })
);

router.patch(
  "/api/cloudflare/routes/:id/disable",
  requireAuth,
  asyncRoute(async (req, res) => {
    const route = await disableProjectRoute(routeParam(req, "id"));
    await recordAuditLog({ actor: actor(req), action: "cloudflare.route.disable", entityType: "cloudflare_route", entityId: route.id, projectId: route.projectId });
    res.json(route);
  })
);

router.delete(
  "/api/cloudflare/routes/:id",
  requireAuth,
  asyncRoute(async (req, res) => {
    const id = routeParam(req, "id");
    await deleteProjectRoute(id);
    await recordAuditLog({ actor: actor(req), action: "cloudflare.route.delete", entityType: "cloudflare_route", entityId: id });
    res.status(204).end();
  })
);

export default router;
