import fs from "node:fs/promises";
import { and, asc, eq } from "drizzle-orm";
import type { FrpClientStatus, FrpOverview, FrpServerStatus, FrpTunnel, FrpTunnelStatus } from "../../shared/types.js";
import { config } from "../config.js";
import { db } from "../db/index.js";
import { appSettings, deploymentNodes, frpTunnels, frpWorkerStates, type FrpTunnelRow } from "../db/schema.js";
import { runCommand } from "./commands.js";
import { getNode } from "./nodes.js";
import { createId, hashToken } from "./tokens.js";
import { HttpError } from "../http-utils.js";

const publicHostKey = "frp.public_host";

export type FrpTunnelInput = {
  name: string;
  nodeId: string;
  protocol: "tcp" | "udp";
  localHost: string;
  localPort: number;
  remotePort: number;
  enabled: boolean;
};

type DashboardProxy = {
  name?: string;
  status?: string;
  trafficIn?: number;
  trafficOut?: number;
  traffic_in?: number;
  traffic_out?: number;
  todayTrafficIn?: number;
  todayTrafficOut?: number;
  curConns?: number;
  cur_conns?: number;
};

type DashboardClient = {
  user?: string;
  clientID?: string;
  version?: string;
  protocol?: string;
  wireProtocol?: string;
  status?: string;
  online?: boolean;
};

function numberValue(value: unknown) {
  const parsed = Number(value ?? 0);
  return Number.isFinite(parsed) ? parsed : 0;
}

async function readFrpToken() {
  try {
    return (await fs.readFile(config.frpTokenPath, "utf8")).trim();
  } catch {
    return "";
  }
}

async function dashboardRequest<T>(pathname: string): Promise<T> {
  const token = await readFrpToken();
  if (!token) throw new Error("FRP authentication token is not initialized.");
  const response = await fetch(`${config.frpDashboardUrl.replace(/\/$/, "")}${pathname}`, {
    headers: { Authorization: `Basic ${Buffer.from(`yanto:${token}`).toString("base64")}` },
    signal: AbortSignal.timeout(2500)
  });
  if (!response.ok) throw new Error(`FRP dashboard returned ${response.status}.`);
  return response.json() as Promise<T>;
}

async function containerState() {
  const result = await runCommand("docker", ["inspect", "--format", "{{json .State}}", config.frpContainerName], { timeoutMs: 5000 });
  if (result.exitCode !== 0) return null;
  try {
    const state = JSON.parse(result.output.trim()) as { Status?: string; StartedAt?: string };
    return { status: state.Status ?? null, startedAt: state.StartedAt ?? null };
  } catch {
    return null;
  }
}

export async function publicFrpSettings() {
  const [row] = await db.select().from(appSettings).where(eq(appSettings.key, publicHostKey)).limit(1);
  const publicHost = row?.value.trim() ?? "";
  return {
    publicHost,
    bindPort: config.frpBindPort,
    portStart: config.frpPortStart,
    portEnd: config.frpPortEnd,
    configured: Boolean(publicHost)
  };
}

export async function saveFrpSettings(publicHost: string) {
  const value = publicHost.trim();
  await db
    .insert(appSettings)
    .values({ key: publicHostKey, value, updatedAt: new Date() })
    .onConflictDoUpdate({ target: appSettings.key, set: { value, updatedAt: new Date() } });
  return publicFrpSettings();
}

export function validateFrpRemotePort(remotePort: number) {
  if (remotePort < config.frpPortStart || remotePort > config.frpPortEnd) {
    throw new HttpError(400, `Public port must be between ${config.frpPortStart} and ${config.frpPortEnd}.`);
  }
}

async function assertWorkerNode(nodeId: string) {
  const node = await getNode(nodeId);
  if (!node || node.role !== "worker") throw new HttpError(400, "FRP tunnels must target an enrolled worker node.");
  return node;
}

function rethrowTunnelWriteError(error: unknown): never {
  if (error && typeof error === "object" && "code" in error && error.code === "23505") {
    throw new HttpError(409, "That public port is already used by another tunnel with the same protocol.");
  }
  throw error;
}

export async function createFrpTunnel(input: FrpTunnelInput) {
  await assertWorkerNode(input.nodeId);
  validateFrpRemotePort(input.remotePort);
  if (input.enabled && !(await publicFrpSettings()).configured) throw new HttpError(400, "Save the VPS public endpoint before enabling a tunnel.");
  try {
    const [row] = await db.insert(frpTunnels).values({
      id: createId("frp"),
      ...input,
      syncStatus: input.enabled ? "syncing" : "disabled"
    }).returning();
    return row;
  } catch (error) {
    rethrowTunnelWriteError(error);
  }
}

export async function updateFrpTunnel(id: string, input: Partial<FrpTunnelInput>) {
  const [current] = await db.select().from(frpTunnels).where(eq(frpTunnels.id, id)).limit(1);
  if (!current) throw new HttpError(404, "FRP tunnel not found.");
  if (input.nodeId) await assertWorkerNode(input.nodeId);
  if (input.remotePort !== undefined) validateFrpRemotePort(input.remotePort);
  const enabled = input.enabled ?? current.enabled;
  if (enabled && !(await publicFrpSettings()).configured) throw new HttpError(400, "Save the VPS public endpoint before enabling a tunnel.");
  try {
    const [row] = await db.update(frpTunnels).set({
      ...input,
      syncStatus: enabled ? "syncing" : "disabled",
      lastError: null,
      updatedAt: new Date()
    }).where(eq(frpTunnels.id, id)).returning();
    return row;
  } catch (error) {
    rethrowTunnelWriteError(error);
  }
}

export async function deleteFrpTunnel(id: string) {
  const [row] = await db.delete(frpTunnels).where(eq(frpTunnels.id, id)).returning();
  if (!row) throw new HttpError(404, "FRP tunnel not found.");
  return row;
}

async function desiredPayload(nodeId: string) {
  const settings = await publicFrpSettings();
  const tunnels = await db.select().from(frpTunnels)
    .where(and(eq(frpTunnels.nodeId, nodeId), eq(frpTunnels.enabled, true)))
    .orderBy(asc(frpTunnels.createdAt));
  const desired = {
    nodeId,
    serverAddr: settings.publicHost,
    serverPort: settings.bindPort,
    wireProtocol: "v2",
    tunnels: tunnels.map((tunnel) => ({
      id: tunnel.id,
      name: tunnel.name,
      protocol: tunnel.protocol,
      localHost: tunnel.localHost,
      localPort: tunnel.localPort,
      remotePort: tunnel.remotePort
    }))
  };
  return { settings, tunnels, desired, revision: hashToken(JSON.stringify(desired)) };
}

export async function workerFrpConfig(nodeId: string) {
  const { settings, desired, revision } = await desiredPayload(nodeId);
  return {
    configured: settings.configured,
    ...desired,
    revision,
    authToken: settings.configured ? await readFrpToken() : ""
  };
}

export async function updateFrpWorkerState(nodeId: string, input: {
  appliedRevision?: string | null;
  processStatus: "running" | "stopped" | "error";
  frpcVersion?: string | null;
  lastError?: string | null;
}) {
  const now = new Date();
  const { revision } = await desiredPayload(nodeId);
  await db.insert(frpWorkerStates).values({
    nodeId,
    desiredRevision: revision,
    appliedRevision: input.appliedRevision ?? null,
    processStatus: input.processStatus,
    frpcVersion: input.frpcVersion ?? null,
    lastError: input.lastError ?? null,
    lastReportedAt: now,
    updatedAt: now
  }).onConflictDoUpdate({
    target: frpWorkerStates.nodeId,
    set: {
      appliedRevision: input.appliedRevision ?? null,
      desiredRevision: revision,
      processStatus: input.processStatus,
      frpcVersion: input.frpcVersion ?? null,
      lastError: input.lastError ?? null,
      lastReportedAt: now,
      updatedAt: now
    }
  });

  const synchronized = input.processStatus === "running" && input.appliedRevision === revision;
  await db.update(frpTunnels).set({
    syncStatus: input.processStatus === "error" ? "error" : synchronized ? "offline" : "syncing",
    lastError: input.lastError ?? null,
    lastSyncedAt: synchronized ? now : null
  }).where(and(eq(frpTunnels.nodeId, nodeId), eq(frpTunnels.enabled, true)));
}

function proxyNames(proxy: DashboardProxy) {
  const name = proxy.name ?? "";
  return [name, name.includes(".") ? name.slice(name.lastIndexOf(".") + 1) : name];
}

function tunnelStatus(row: FrpTunnelRow, workerRevision: string | null, desiredRevision: string, proxy?: DashboardProxy): FrpTunnelStatus {
  if (!row.enabled) return "disabled";
  if (row.lastError || row.syncStatus === "error") return "error";
  if (!workerRevision || workerRevision !== desiredRevision || row.syncStatus === "syncing") return "syncing";
  return proxy?.status?.toLowerCase() === "online" ? "online" : "offline";
}

async function dashboardData() {
  const [server, clients, tcp, udp] = await Promise.all([
    dashboardRequest<Record<string, unknown>>("/api/serverinfo"),
    dashboardRequest<{ clients?: DashboardClient[] } | DashboardClient[]>("/api/clients").catch(() => ({ clients: [] })),
    dashboardRequest<{ proxies?: DashboardProxy[] } | DashboardProxy[]>("/api/proxy/tcp").catch(() => ({ proxies: [] })),
    dashboardRequest<{ proxies?: DashboardProxy[] } | DashboardProxy[]>("/api/proxy/udp").catch(() => ({ proxies: [] }))
  ]);
  const clientRows = Array.isArray(clients) ? clients : clients.clients ?? [];
  const tcpRows = Array.isArray(tcp) ? tcp : tcp.proxies ?? [];
  const udpRows = Array.isArray(udp) ? udp : udp.proxies ?? [];
  return { server, clients: clientRows, proxies: [...tcpRows, ...udpRows] };
}

async function frpServerStatus(): Promise<{ status: FrpServerStatus; dashboard: Awaited<ReturnType<typeof dashboardData>> | null }> {
  const state = await containerState();
  if (state?.status !== "running") {
    return {
      status: { running: false, containerStatus: state?.status ?? null, version: null, uptimeSeconds: null, trafficInBytes: 0, trafficOutBytes: 0, error: state ? null : "FRP server container was not found." },
      dashboard: null
    };
  }
  try {
    const dashboard = await dashboardData();
    const info = dashboard.server;
    const startedAt = state.startedAt ? Date.parse(state.startedAt) : Number.NaN;
    const uptimeSeconds = Number.isNaN(startedAt) ? null : Math.max(0, Math.floor((Date.now() - startedAt) / 1000));
    return {
      status: {
        running: true,
        containerStatus: state.status,
        version: String(info.version ?? info.serverVersion ?? "") || null,
        uptimeSeconds,
        trafficInBytes: numberValue(info.total_traffic_in ?? info.totalTrafficIn),
        trafficOutBytes: numberValue(info.total_traffic_out ?? info.totalTrafficOut),
        error: null
      },
      dashboard
    };
  } catch (error) {
    return {
      status: { running: true, containerStatus: state.status, version: null, uptimeSeconds: null, trafficInBytes: 0, trafficOutBytes: 0, error: error instanceof Error ? error.message : "FRP dashboard is unavailable." },
      dashboard: null
    };
  }
}

export async function frpOverview(): Promise<FrpOverview> {
  const [settings, tunnelRows, workerRows, runtime] = await Promise.all([
    publicFrpSettings(),
    db.select({ tunnel: frpTunnels, nodeName: deploymentNodes.name }).from(frpTunnels).leftJoin(deploymentNodes, eq(frpTunnels.nodeId, deploymentNodes.id)).orderBy(asc(frpTunnels.createdAt)),
    db.select({ state: frpWorkerStates, node: deploymentNodes }).from(deploymentNodes).leftJoin(frpWorkerStates, eq(frpWorkerStates.nodeId, deploymentNodes.id)).where(eq(deploymentNodes.role, "worker")).orderBy(asc(deploymentNodes.name)),
    frpServerStatus()
  ]);

  const dashboardClients = new Map((runtime.dashboard?.clients ?? []).map((client) => [client.user || client.clientID || "", client]));
  const proxies = runtime.dashboard?.proxies ?? [];
  const desiredRevisions = new Map<string, string>();
  await Promise.all(workerRows.map(async ({ node }) => desiredRevisions.set(node.id, (await desiredPayload(node.id)).revision)));

  const clients: FrpClientStatus[] = workerRows.map(({ node, state }) => {
    const dashboard = dashboardClients.get(node.id);
    const workerOnline = node.lastSeenAt && Date.now() - new Date(node.lastSeenAt).getTime() < 60_000;
    return {
      nodeId: node.id,
      nodeName: node.name,
      workerStatus: workerOnline ? "online" : "offline",
      frpcStatus: dashboard ? (dashboard.online === false ? "offline" : "online") : state?.processStatus ?? "stopped",
      frpcVersion: dashboard?.version ?? state?.frpcVersion ?? null,
      protocol: dashboard?.wireProtocol ?? dashboard?.protocol ?? null,
      lastSeenAt: node.lastSeenAt?.toISOString() ?? null,
      lastError: state?.lastError ?? null
    };
  });

  const tunnels: FrpTunnel[] = tunnelRows.map(({ tunnel, nodeName }) => {
    const proxy = proxies.find((candidate) => proxyNames(candidate).includes(tunnel.id));
    const worker = workerRows.find(({ node }) => node.id === tunnel.nodeId)?.state;
    return {
      id: tunnel.id,
      nodeId: tunnel.nodeId,
      name: tunnel.name,
      protocol: tunnel.protocol as "tcp" | "udp",
      localHost: tunnel.localHost,
      localPort: tunnel.localPort,
      remotePort: tunnel.remotePort,
      enabled: tunnel.enabled,
      nodeName,
      syncStatus: tunnelStatus(tunnel, worker?.appliedRevision ?? null, desiredRevisions.get(tunnel.nodeId) ?? "", proxy),
      lastError: tunnel.lastError,
      lastSyncedAt: tunnel.lastSyncedAt?.toISOString() ?? null,
      trafficInBytes: numberValue(proxy?.todayTrafficIn ?? proxy?.trafficIn ?? proxy?.traffic_in),
      trafficOutBytes: numberValue(proxy?.todayTrafficOut ?? proxy?.trafficOut ?? proxy?.traffic_out),
      currentConnections: numberValue(proxy?.curConns ?? proxy?.cur_conns),
      createdAt: tunnel.createdAt.toISOString(),
      updatedAt: tunnel.updatedAt.toISOString()
    };
  });

  return { settings, server: runtime.status, clients, tunnels };
}

export async function controlFrpServer(action: "start" | "stop" | "restart") {
  const result = await runCommand("docker", [action, config.frpContainerName], { timeoutMs: 30_000 });
  if (result.exitCode !== 0) throw new HttpError(503, `Unable to ${action} FRP server container.`);
  return frpServerStatus().then((result) => result.status);
}
