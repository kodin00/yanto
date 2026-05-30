import fs from "node:fs";
import path from "node:path";
import { eq } from "drizzle-orm";
import { db } from "../db/index.js";
import { appSettings, cloudflareRoutes, cloudflareTunnels } from "../db/schema.js";
import type { CloudflareRouteRow, CloudflareTunnelRow, ProjectRow } from "../db/schema.js";
import { config } from "../config.js";
import { createId } from "./tokens.js";
import { runCommand } from "./commands.js";
import { getProject } from "./projects.js";
import { encrypt, decrypt, isEncrypted } from "./crypto.js";
import { normalizeString } from "./utils.js";

// --- Cloudflare Settings (app_settings key: "cloudflare.tunnel") ---

const cloudflareSettingsKey = "cloudflare.tunnel";

type CloudflareSettings = {
  accountId: string;
  zoneId: string;
  apiToken: string;
};

const emptyCloudflareSettings: CloudflareSettings = {
  accountId: "",
  zoneId: "",
  apiToken: ""
};

async function runRequiredDocker(args: string[]) {
  const result = await runCommand("docker", args);
  if (result.exitCode !== 0) {
    throw new Error(result.output.trim() || `Docker command failed: docker ${args.join(" ")}`);
  }
  return result;
}

function projectComposeNetwork(project: Pick<ProjectRow, "folderName">) {
  return `${project.folderName}_default`;
}

async function dockerNetworkExists(networkName: string) {
  const result = await runCommand("docker", ["network", "inspect", networkName, "--format", "{{.Name}}"]);
  return result.exitCode === 0;
}

async function cloudflaredNetworkNames(nodeId: string) {
  const containerName = `yanto-cloudflared-${nodeId}`;
  const result = await runCommand("docker", ["inspect", containerName, "--format", "{{json .NetworkSettings.Networks}}"]);
  if (result.exitCode !== 0 || !result.output.trim()) return new Set<string>();

  try {
    return new Set(Object.keys(JSON.parse(result.output.trim()) as Record<string, unknown>));
  } catch {
    return new Set<string>();
  }
}

function decryptTunnelRow(row: CloudflareTunnelRow): CloudflareTunnelRow {
  if (!row.tunnelToken || !isEncrypted(row.tunnelToken)) return row;
  return { ...row, tunnelToken: decrypt(row.tunnelToken) };
}

function parseCloudflareSettings(value: string | undefined): CloudflareSettings {
  if (!value) return emptyCloudflareSettings;
  try {
    const parsed = JSON.parse(value) as Partial<CloudflareSettings>;
    return {
      accountId: normalizeString(parsed.accountId),
      zoneId: normalizeString(parsed.zoneId),
      apiToken: normalizeString(parsed.apiToken)
    };
  } catch {
    return emptyCloudflareSettings;
  }
}

export async function getStoredCloudflareSettings(): Promise<CloudflareSettings> {
  const [row] = await db.select().from(appSettings).where(eq(appSettings.key, cloudflareSettingsKey)).limit(1);
  return parseCloudflareSettings(row?.value);
}

export async function publicCloudflareSettings() {
  const settings = await getStoredCloudflareSettings();
  return {
    accountId: settings.accountId,
    zoneId: settings.zoneId,
    hasApiToken: Boolean(settings.apiToken)
  };
}

export async function saveCloudflareSettings(input: Partial<CloudflareSettings>) {
  const current = await getStoredCloudflareSettings();
  const next: CloudflareSettings = {
    accountId: normalizeString(input.accountId),
    zoneId: normalizeString(input.zoneId),
    apiToken: normalizeString(input.apiToken) || current.apiToken
  };

  await db
    .insert(appSettings)
    .values({ key: cloudflareSettingsKey, value: JSON.stringify(next), updatedAt: new Date() })
    .onConflictDoUpdate({
      target: appSettings.key,
      set: { value: JSON.stringify(next), updatedAt: new Date() }
    });

  return publicCloudflareSettings();
}

// --- Cloudflare API Client ---

const CF_API_BASE = "https://api.cloudflare.com/client/v4";

type CfApiResponse<T> = {
  success: boolean;
  result: T;
  errors?: { code: number; message: string }[];
};

async function cfFetch<T>(settings: CloudflareSettings, apiPath: string, init?: RequestInit): Promise<T> {
  if (!settings.apiToken) {
    throw new Error("Cloudflare API token is not configured.");
  }

  const response = await fetch(`${CF_API_BASE}${apiPath}`, {
    ...init,
    headers: {
      Authorization: `Bearer ${settings.apiToken}`,
      "Content-Type": "application/json",
      ...init?.headers
    }
  });

  if (!response.ok) {
    const text = await response.text().catch(() => "");
    throw new Error(`Cloudflare API returned HTTP ${response.status}: ${text.slice(0, 200)}`);
  }

  let body: CfApiResponse<T>;
  try {
    body = (await response.json()) as CfApiResponse<T>;
  } catch {
    throw new Error("Cloudflare API returned a non-JSON response.");
  }

  if (!body.success) {
    const messages = body.errors?.map((e) => e.message).join("; ") ?? "Unknown Cloudflare API error.";
    throw new Error(messages);
  }

  return body.result;
}

export async function validateCloudflareSettings(input: Partial<CloudflareSettings>) {
  const settings: CloudflareSettings = {
    accountId: normalizeString(input.accountId),
    zoneId: normalizeString(input.zoneId),
    apiToken: normalizeString(input.apiToken)
  };

  if (!settings.apiToken) {
    const stored = await getStoredCloudflareSettings();
    settings.apiToken = stored.apiToken;
  }

  if (!settings.apiToken) {
    throw new Error("Provide an API token to validate.");
  }

  const account = await cfFetch<{ name: string }>(settings, `/accounts/${settings.accountId}`);
  let zoneName: string | undefined;
  if (settings.zoneId) {
    const zone = await cfFetch<{ name: string }>(settings, `/zones/${settings.zoneId}`);
    zoneName = zone.name;
  }

  return { ok: true, accountName: account.name, zoneName };
}

// --- Tunnel Management ---

export async function getTunnelForNode(nodeId: string): Promise<CloudflareTunnelRow | undefined> {
  const [row] = await db.select().from(cloudflareTunnels).where(eq(cloudflareTunnels.nodeId, nodeId)).limit(1);
  return row ? decryptTunnelRow(row) : undefined;
}

export async function listTunnels(): Promise<CloudflareTunnelRow[]> {
  const rows = await db.select().from(cloudflareTunnels);
  return rows.map(decryptTunnelRow);
}

export async function listTunnelsWithEnabledRoutes(): Promise<CloudflareTunnelRow[]> {
  const tunnels = await listTunnels();
  const routes = await db.select().from(cloudflareRoutes);
  const tunnelIdsWithEnabledRoutes = new Set(routes.filter((route) => route.enabled).map((route) => route.tunnelId));
  return tunnels.filter((tunnel) => tunnelIdsWithEnabledRoutes.has(tunnel.id));
}

export async function connectCloudflaredToProjectNetwork(tunnel: CloudflareTunnelRow, project: Pick<ProjectRow, "folderName">): Promise<boolean> {
  const networkName = projectComposeNetwork(project);
  if (!(await dockerNetworkExists(networkName))) {
    return false;
  }

  await ensureCloudflaredRunning(tunnel);
  const currentNetworks = await cloudflaredNetworkNames(tunnel.nodeId);
  if (currentNetworks.has(networkName)) {
    return true;
  }

  const containerName = `yanto-cloudflared-${tunnel.nodeId}`;
  const result = await runCommand("docker", ["network", "connect", networkName, containerName]);
  if (result.exitCode !== 0 && !result.output.toLowerCase().includes("already exists")) {
    throw new Error(result.output.trim() || `Unable to connect cloudflared to Docker network ${networkName}.`);
  }
  return true;
}

export async function ensureTunnelForNode(nodeId: string): Promise<CloudflareTunnelRow> {
  const existing = await getTunnelForNode(nodeId);
  if (existing) return existing;

  const settings = await getStoredCloudflareSettings();
  if (!settings.apiToken || !settings.accountId) {
    throw new Error("Configure Cloudflare settings before creating a tunnel.");
  }

  const tunnelName = `yanto-${nodeId}`;
  type CfTunnelResult = { id: string; name: string };
  const result = await cfFetch<CfTunnelResult>(settings, `/accounts/${settings.accountId}/cfd_tunnel`, {
    method: "POST",
    body: JSON.stringify({ name: tunnelName, config_src: "cloudflare" })
  });

  type CfTokenResult = string;
  const token = await cfFetch<CfTokenResult>(settings, `/accounts/${settings.accountId}/cfd_tunnel/${result.id}/token`);

  const id = createId("cft");
  try {
    const [row] = await db
      .insert(cloudflareTunnels)
      .values({
        id,
        nodeId,
        cfAccountId: settings.accountId,
        cfTunnelId: result.id,
        tunnelName: result.name,
        tunnelToken: encrypt(token),
        status: "active"
      })
      .returning();

    return decryptTunnelRow(row);
  } catch (error) {
    // Unique constraint violation — another concurrent call created the tunnel
    const concurrent = await getTunnelForNode(nodeId);
    if (concurrent) return concurrent;
    throw error;
  }
}

export async function putTunnelConfig(tunnel: CloudflareTunnelRow): Promise<void> {
  const settings = await getStoredCloudflareSettings();
  const routes = await db.select().from(cloudflareRoutes).where(eq(cloudflareRoutes.tunnelId, tunnel.id));

  const enabledRoutes = routes.filter((r) => r.enabled);
  const ingress = [
    ...enabledRoutes.map((r) => ({
      hostname: r.hostname,
      service: r.serviceTarget,
      originRequest: r.noTlsVerify ? { noTLSVerify: true } : {}
    })),
    { service: "http_status:404" }
  ];

  await cfFetch(settings, `/accounts/${settings.accountId}/cfd_tunnel/${tunnel.cfTunnelId}/configurations`, {
    method: "PUT",
    body: JSON.stringify({ config: { ingress } })
  });
}

type CfDnsRecord = { id: string };

export async function upsertTunnelDnsRecord(settings: CloudflareSettings, tunnelCfId: string, hostname: string): Promise<string | null> {
  if (!settings.zoneId) return null;

  type CfDnsListResult = { id: string; name: string; type: string; content: string }[];
  const records = await cfFetch<CfDnsListResult>(settings, `/zones/${settings.zoneId}/dns_records?type=CNAME&name=${encodeURIComponent(hostname)}`);

  const target = `${tunnelCfId}.cfargotunnel.com`;
  const existing = records.find((r) => r.type === "CNAME" && r.name === hostname);

  if (existing) {
    if (existing.content === target) return existing.id;
    await cfFetch(settings, `/zones/${settings.zoneId}/dns_records/${existing.id}`, {
      method: "PUT",
      body: JSON.stringify({ type: "CNAME", name: hostname, content: target, proxied: true })
    });
    return existing.id;
  }

  const created = await cfFetch<CfDnsRecord>(settings, `/zones/${settings.zoneId}/dns_records`, {
    method: "POST",
    body: JSON.stringify({ type: "CNAME", name: hostname, content: target, proxied: true })
  });

  return created.id;
}

export async function deleteTunnelDnsRecord(settings: CloudflareSettings, tunnelCfId: string, hostname: string, dnsRecordId?: string | null): Promise<void> {
  if (!settings.zoneId) return;

  type CfDnsListResult = { id: string; name: string; type: string; content: string }[];
  const records = await cfFetch<CfDnsListResult>(settings, `/zones/${settings.zoneId}/dns_records?type=CNAME&name=${encodeURIComponent(hostname)}`);
  const target = `${tunnelCfId}.cfargotunnel.com`;
  const matchingRecords = records.filter((record) => record.type === "CNAME" && record.name === hostname && (record.id === dnsRecordId || record.content === target));

  for (const record of matchingRecords) {
    await cfFetch(settings, `/zones/${settings.zoneId}/dns_records/${record.id}`, { method: "DELETE" });
  }
}

// --- Route Publishing ---

export async function listRoutesForProject(projectId: string): Promise<CloudflareRouteRow[]> {
  return db.select().from(cloudflareRoutes).where(eq(cloudflareRoutes.projectId, projectId));
}

export async function publishProjectRoute(projectId: string, hostname: string, serviceTarget: string, noTlsVerify = false, nodeId?: string): Promise<CloudflareRouteRow> {
  const project = await getProject(projectId);
  if (!project) throw new Error("Project not found.");

  const targetNodeId = nodeId ?? project.targetNodeId;
  const tunnel = await ensureTunnelForNode(targetNodeId);
  const settings = await getStoredCloudflareSettings();

  const routeId = createId("cfr");
  await db
    .insert(cloudflareRoutes)
    .values({
      id: routeId,
      tunnelId: tunnel.id,
      projectId,
      hostname,
      serviceTarget,
      noTlsVerify: serviceTarget.startsWith("https://") ? noTlsVerify : false,
      enabled: true
    })
    .returning();

  const dnsRecordId = await upsertTunnelDnsRecord(settings, tunnel.cfTunnelId, hostname);
  if (dnsRecordId) {
    await db
      .update(cloudflareRoutes)
      .set({ cfDnsRecordId: dnsRecordId, lastPublishedAt: new Date() })
      .where(eq(cloudflareRoutes.id, routeId));
  }

  await putTunnelConfig(tunnel);
  await ensureCloudflaredRunning(tunnel);
  await connectCloudflaredToProjectNetwork(tunnel, project);

  const [updated] = await db.select().from(cloudflareRoutes).where(eq(cloudflareRoutes.id, routeId)).limit(1);
  return updated;
}

export async function disableProjectRoute(routeId: string): Promise<CloudflareRouteRow> {
  const [route] = await db.select().from(cloudflareRoutes).where(eq(cloudflareRoutes.id, routeId)).limit(1);
  if (!route) throw new Error("Route not found.");

  await db.update(cloudflareRoutes).set({ enabled: false, updatedAt: new Date() }).where(eq(cloudflareRoutes.id, routeId));

  const [tunnel] = await db.select().from(cloudflareTunnels).where(eq(cloudflareTunnels.id, route.tunnelId)).limit(1);
  if (tunnel) await putTunnelConfig(decryptTunnelRow(tunnel));

  const [updated] = await db.select().from(cloudflareRoutes).where(eq(cloudflareRoutes.id, routeId)).limit(1);
  return updated;
}

export async function enableProjectRoute(routeId: string): Promise<CloudflareRouteRow> {
  const [route] = await db.select().from(cloudflareRoutes).where(eq(cloudflareRoutes.id, routeId)).limit(1);
  if (!route) throw new Error("Route not found.");

  await db.update(cloudflareRoutes).set({ enabled: true, updatedAt: new Date() }).where(eq(cloudflareRoutes.id, routeId));

  const [tunnel] = await db.select().from(cloudflareTunnels).where(eq(cloudflareTunnels.id, route.tunnelId)).limit(1);
  const project = await getProject(route.projectId);
  if (tunnel) {
    const decrypted = decryptTunnelRow(tunnel);
    await putTunnelConfig(decrypted);
    await ensureCloudflaredRunning(decrypted);
    if (project) await connectCloudflaredToProjectNetwork(decrypted, project);
  }

  const [updated] = await db.select().from(cloudflareRoutes).where(eq(cloudflareRoutes.id, routeId)).limit(1);
  return updated;
}

export async function deleteProjectRoute(routeId: string): Promise<void> {
  const [route] = await db.select().from(cloudflareRoutes).where(eq(cloudflareRoutes.id, routeId)).limit(1);
  if (!route) throw new Error("Route not found.");

  const [tunnel] = await db.select().from(cloudflareTunnels).where(eq(cloudflareTunnels.id, route.tunnelId)).limit(1);
  const settings = await getStoredCloudflareSettings();
  if (tunnel) {
    await deleteTunnelDnsRecord(settings, tunnel.cfTunnelId, route.hostname, route.cfDnsRecordId);
  }

  await db.delete(cloudflareRoutes).where(eq(cloudflareRoutes.id, routeId));

  if (tunnel) await putTunnelConfig(decryptTunnelRow(tunnel));
}

// --- Runtime (cloudflared Docker container) ---

export async function startCloudflared(tunnel: CloudflareTunnelRow): Promise<void> {
  const containerName = `yanto-cloudflared-${tunnel.nodeId}`;

  // Remove stale container if it exists (ignore error if it doesn't)
  try {
    await runCommand("docker", ["rm", "-f", containerName]);
  } catch {
    // container doesn't exist, that's fine
  }

  await fs.promises.mkdir(config.cloudflaredDir, { recursive: true, mode: 0o700 });
  const envPath = path.join(config.cloudflaredDir, `${tunnel.nodeId}.env`);
  await fs.promises.writeFile(envPath, `TUNNEL_TOKEN=${tunnel.tunnelToken}\n`, { mode: 0o600 });

  await runRequiredDocker([
    "run", "-d",
    "--name", containerName,
    "--restart", "unless-stopped",
    "--label", `yanto.cloudflared.node=${tunnel.nodeId}`,
    "--env-file", envPath,
    "cloudflare/cloudflared:latest",
    "tunnel", "--no-autoupdate", "run"
  ]);

  await db.update(cloudflareTunnels).set({ status: "active", updatedAt: new Date() }).where(eq(cloudflareTunnels.id, tunnel.id));
}

export async function stopCloudflared(nodeId: string): Promise<void> {
  const containerName = `yanto-cloudflared-${nodeId}`;
  await runRequiredDocker(["stop", containerName]);
  await runRequiredDocker(["rm", containerName]);

  const [tunnel] = await db.select().from(cloudflareTunnels).where(eq(cloudflareTunnels.nodeId, nodeId)).limit(1);
  if (tunnel) {
    await db.update(cloudflareTunnels).set({ status: "stopped", updatedAt: new Date() }).where(eq(cloudflareTunnels.id, tunnel.id));
  }
}

export async function restartCloudflared(nodeId: string): Promise<void> {
  const [tunnel] = await db.select().from(cloudflareTunnels).where(eq(cloudflareTunnels.nodeId, nodeId)).limit(1);
  if (!tunnel) throw new Error("No tunnel found for this node.");

  try {
    await stopCloudflared(nodeId);
  } catch {
    // container may not be running
  }
  await startCloudflared(decryptTunnelRow(tunnel));
}

export async function getCloudflaredStatus(nodeId: string): Promise<{ running: boolean; containerId?: string; status?: string }> {
  const containerName = `yanto-cloudflared-${nodeId}`;
  const result = await runCommand("docker", ["ps", "-a", "--filter", `name=${containerName}`, "--format", "{{json .}}"]);

  if (!result.output.trim()) {
    return { running: false };
  }

  try {
    const line = result.output.trim().split("\n")[0];
    const parsed = JSON.parse(line) as { ID: string; State: string; Status: string };
    return {
      running: parsed.State === "running",
      containerId: parsed.ID,
      status: parsed.Status
    };
  } catch {
    return { running: false };
  }
}

export async function ensureCloudflaredRunning(tunnel: CloudflareTunnelRow): Promise<void> {
  const runtime = await getCloudflaredStatus(tunnel.nodeId);
  if (!runtime.running) {
    await startCloudflared(tunnel);
  }
}

export async function ensureEnabledCloudflaredConnectors(): Promise<{ started: string[]; failed: { nodeId: string; error: string }[] }> {
  const tunnels = await listTunnelsWithEnabledRoutes();
  const routes = await db.select().from(cloudflareRoutes);
  const started: string[] = [];
  const failed: { nodeId: string; error: string }[] = [];

  for (const tunnel of tunnels) {
    try {
      const runtime = await getCloudflaredStatus(tunnel.nodeId);
      if (!runtime.running) {
        await startCloudflared(tunnel);
        started.push(tunnel.nodeId);
      }

      const enabledRoutes = routes.filter((route) => route.enabled && route.tunnelId === tunnel.id);
      for (const route of enabledRoutes) {
        const project = await getProject(route.projectId);
        if (project) await connectCloudflaredToProjectNetwork(tunnel, project);
      }
    } catch (error) {
      failed.push({ nodeId: tunnel.nodeId, error: error instanceof Error ? error.message : "Unable to start cloudflared." });
    }
  }

  return { started, failed };
}

export async function getTunnelHealth(tunnel: CloudflareTunnelRow): Promise<{ healthy: boolean; connectors?: number; status?: string }> {
  try {
    const settings = await getStoredCloudflareSettings();
    type CfHealthResult = { status: string; conns: { id: string; connected_at: string }[] };
    const result = await cfFetch<CfHealthResult>(settings, `/accounts/${tunnel.cfAccountId}/cfd_tunnel/${tunnel.cfTunnelId}/health`);
    return {
      healthy: result.status === "healthy",
      connectors: result.conns?.length ?? 0,
      status: result.status
    };
  } catch {
    return { healthy: false };
  }
}
