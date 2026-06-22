import fs from "node:fs";
import path from "node:path";
import { eq } from "drizzle-orm";
import { db } from "../db/index.js";
import { appSettings, cloudflareClients, cloudflareRoutes, cloudflareTunnelAssignments, cloudflareTunnels, projects } from "../db/schema.js";
import type { CloudflareClientRow, CloudflareRouteRow, CloudflareTunnelAssignmentRow, CloudflareTunnelRow, ProjectRow } from "../db/schema.js";
import { config } from "../config.js";
import { createId } from "./tokens.js";
import { runCommand } from "./commands.js";
import { getProject } from "./projects.js";
import { encrypt, decrypt, isEncrypted } from "./crypto.js";
import { normalizeString } from "./utils.js";
import { classifyRouteDiagnostic } from "./cloudflare-diagnostics.js";
import type { CloudflareClient, CloudflareDnsRecord, CloudflareDnsRecordType, CloudflareRouteDiagnostic, CloudflareRouteDiagnosticDnsRecord, CloudflareRouteReachabilityStatus, CloudflareZone } from "../../shared/types.js";

// --- Cloudflare Settings (app_settings key: "cloudflare.tunnel") ---

const cloudflareSettingsKey = "cloudflare.tunnel";

type CloudflareSettings = {
  accountId: string;
  zoneId: string;
  apiToken: string;
};

type CloudflareAuth = Pick<CloudflareSettings, "apiToken">;

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

function cloudflaredContainerName(tunnelId: string) {
  return `yanto-cloudflared-${tunnelId}`;
}

async function cloudflaredNetworkNames(tunnelId: string) {
  const containerName = cloudflaredContainerName(tunnelId);
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
      apiToken: parsed.apiToken && isEncrypted(parsed.apiToken) ? decrypt(parsed.apiToken) : normalizeString(parsed.apiToken)
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
    .values({ key: cloudflareSettingsKey, value: JSON.stringify({ ...next, apiToken: next.apiToken ? encrypt(next.apiToken) : "" }), updatedAt: new Date() })
    .onConflictDoUpdate({
      target: appSettings.key,
      set: { value: JSON.stringify({ ...next, apiToken: next.apiToken ? encrypt(next.apiToken) : "" }), updatedAt: new Date() }
    });

  if (next.accountId && next.apiToken) {
    await db.insert(cloudflareClients).values({
      id: createId("cfc"), name: "Default Cloudflare", accountId: next.accountId, apiToken: encrypt(next.apiToken)
    }).onConflictDoUpdate({
      target: cloudflareClients.accountId,
      set: { apiToken: encrypt(next.apiToken), updatedAt: new Date() }
    });
  }

  return publicCloudflareSettings();
}

// --- Cloudflare API Client ---

const CF_API_BASE = "https://api.cloudflare.com/client/v4";

type CfApiResponse<T> = {
  success: boolean;
  result: T;
  errors?: { code: number; message: string }[];
};

async function cfFetch<T>(settings: CloudflareAuth, apiPath: string, init?: RequestInit): Promise<T> {
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

function decryptClient(row: CloudflareClientRow): CloudflareClientRow {
  return { ...row, apiToken: isEncrypted(row.apiToken) ? decrypt(row.apiToken) : row.apiToken };
}

function publicClient(row: CloudflareClientRow): CloudflareClient {
  return {
    id: row.id,
    name: row.name,
    accountId: row.accountId,
    hasApiToken: Boolean(row.apiToken),
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString()
  };
}

async function requireClient(clientId: string) {
  const [row] = await db.select().from(cloudflareClients).where(eq(cloudflareClients.id, clientId)).limit(1);
  if (!row) throw new Error("Cloudflare client not found.");
  return decryptClient(row);
}

export async function listCloudflareClients() {
  return (await db.select().from(cloudflareClients)).map(publicClient);
}

export async function validateCloudflareClient(input: { accountId: string; apiToken: string }) {
  const accountId = normalizeString(input.accountId);
  const apiToken = normalizeString(input.apiToken);
  if (!accountId || !apiToken) throw new Error("Account ID and API token are required.");
  await cfFetch<unknown[]>({ apiToken }, `/accounts/${accountId}/cfd_tunnel?per_page=1`);
  const zones = await cfFetch<{ id: string; name: string; status: string }[]>({ apiToken }, `/zones?account.id=${encodeURIComponent(accountId)}&per_page=50`);
  return { ok: true, accountName: accountId, zones };
}

export async function createCloudflareClient(input: { name: string; accountId: string; apiToken: string }) {
  await validateCloudflareClient(input);
  const [row] = await db.insert(cloudflareClients).values({
    id: createId("cfc"),
    name: normalizeString(input.name),
    accountId: normalizeString(input.accountId),
    apiToken: encrypt(normalizeString(input.apiToken))
  }).returning();
  return publicClient(row);
}

export async function updateCloudflareClient(clientId: string, input: { name?: string; accountId?: string; apiToken?: string }) {
  const current = await requireClient(clientId);
  const next = {
    name: normalizeString(input.name) || current.name,
    accountId: normalizeString(input.accountId) || current.accountId,
    apiToken: normalizeString(input.apiToken) || current.apiToken
  };
  await validateCloudflareClient(next);
  const [row] = await db.update(cloudflareClients).set({ name: next.name, accountId: next.accountId, apiToken: encrypt(next.apiToken), updatedAt: new Date() }).where(eq(cloudflareClients.id, clientId)).returning();
  return publicClient(row);
}

export async function deleteCloudflareClient(clientId: string) {
  const owned = await db.select({ id: cloudflareTunnels.id }).from(cloudflareTunnels).where(eq(cloudflareTunnels.clientId, clientId)).limit(1);
  if (owned.length) throw new Error("Delete this client's tunnels first.");
  await db.delete(cloudflareClients).where(eq(cloudflareClients.id, clientId));
}

export async function listCloudflareZones(clientId: string): Promise<CloudflareZone[]> {
  const client = await requireClient(clientId);
  return cfFetch<CloudflareZone[]>({ apiToken: client.apiToken }, `/zones?account.id=${encodeURIComponent(client.accountId)}&per_page=50`);
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

export function publicTunnel(tunnel: CloudflareTunnelRow) {
  return {
    id: tunnel.id,
    clientId: tunnel.clientId,
    nodeId: tunnel.nodeId,
    cfAccountId: tunnel.cfAccountId,
    cfTunnelId: tunnel.cfTunnelId,
    tunnelName: tunnel.tunnelName,
    dockerNetworkName: tunnel.dockerNetworkName,
    status: tunnel.status,
    lastHealthCheckAt: tunnel.lastHealthCheckAt,
    createdAt: tunnel.createdAt,
    updatedAt: tunnel.updatedAt
  };
}

export async function listPublicTunnels() {
  return (await db.select().from(cloudflareTunnels)).map(publicTunnel);
}

export async function listManagedHostnames() {
  return db.select().from(cloudflareRoutes);
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
  const currentNetworks = await cloudflaredNetworkNames(tunnel.id);
  if (currentNetworks.has(networkName)) {
    return true;
  }

  const containerName = cloudflaredContainerName(tunnel.id);
  const result = await runCommand("docker", ["network", "connect", networkName, containerName]);
  if (result.exitCode !== 0 && !result.output.toLowerCase().includes("already exists")) {
    throw new Error(result.output.trim() || `Unable to connect cloudflared to Docker network ${networkName}.`);
  }
  return true;
}

export async function createManagedTunnel(input: { clientId: string; name: string }) {
  const client = await requireClient(input.clientId);
  const tunnelName = normalizeString(input.name);
  if (!tunnelName) throw new Error("Tunnel name is required.");
  const result = await cfFetch<{ id: string; name: string }>({ apiToken: client.apiToken }, `/accounts/${client.accountId}/cfd_tunnel`, {
    method: "POST",
    body: JSON.stringify({ name: tunnelName, config_src: "cloudflare" })
  });
  const token = await cfFetch<string>({ apiToken: client.apiToken }, `/accounts/${client.accountId}/cfd_tunnel/${result.id}/token`);
  const id = createId("cft");
  const dockerNetworkName = `yanto-cf-${id.toLowerCase().replace(/[^a-z0-9_.-]/g, "-")}`;
  try {
    await runRequiredDocker(["network", "create", "--label", `yanto.cloudflared.tunnel=${id}`, dockerNetworkName]);
    const [row] = await db.insert(cloudflareTunnels).values({
      id,
      clientId: client.id,
      nodeId: config.localNodeId,
      cfAccountId: client.accountId,
      cfTunnelId: result.id,
      tunnelName: result.name,
      tunnelToken: encrypt(token),
      dockerNetworkName,
      status: "active"
    }).returning();
    const decrypted = decryptTunnelRow(row);
    await putTunnelConfig(decrypted);
    await startCloudflared(decrypted);
    return row;
  } catch (error) {
    await runCommand("docker", ["network", "rm", dockerNetworkName]);
    await cfFetch({ apiToken: client.apiToken }, `/accounts/${client.accountId}/cfd_tunnel/${result.id}`, { method: "DELETE" }).catch(() => undefined);
    throw error;
  }
}

export async function deleteManagedTunnel(tunnelId: string, force = false) {
  const [tunnel] = await db.select().from(cloudflareTunnels).where(eq(cloudflareTunnels.id, tunnelId)).limit(1);
  if (!tunnel) throw new Error("Tunnel not found.");
  const [routes, assignments] = await Promise.all([
    db.select().from(cloudflareRoutes).where(eq(cloudflareRoutes.tunnelId, tunnelId)),
    db.select().from(cloudflareTunnelAssignments).where(eq(cloudflareTunnelAssignments.tunnelId, tunnelId))
  ]);
  if (!force && (routes.length || assignments.length)) throw new Error("Remove hostnames and network assignments first, or force delete the tunnel.");
  const client = await requireClient(tunnel.clientId);
  if (force) {
    for (const route of routes) await deleteDnsRecordForRoute(client, route).catch(() => undefined);
  }
  await runCommand("docker", ["rm", "-f", cloudflaredContainerName(tunnel.id)]);
  if (force) await disconnectAllFromNetwork(tunnel.dockerNetworkName);
  await runCommand("docker", ["network", "rm", tunnel.dockerNetworkName]);
  await cfFetch({ apiToken: client.apiToken }, `/accounts/${client.accountId}/cfd_tunnel/${tunnel.cfTunnelId}`, { method: "DELETE" });
  await db.delete(cloudflareTunnels).where(eq(cloudflareTunnels.id, tunnel.id));
}

export async function ensureTunnelForNode(nodeId: string): Promise<CloudflareTunnelRow> {
  const existing = await getTunnelForNode(nodeId);
  if (existing) return existing;
  const [client] = await db.select().from(cloudflareClients).limit(1);
  if (!client) throw new Error("Add a Cloudflare client before creating a tunnel.");
  return decryptTunnelRow(await createManagedTunnel({ clientId: client.id, name: `yanto-${nodeId}` }));
}

export async function putTunnelConfig(tunnel: CloudflareTunnelRow): Promise<void> {
  const client = await requireClient(tunnel.clientId);
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

  await cfFetch({ apiToken: client.apiToken }, `/accounts/${client.accountId}/cfd_tunnel/${tunnel.cfTunnelId}/configurations`, {
    method: "PUT",
    body: JSON.stringify({ config: { ingress } })
  });
}

function assignmentHost(assignment: CloudflareTunnelAssignmentRow) {
  if (assignment.targetType === "compose_service") {
    return `${assignment.composeProject}-${assignment.composeService}`.toLowerCase().replace(/[^a-z0-9_.-]/g, "-");
  }
  return assignment.containerName ?? "";
}

async function assignmentContainerIds(assignment: CloudflareTunnelAssignmentRow) {
  const args = ["ps", "-a", "--format", "{{.ID}}"];
  if (assignment.targetType === "compose_service") {
    args.splice(2, 0, "--filter", `label=com.docker.compose.project=${assignment.composeProject}`, "--filter", `label=com.docker.compose.service=${assignment.composeService}`);
  } else {
    args.splice(2, 0, "--filter", `name=^/${assignment.containerName}$`);
  }
  const result = await runCommand("docker", args);
  return result.output.trim().split("\n").filter(Boolean);
}

async function connectAssignment(tunnel: CloudflareTunnelRow, assignment: CloudflareTunnelAssignmentRow) {
  const ids = await assignmentContainerIds(assignment);
  for (const id of ids) {
    const args = ["network", "connect"];
    if (assignment.targetType === "compose_service") args.push("--alias", assignmentHost(assignment));
    args.push(tunnel.dockerNetworkName, id);
    const result = await runCommand("docker", args);
    if (result.exitCode !== 0 && !result.output.toLowerCase().includes("already exists")) throw new Error(result.output.trim() || "Unable to attach container to tunnel network.");
  }
  return ids.length;
}

async function disconnectAssignment(tunnel: CloudflareTunnelRow, assignment: CloudflareTunnelAssignmentRow) {
  for (const id of await assignmentContainerIds(assignment)) await runCommand("docker", ["network", "disconnect", tunnel.dockerNetworkName, id]);
}

async function disconnectAllFromNetwork(networkName: string) {
  const result = await runCommand("docker", ["network", "inspect", networkName, "--format", "{{range $id, $_ := .Containers}}{{$id}} {{end}}"]);
  for (const id of result.output.trim().split(/\s+/).filter(Boolean)) await runCommand("docker", ["network", "disconnect", "-f", networkName, id]);
}

export async function listTunnelAssignments(tunnelId?: string) {
  return tunnelId
    ? db.select().from(cloudflareTunnelAssignments).where(eq(cloudflareTunnelAssignments.tunnelId, tunnelId))
    : db.select().from(cloudflareTunnelAssignments);
}

export async function createTunnelAssignment(input: { tunnelId: string; projectId?: string; composeProject?: string; composeService?: string; containerName?: string }) {
  const [tunnel] = await db.select().from(cloudflareTunnels).where(eq(cloudflareTunnels.id, input.tunnelId)).limit(1);
  if (!tunnel) throw new Error("Tunnel not found.");
  const composeProject = normalizeString(input.composeProject);
  const composeService = normalizeString(input.composeService);
  const containerName = normalizeString(input.containerName);
  const targetType = composeProject && composeService ? "compose_service" : "container";
  if (targetType === "container" && !containerName) throw new Error("Choose a Compose service or container.");
  const [assignment] = await db.insert(cloudflareTunnelAssignments).values({
    id: createId("cfa"), tunnelId: tunnel.id, targetType, projectId: input.projectId || null,
    composeProject: composeProject || null, composeService: composeService || null, containerName: containerName || null
  }).returning();
  try {
    await connectAssignment(tunnel, assignment);
    return assignment;
  } catch (error) {
    await db.delete(cloudflareTunnelAssignments).where(eq(cloudflareTunnelAssignments.id, assignment.id));
    throw error;
  }
}

export async function deleteTunnelAssignment(assignmentId: string) {
  const [assignment] = await db.select().from(cloudflareTunnelAssignments).where(eq(cloudflareTunnelAssignments.id, assignmentId)).limit(1);
  if (!assignment) throw new Error("Network assignment not found.");
  const route = await db.select({ id: cloudflareRoutes.id }).from(cloudflareRoutes).where(eq(cloudflareRoutes.assignmentId, assignmentId)).limit(1);
  if (route.length) throw new Error("Remove hostnames using this assignment first.");
  const [tunnel] = await db.select().from(cloudflareTunnels).where(eq(cloudflareTunnels.id, assignment.tunnelId)).limit(1);
  if (tunnel) await disconnectAssignment(tunnel, assignment);
  await db.delete(cloudflareTunnelAssignments).where(eq(cloudflareTunnelAssignments.id, assignmentId));
}

export async function reconcileTunnelAssignments() {
  const tunnels = await listTunnels();
  const assignments = await db.select().from(cloudflareTunnelAssignments);
  let connected = 0;
  for (const tunnel of tunnels) {
    if (!(await dockerNetworkExists(tunnel.dockerNetworkName))) await runRequiredDocker(["network", "create", "--label", `yanto.cloudflared.tunnel=${tunnel.id}`, tunnel.dockerNetworkName]);
    await ensureCloudflaredRunning(tunnel);
    for (const assignment of assignments.filter((item) => item.tunnelId === tunnel.id)) connected += await connectAssignment(tunnel, assignment);
  }
  return { connected };
}

async function upsertDnsRecordForRoute(client: CloudflareClientRow, tunnel: CloudflareTunnelRow, route: CloudflareRouteRow) {
  const records = await cfFetch<{ id: string; type: string; name: string; content: string }[]>({ apiToken: client.apiToken }, `/zones/${route.zoneId}/dns_records?type=CNAME&name=${encodeURIComponent(route.hostname)}`);
  const target = `${tunnel.cfTunnelId}.cfargotunnel.com`;
  const existing = records.find((record) => record.type === "CNAME" && record.name === route.hostname);
  if (existing) {
    if (existing.content !== target) await cfFetch({ apiToken: client.apiToken }, `/zones/${route.zoneId}/dns_records/${existing.id}`, { method: "PUT", body: JSON.stringify({ type: "CNAME", name: route.hostname, content: target, proxied: true }) });
    return existing.id;
  }
  const created = await cfFetch<{ id: string }>({ apiToken: client.apiToken }, `/zones/${route.zoneId}/dns_records`, { method: "POST", body: JSON.stringify({ type: "CNAME", name: route.hostname, content: target, proxied: true }) });
  return created.id;
}

async function deleteDnsRecordForRoute(client: CloudflareClientRow, route: CloudflareRouteRow) {
  if (route.cfDnsRecordId) await cfFetch({ apiToken: client.apiToken }, `/zones/${route.zoneId}/dns_records/${route.cfDnsRecordId}`, { method: "DELETE" });
}

export async function createManagedHostname(input: { tunnelId: string; assignmentId: string; zoneId: string; hostname: string; protocol: "http" | "https"; port: number; noTlsVerify?: boolean }) {
  const [[tunnel], [assignment]] = await Promise.all([
    db.select().from(cloudflareTunnels).where(eq(cloudflareTunnels.id, input.tunnelId)).limit(1),
    db.select().from(cloudflareTunnelAssignments).where(eq(cloudflareTunnelAssignments.id, input.assignmentId)).limit(1)
  ]);
  if (!tunnel || !assignment || assignment.tunnelId !== tunnel.id) throw new Error("The hostname target must be assigned to this tunnel.");
  if (!(await assignmentContainerIds(assignment)).length) throw new Error("The assigned target container is not available.");
  const hostname = normalizeString(input.hostname).toLowerCase();
  const serviceTarget = `${input.protocol}://${assignmentHost(assignment)}:${input.port}`;
  const [route] = await db.insert(cloudflareRoutes).values({
    id: createId("cfr"), tunnelId: tunnel.id, projectId: assignment.projectId, assignmentId: assignment.id,
    zoneId: input.zoneId, hostname, serviceTarget, protocol: input.protocol, port: input.port,
    noTlsVerify: input.protocol === "https" && Boolean(input.noTlsVerify), enabled: true
  }).returning();
  const client = await requireClient(tunnel.clientId);
  try {
    await putTunnelConfig(tunnel);
    const cfDnsRecordId = await upsertDnsRecordForRoute(client, tunnel, route);
    const [updated] = await db.update(cloudflareRoutes).set({ cfDnsRecordId, syncStatus: "active", lastError: null, lastPublishedAt: new Date(), updatedAt: new Date() }).where(eq(cloudflareRoutes.id, route.id)).returning();
    return updated;
  } catch (error) {
    const message = error instanceof Error ? error.message : "Cloudflare synchronization failed.";
    await db.update(cloudflareRoutes).set({ enabled: false, syncStatus: "error", lastError: message, updatedAt: new Date() }).where(eq(cloudflareRoutes.id, route.id));
    await putTunnelConfig(tunnel).catch(() => undefined);
    throw error;
  }
}

export async function retryManagedHostname(routeId: string) {
  const [route] = await db.select().from(cloudflareRoutes).where(eq(cloudflareRoutes.id, routeId)).limit(1);
  if (!route) throw new Error("Hostname not found.");
  const [tunnel] = await db.select().from(cloudflareTunnels).where(eq(cloudflareTunnels.id, route.tunnelId)).limit(1);
  if (!tunnel) throw new Error("Tunnel not found.");
  const client = await requireClient(tunnel.clientId);
  await db.update(cloudflareRoutes).set({ enabled: true, syncStatus: "pending", lastError: null, updatedAt: new Date() }).where(eq(cloudflareRoutes.id, route.id));
  try {
    await putTunnelConfig(tunnel);
    const cfDnsRecordId = await upsertDnsRecordForRoute(client, tunnel, route);
    const [updated] = await db.update(cloudflareRoutes).set({ cfDnsRecordId, syncStatus: "active", lastError: null, lastPublishedAt: new Date(), updatedAt: new Date() }).where(eq(cloudflareRoutes.id, route.id)).returning();
    return updated;
  } catch (error) {
    const message = error instanceof Error ? error.message : "Cloudflare synchronization failed.";
    await db.update(cloudflareRoutes).set({ enabled: false, syncStatus: "error", lastError: message, updatedAt: new Date() }).where(eq(cloudflareRoutes.id, route.id));
    await putTunnelConfig(tunnel).catch(() => undefined);
    throw error;
  }
}

export async function deleteManagedHostname(routeId: string) {
  const [route] = await db.select().from(cloudflareRoutes).where(eq(cloudflareRoutes.id, routeId)).limit(1);
  if (!route) throw new Error("Hostname not found.");
  const [tunnel] = await db.select().from(cloudflareTunnels).where(eq(cloudflareTunnels.id, route.tunnelId)).limit(1);
  if (!tunnel) throw new Error("Tunnel not found.");
  const client = await requireClient(tunnel.clientId);
  await deleteDnsRecordForRoute(client, route);
  await db.delete(cloudflareRoutes).where(eq(cloudflareRoutes.id, route.id));
  await putTunnelConfig(tunnel);
}

type CfDnsRecord = { id: string };

type CfDnsRecordResult = {
  id: string;
  type: string;
  name: string;
  content: string;
  ttl: number;
  proxiable?: boolean;
  proxied?: boolean;
  priority?: number | null;
  comment?: string | null;
  created_on?: string;
  modified_on?: string;
};

type CloudflareDnsRecordInput = {
  type: CloudflareDnsRecordType;
  name: string;
  content: string;
  ttl?: number;
  proxied?: boolean;
  priority?: number | null;
  comment?: string | null;
};

function toPublicDnsRecord(record: CfDnsRecordResult): CloudflareDnsRecord {
  return {
    id: record.id,
    type: record.type,
    name: record.name,
    content: record.content,
    ttl: record.ttl,
    proxiable: Boolean(record.proxiable),
    proxied: Boolean(record.proxied),
    priority: record.priority ?? null,
    comment: record.comment ?? null,
    createdOn: record.created_on ?? null,
    modifiedOn: record.modified_on ?? null
  };
}

function ensureDnsSettings(settings: CloudflareSettings) {
  if (!settings.zoneId) {
    throw new Error("Cloudflare Zone ID is not configured.");
  }
  if (!settings.apiToken) {
    throw new Error("Cloudflare API token is not configured.");
  }
}

function dnsRecordPayload(input: CloudflareDnsRecordInput) {
  const proxied = ["A", "AAAA", "CNAME"].includes(input.type) ? Boolean(input.proxied) : false;
  return {
    type: input.type,
    name: normalizeString(input.name),
    content: normalizeString(input.content),
    ttl: input.ttl ?? 1,
    proxied,
    ...(input.type === "MX" && input.priority != null ? { priority: input.priority } : {}),
    ...(input.comment ? { comment: normalizeString(input.comment) } : {})
  };
}

export async function listDnsRecords(): Promise<CloudflareDnsRecord[]> {
  const settings = await getStoredCloudflareSettings();
  ensureDnsSettings(settings);
  const records = await cfFetch<CfDnsRecordResult[]>(
    settings,
    `/zones/${settings.zoneId}/dns_records?per_page=200&order=type&direction=asc`
  );
  return records.map(toPublicDnsRecord);
}

export async function createDnsRecord(input: CloudflareDnsRecordInput): Promise<CloudflareDnsRecord> {
  const settings = await getStoredCloudflareSettings();
  ensureDnsSettings(settings);
  const created = await cfFetch<CfDnsRecordResult>(settings, `/zones/${settings.zoneId}/dns_records`, {
    method: "POST",
    body: JSON.stringify(dnsRecordPayload(input))
  });
  return toPublicDnsRecord(created);
}

export async function updateDnsRecord(recordId: string, input: CloudflareDnsRecordInput): Promise<CloudflareDnsRecord> {
  const settings = await getStoredCloudflareSettings();
  ensureDnsSettings(settings);
  const updated = await cfFetch<CfDnsRecordResult>(settings, `/zones/${settings.zoneId}/dns_records/${recordId}`, {
    method: "PATCH",
    body: JSON.stringify(dnsRecordPayload(input))
  });
  return toPublicDnsRecord(updated);
}

export async function deleteDnsRecord(recordId: string): Promise<void> {
  const settings = await getStoredCloudflareSettings();
  ensureDnsSettings(settings);
  await cfFetch<{ id: string }>(settings, `/zones/${settings.zoneId}/dns_records/${recordId}`, { method: "DELETE" });
}

function diagnosticDnsRecord(record: CloudflareDnsRecord): CloudflareRouteDiagnosticDnsRecord {
  return {
    id: record.id,
    type: record.type,
    name: record.name,
    content: record.content,
    proxied: record.proxied
  };
}

async function checkHostnameReachability(hostname: string): Promise<CloudflareRouteReachabilityStatus> {
  const url = `https://${hostname}`;
  const request = async (method: "HEAD" | "GET") => {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), 3500);
    try {
      return await fetch(url, { method, signal: controller.signal });
    } finally {
      clearTimeout(timer);
    }
  };

  try {
    const head = await request("HEAD");
    if (head.status === 405) {
      const get = await request("GET");
      return get.status < 400 ? "ok" : "failed";
    }
    return head.status < 400 ? "ok" : "failed";
  } catch {
    return "failed";
  }
}

export async function listRouteDiagnostics(): Promise<CloudflareRouteDiagnostic[]> {
  const checkedAt = new Date().toISOString();
  const [routes, tunnels, projectRows] = await Promise.all([
    db.select().from(cloudflareRoutes),
    listTunnels(),
    db.select().from(projects)
  ]);
  const tunnelById = new Map(tunnels.map((tunnel) => [tunnel.id, tunnel]));
  const projectById = new Map(projectRows.map((project) => [project.id, project]));
  const runtimeByNodeId = new Map<string, Promise<{ running: boolean } | null>>();
  const healthByTunnelId = new Map<string, Promise<{ healthy: boolean } | null>>();

  let dnsRecords: CloudflareDnsRecord[] | null = null;
  try {
    dnsRecords = await listDnsRecords();
  } catch {
    dnsRecords = null;
  }

  const runtimeFor = (tunnelId: string) => {
    if (!runtimeByNodeId.has(tunnelId)) {
      runtimeByNodeId.set(tunnelId, getCloudflaredStatus(tunnelId).catch(() => null));
    }
    return runtimeByNodeId.get(tunnelId)!;
  };
  const healthFor = (tunnel: CloudflareTunnelRow) => {
    if (!healthByTunnelId.has(tunnel.id)) {
      healthByTunnelId.set(tunnel.id, getTunnelHealth(tunnel).catch(() => null));
    }
    return healthByTunnelId.get(tunnel.id)!;
  };

  return Promise.all(routes.map(async (route) => {
    const tunnel = tunnelById.get(route.tunnelId);
    const project = route.projectId ? projectById.get(route.projectId) : undefined;
    const expectedDnsTarget = tunnel ? `${tunnel.cfTunnelId}.cfargotunnel.com` : null;
    const actualDnsRecords = (dnsRecords ?? [])
      .filter((record) => record.name.toLowerCase() === route.hostname.toLowerCase())
      .map(diagnosticDnsRecord);
    const [runtime, health] = tunnel ? await Promise.all([runtimeFor(tunnel.id), healthFor(tunnel)]) : [null, null];
    const preliminary = classifyRouteDiagnostic({
      routeEnabled: route.enabled,
      hostname: route.hostname,
      expectedDnsTarget,
      dnsRecords: dnsRecords ? actualDnsRecords : null,
      tunnelExists: Boolean(tunnel),
      tunnelRuntimeRunning: runtime ? runtime.running : tunnel ? null : false,
      tunnelHealthy: health ? health.healthy : tunnel ? null : false,
      reachabilityStatus: "skipped"
    });
    const reachabilityStatus =
      route.enabled && preliminary.dnsStatus === "ok" && preliminary.tunnelStatus === "running"
        ? await checkHostnameReachability(route.hostname)
        : "skipped";
    const classification = classifyRouteDiagnostic({
      routeEnabled: route.enabled,
      hostname: route.hostname,
      expectedDnsTarget,
      dnsRecords: dnsRecords ? actualDnsRecords : null,
      tunnelExists: Boolean(tunnel),
      tunnelRuntimeRunning: runtime ? runtime.running : tunnel ? null : false,
      tunnelHealthy: health ? health.healthy : tunnel ? null : false,
      reachabilityStatus
    });

    return {
      routeId: route.id,
      tunnelId: route.tunnelId,
      projectId: route.projectId,
      projectName: project?.name ?? null,
      hostname: route.hostname,
      serviceTarget: route.serviceTarget,
      routeEnabled: route.enabled,
      expectedDnsTarget,
      actualDnsRecords,
      checkedAt,
      ...classification
    };
  }));
}

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
  const [existingRoute] = await db.select().from(cloudflareRoutes).where(eq(cloudflareRoutes.projectId, projectId)).limit(1);
  const normalizedNoTlsVerify = serviceTarget.startsWith("https://") ? noTlsVerify : false;

  if (existingRoute && existingRoute.hostname !== hostname) {
    const [existingTunnel] = await db.select().from(cloudflareTunnels).where(eq(cloudflareTunnels.id, existingRoute.tunnelId)).limit(1);
    if (existingTunnel) {
      await deleteTunnelDnsRecord(settings, existingTunnel.cfTunnelId, existingRoute.hostname, existingRoute.cfDnsRecordId);
    }
  }

  let routeId = existingRoute?.id ?? createId("cfr");
  if (existingRoute) {
    await db
      .update(cloudflareRoutes)
      .set({
        tunnelId: tunnel.id,
        hostname,
        serviceTarget,
        noTlsVerify: normalizedNoTlsVerify,
        enabled: true,
        cfDnsRecordId: null,
        updatedAt: new Date()
      })
      .where(eq(cloudflareRoutes.id, existingRoute.id));
  } else {
    const [created] = await db
      .insert(cloudflareRoutes)
      .values({
        id: routeId,
        tunnelId: tunnel.id,
        projectId,
        assignmentId: null,
        zoneId: settings.zoneId || "legacy",
        hostname,
        serviceTarget,
        protocol: serviceTarget.startsWith("https://") ? "https" : "http",
        port: Number(serviceTarget.match(/:(\d+)$/)?.[1] ?? 80),
        noTlsVerify: normalizedNoTlsVerify,
        enabled: true
      })
      .returning();
    routeId = created.id;
  }

  const dnsRecordId = await upsertTunnelDnsRecord(settings, tunnel.cfTunnelId, hostname);
  if (dnsRecordId) {
    await db
      .update(cloudflareRoutes)
      .set({ cfDnsRecordId: dnsRecordId, lastPublishedAt: new Date() })
      .where(eq(cloudflareRoutes.id, routeId));
  }

  await putTunnelConfig(tunnel);
  if (existingRoute?.tunnelId && existingRoute.tunnelId !== tunnel.id) {
    const [previousTunnel] = await db.select().from(cloudflareTunnels).where(eq(cloudflareTunnels.id, existingRoute.tunnelId)).limit(1);
    if (previousTunnel) await putTunnelConfig(decryptTunnelRow(previousTunnel));
  }
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
  const project = route.projectId ? await getProject(route.projectId) : undefined;
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
  const containerName = cloudflaredContainerName(tunnel.id);
  const legacyContainerName = `yanto-cloudflared-${tunnel.nodeId}`;

  // Remove stale container if it exists (ignore error if it doesn't)
  try {
    await runCommand("docker", ["rm", "-f", containerName]);
    if (legacyContainerName !== containerName) await runCommand("docker", ["rm", "-f", legacyContainerName]);
  } catch {
    // container doesn't exist, that's fine
  }

  await fs.promises.mkdir(config.cloudflaredDir, { recursive: true, mode: 0o700 });
  if (!(await dockerNetworkExists(tunnel.dockerNetworkName))) await runRequiredDocker(["network", "create", "--label", `yanto.cloudflared.tunnel=${tunnel.id}`, tunnel.dockerNetworkName]);
  const envPath = path.join(config.cloudflaredDir, `${tunnel.id}.env`);
  await fs.promises.writeFile(envPath, `TUNNEL_TOKEN=${tunnel.tunnelToken}\n`, { mode: 0o600 });

  await runRequiredDocker([
    "run", "-d",
    "--name", containerName,
    "--restart", "unless-stopped",
    "--label", `yanto.cloudflared.tunnel=${tunnel.id}`,
    "--network", tunnel.dockerNetworkName,
    "--env-file", envPath,
    "cloudflare/cloudflared:latest",
    "tunnel", "--no-autoupdate", "run"
  ]);

  await db.update(cloudflareTunnels).set({ status: "active", updatedAt: new Date() }).where(eq(cloudflareTunnels.id, tunnel.id));
}

export async function stopCloudflared(nodeId: string): Promise<void> {
  const [tunnel] = await db.select().from(cloudflareTunnels).where(eq(cloudflareTunnels.id, nodeId)).limit(1);
  const resolved = tunnel ?? (await db.select().from(cloudflareTunnels).where(eq(cloudflareTunnels.nodeId, nodeId)).limit(1))[0];
  if (!resolved) throw new Error("Tunnel not found.");
  const containerName = cloudflaredContainerName(resolved.id);
  await runRequiredDocker(["stop", containerName]);
  await runRequiredDocker(["rm", containerName]);

  await db.update(cloudflareTunnels).set({ status: "stopped", updatedAt: new Date() }).where(eq(cloudflareTunnels.id, resolved.id));
}

export async function restartCloudflared(nodeId: string): Promise<void> {
  const [byId] = await db.select().from(cloudflareTunnels).where(eq(cloudflareTunnels.id, nodeId)).limit(1);
  const tunnel = byId ?? (await db.select().from(cloudflareTunnels).where(eq(cloudflareTunnels.nodeId, nodeId)).limit(1))[0];
  if (!tunnel) throw new Error("No tunnel found for this node.");

  try {
    await stopCloudflared(nodeId);
  } catch {
    // container may not be running
  }
  await startCloudflared(decryptTunnelRow(tunnel));
}

export async function getCloudflaredStatus(nodeId: string): Promise<{ running: boolean; containerId?: string; status?: string }> {
  const [byId] = await db.select().from(cloudflareTunnels).where(eq(cloudflareTunnels.id, nodeId)).limit(1);
  const tunnel = byId ?? (await db.select().from(cloudflareTunnels).where(eq(cloudflareTunnels.nodeId, nodeId)).limit(1))[0];
  if (!tunnel) return { running: false };
  const containerName = cloudflaredContainerName(tunnel.id);
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
  const runtime = await getCloudflaredStatus(tunnel.id);
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
      const runtime = await getCloudflaredStatus(tunnel.id);
      if (!runtime.running) {
        await startCloudflared(tunnel);
        started.push(tunnel.nodeId);
      }

      const enabledRoutes = routes.filter((route) => route.enabled && route.tunnelId === tunnel.id);
      for (const route of enabledRoutes) {
        const project = route.projectId ? await getProject(route.projectId) : undefined;
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
    const client = await requireClient(tunnel.clientId);
    type CfHealthResult = { status: string; connections?: unknown[] };
    const result = await cfFetch<CfHealthResult>({ apiToken: client.apiToken }, `/accounts/${tunnel.cfAccountId}/cfd_tunnel/${tunnel.cfTunnelId}`);
    return {
      healthy: result.status === "healthy",
      connectors: result.connections?.length ?? 0,
      status: result.status
    };
  } catch {
    return { healthy: false };
  }
}
