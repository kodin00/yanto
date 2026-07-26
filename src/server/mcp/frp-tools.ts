import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import { frpSettingsInput, frpTunnelInput, frpTunnelUpdateInput } from "../route-schemas.js";
import { recordAuditLog } from "../services/audit.js";
import { controlFrpServer, createFrpTunnel, deleteFrpTunnel, frpOverview, saveFrpSettings, updateFrpTunnel } from "../services/frp.js";
import type { YantoMcpContext } from "./context.js";
import { destructive, readOnly, registerTool, summarize, write } from "./tool-registration.js";
import { requireConfirm, toolResult } from "./tool-utils.js";

const confirmInput = z.object({ confirm: z.boolean().optional() });
const deleteInput = z.object({ id: z.string().min(1), confirm: z.boolean().optional() });
const tunnelUpdateInput = frpTunnelUpdateInput.extend({ id: z.string().min(1) });

export function registerFrpTools(server: McpServer, ctx: YantoMcpContext) {
  registerTool(server, ctx, "read", "yanto_frp_overview", "Read FRP overview.", z.object({}), readOnly, async () => summarize("FRP overview", await frpOverview(), "overview"));
  registerTool(server, ctx, "write", "yanto_frp_settings_update", "Update FRP settings.", frpSettingsInput, write, async (input) => {
    const settings = await saveFrpSettings(input.publicHost);
    await recordAuditLog({ actor: ctx.actor, action: "frp.settings.save", entityType: "frp", metadata: { publicHost: settings.publicHost } });
    return summarize("FRP settings saved", settings, "settings");
  });
  registerTool(server, ctx, "write", "yanto_frp_server_start", "Start FRP server.", z.object({}), write, async () => {
    const serverState = await controlFrpServer("start");
    await recordAuditLog({ actor: ctx.actor, action: "frp.server.start", entityType: "frp_server" });
    return summarize("FRP server started", serverState, "server");
  });
  registerTool(server, ctx, "admin", "yanto_frp_server_stop", "Stop FRP server. Requires confirm: true.", confirmInput, destructive, async (input) => {
    requireConfirm(input.confirm, "FRP server stop");
    const serverState = await controlFrpServer("stop");
    await recordAuditLog({ actor: ctx.actor, action: "frp.server.stop", entityType: "frp_server" });
    return summarize("FRP server stopped", serverState, "server");
  });
  registerTool(server, ctx, "admin", "yanto_frp_server_restart", "Restart FRP server. Requires confirm: true.", confirmInput, destructive, async (input) => {
    requireConfirm(input.confirm, "FRP server restart");
    const serverState = await controlFrpServer("restart");
    await recordAuditLog({ actor: ctx.actor, action: "frp.server.restart", entityType: "frp_server" });
    return summarize("FRP server restarted", serverState, "server");
  });
  registerTool(server, ctx, "write", "yanto_frp_tunnel_create", "Create FRP tunnel.", frpTunnelInput, write, async (input) => {
    const tunnel = await createFrpTunnel(input);
    await recordAuditLog({ actor: ctx.actor, action: "frp.tunnel.create", entityType: "frp_tunnel", entityId: tunnel.id, metadata: { protocol: tunnel.protocol, remotePort: tunnel.remotePort } });
    return summarize("FRP tunnel created", tunnel, "tunnel");
  });
  registerTool(server, ctx, "write", "yanto_frp_tunnel_update", "Update FRP tunnel.", tunnelUpdateInput, write, async (input) => {
    const { id, ...patch } = input;
    const tunnel = await updateFrpTunnel(id, patch);
    await recordAuditLog({ actor: ctx.actor, action: "frp.tunnel.update", entityType: "frp_tunnel", entityId: tunnel.id, metadata: { enabled: tunnel.enabled, remotePort: tunnel.remotePort } });
    return summarize("FRP tunnel updated", tunnel, "tunnel");
  });
  registerTool(server, ctx, "admin", "yanto_frp_tunnel_delete", "Delete FRP tunnel. Requires confirm: true.", deleteInput, destructive, async (input) => {
    requireConfirm(input.confirm, "FRP tunnel deletion");
    const tunnel = await deleteFrpTunnel(input.id);
    await recordAuditLog({ actor: ctx.actor, action: "frp.tunnel.delete", entityType: "frp_tunnel", entityId: tunnel.id });
    return toolResult("FRP tunnel deleted.", { ok: true, id: input.id });
  });
}
