import { Router } from "express";
import { requireAuth } from "../auth.js";
import { asyncRoute, actor, routeParam } from "../http-utils.js";
import { cloudflareDnsRecordInput, cloudflareRouteInput } from "../route-schemas.js";
import { recordAuditLog } from "../services/audit.js";
import {
  createDnsRecord,
  deleteDnsRecord,
  deleteProjectRoute,
  disableProjectRoute,
  enableProjectRoute,
  getCloudflaredStatus,
  getTunnelForNode,
  getTunnelHealth,
  listDnsRecords,
  listRoutesForProject,
  listTunnels,
  publishProjectRoute,
  restartCloudflared,
  startCloudflared,
  stopCloudflared,
  updateDnsRecord
} from "../services/cloudflare.js";

const router = Router();

router.get(
  "/api/cloudflare/tunnels",
  requireAuth,
  asyncRoute(async (_req, res) => {
    res.json(await listTunnels());
  })
);

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
  "/api/cloudflare/tunnels/node/:nodeId",
  requireAuth,
  asyncRoute(async (req, res) => {
    const nodeId = routeParam(req, "nodeId");
    const tunnel = await getTunnelForNode(nodeId);
    if (!tunnel) {
      res.status(404).json({ message: "No tunnel found for this node." });
      return;
    }
    const [runtime, health] = await Promise.all([getCloudflaredStatus(nodeId), getTunnelHealth(tunnel)]);
    res.json({ tunnel, runtime, health });
  })
);

router.post(
  "/api/cloudflare/tunnels/node/:nodeId/start",
  requireAuth,
  asyncRoute(async (req, res) => {
    const nodeId = routeParam(req, "nodeId");
    const tunnel = await getTunnelForNode(nodeId);
    if (!tunnel) {
      res.status(404).json({ message: "No tunnel found for this node. Create a route first." });
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
