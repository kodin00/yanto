import fs from "node:fs/promises";
import { createHash, randomBytes } from "node:crypto";
import { and, asc, eq, isNull, or, sql } from "drizzle-orm";
import type { FrpClientSetup, FrpOverview, FrpServerStatus, FrpTunnel, FrpTunnelStatus } from "../../shared/types.js";
import { config } from "../config.js";
import { db } from "../db/index.js";
import { appSettings, deploymentNodes, frpNodeAssignments, frpServers, frpTunnels, type FrpServerRow, type FrpTunnelRow } from "../db/schema.js";
import { runCommand } from "./commands.js";
import { createId } from "./tokens.js";
import { HttpError } from "../http-utils.js";
import { decrypt, encrypt, isEncrypted } from "./crypto.js";

const publicHostKey = "frp.public_host";

export type FrpTunnelInput = {
  name: string;
  nodeId?: string | null;
  clientNodeId?: string | null;
  serverId?: string | null;
  protocol: "tcp" | "udp";
  localHost: string;
  localPort: number;
  remotePort: number;
  enabled: boolean;
};

export type FrpRole = "disabled" | "client" | "server" | "both";

export type FrpServerInput = {
  nodeId: string;
  name: string;
  publicHost: string;
  bindPort: number;
  portStart: number;
  portEnd: number;
  authToken?: string;
};

export type FrpNodeStatusInput = {
  revision: number;
  status: "applying" | "online" | "error" | "disabled";
  error?: string | null;
};

type DashboardProxy = {
  name?: string;
  status?: string;
  proxyType?: "tcp" | "udp";
  conf?: {
    remotePort?: number | string;
  };
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

export type FrpConfigTunnel = Pick<FrpTunnelRow, "id" | "protocol" | "localHost" | "localPort" | "remotePort">;

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

function validatePortRange(bindPort: number, portStart: number, portEnd: number) {
  if (![bindPort, portStart, portEnd].every((value) => Number.isInteger(value) && value >= 1 && value <= 65_535)) {
    throw new HttpError(400, "FRP ports must be integers between 1 and 65535.");
  }
  if (portStart > portEnd) throw new HttpError(400, "The FRP forwarding port range is invalid.");
}

async function serverForTunnel(serverId?: string | null) {
  if (!serverId) return null;
  const [server] = await db.select().from(frpServers).where(eq(frpServers.id, serverId)).limit(1);
  if (!server) throw new HttpError(404, "FRP server not found.");
  return server;
}

async function validateTunnelDestination(input: { remotePort: number; serverId?: string | null; clientNodeId?: string | null; allowLegacyUnassigned?: boolean }) {
  const server = await serverForTunnel(input.serverId);
  if (server && (input.remotePort < server.portStart || input.remotePort > server.portEnd)) {
    throw new HttpError(400, `Public port must be between ${server.portStart} and ${server.portEnd} for ${server.name}.`);
  }
  if (!server) validateFrpRemotePort(input.remotePort);
  if (input.clientNodeId && !server && !input.allowLegacyUnassigned) throw new HttpError(400, "A managed client tunnel must select an FRP server.");
  if (input.clientNodeId && server) {
    const [assignment] = await db.select().from(frpNodeAssignments).where(eq(frpNodeAssignments.nodeId, input.clientNodeId)).limit(1);
    if (!assignment || (assignment.role !== "client" && assignment.role !== "both") || assignment.serverId !== server.id) {
      throw new HttpError(400, "The tunnel client is not assigned to the selected FRP server.");
    }
  }
}

async function bumpFrpAssignmentRevision(nodeId?: string | null) {
  if (!nodeId) return;
  await db.update(frpNodeAssignments).set({
    desiredRevision: sql`${frpNodeAssignments.desiredRevision} + 1`,
    status: "pending",
    lastError: null,
    updatedAt: new Date()
  }).where(eq(frpNodeAssignments.nodeId, nodeId));
}

async function syncFrpSshFallback(tunnel: Pick<FrpTunnelRow, "clientNodeId" | "serverId" | "protocol" | "localPort" | "remotePort" | "enabled">) {
  if (!tunnel.enabled || tunnel.protocol !== "tcp" || tunnel.localPort !== 22 || !tunnel.clientNodeId || !tunnel.serverId) return;
  const [server, node] = await Promise.all([
    serverForTunnel(tunnel.serverId),
    db.select().from(deploymentNodes).where(eq(deploymentNodes.id, tunnel.clientNodeId)).limit(1).then((rows) => rows[0])
  ]);
  if (!server || !node) return;
  await db.update(deploymentNodes).set({
    labels: { ...node.labels, "frp.sshHost": server.publicHost, "frp.sshPort": String(tunnel.remotePort) },
    updatedAt: new Date()
  }).where(eq(deploymentNodes.id, node.id));
}

function rethrowTunnelWriteError(error: unknown): never {
  if (error && typeof error === "object" && "code" in error && error.code === "23505") {
    throw new HttpError(409, "That public port is already used by another tunnel with the same protocol.");
  }
  throw error;
}

export async function createFrpTunnel(input: FrpTunnelInput) {
  await validateTunnelDestination(input);
  if (input.enabled && !input.serverId && !(await publicFrpSettings()).configured) throw new HttpError(400, "Save the VPS public endpoint before enabling a tunnel.");
  try {
    const [row] = await db.insert(frpTunnels).values({
      id: createId("frp"),
      ...input,
      nodeId: input.nodeId ?? null,
      clientNodeId: input.clientNodeId ?? null,
      serverId: input.serverId ?? null,
      syncStatus: input.enabled ? "offline" : "disabled"
    }).returning();
    await bumpFrpAssignmentRevision(row.clientNodeId);
    await syncFrpSshFallback(row);
    return row;
  } catch (error) {
    rethrowTunnelWriteError(error);
  }
}

export async function updateFrpTunnel(id: string, input: Partial<FrpTunnelInput>) {
  const [current] = await db.select().from(frpTunnels).where(eq(frpTunnels.id, id)).limit(1);
  if (!current) throw new HttpError(404, "FRP tunnel not found.");
  await validateTunnelDestination({
    remotePort: input.remotePort ?? current.remotePort,
    serverId: input.serverId === undefined ? current.serverId : input.serverId,
    clientNodeId: input.clientNodeId === undefined ? current.clientNodeId : input.clientNodeId,
    allowLegacyUnassigned: current.serverId === null && current.nodeId !== null && current.clientNodeId === current.nodeId && input.serverId === undefined && input.clientNodeId === undefined
  });
  const enabled = input.enabled ?? current.enabled;
  const serverId = input.serverId === undefined ? current.serverId : input.serverId;
  if (enabled && !serverId && !(await publicFrpSettings()).configured) throw new HttpError(400, "Save the VPS public endpoint before enabling a tunnel.");
  try {
    const [row] = await db.update(frpTunnels).set({
      ...input,
      syncStatus: enabled ? "offline" : "disabled",
      lastError: null,
      updatedAt: new Date()
    }).where(eq(frpTunnels.id, id)).returning();
    await Promise.all([bumpFrpAssignmentRevision(current.clientNodeId), bumpFrpAssignmentRevision(row.clientNodeId)]);
    await syncFrpSshFallback(row);
    return row;
  } catch (error) {
    rethrowTunnelWriteError(error);
  }
}

export async function deleteFrpTunnel(id: string) {
  const [row] = await db.delete(frpTunnels).where(eq(frpTunnels.id, id)).returning();
  if (!row) throw new HttpError(404, "FRP tunnel not found.");
  await bumpFrpAssignmentRevision(row.clientNodeId);
  return row;
}

function proxyNames(proxy: DashboardProxy) {
  const name = proxy.name ?? "";
  return [
    name,
    name.includes(".") ? name.slice(name.lastIndexOf(".") + 1) : name,
    name.includes("/") ? name.slice(name.lastIndexOf("/") + 1) : name
  ];
}

type FrpTunnelIdentity = Pick<FrpTunnelRow, "id" | "name" | "protocol" | "remotePort">;

export function findFrpTunnelProxy(tunnel: FrpTunnelIdentity, proxies: DashboardProxy[]) {
  const matching = proxies.filter((proxy) => {
    if (proxy.proxyType && proxy.proxyType !== tunnel.protocol) return false;
    if (proxyNames(proxy).some((name) => name === tunnel.id || name === tunnel.name)) return true;
    const remotePort = Number(proxy.conf?.remotePort);
    return Number.isInteger(remotePort) && remotePort === tunnel.remotePort;
  });
  return matching.find((proxy) => proxy.status?.toLowerCase() === "online") ?? matching[0];
}

function tunnelStatus(row: FrpTunnelRow, proxy?: DashboardProxy): FrpTunnelStatus {
  if (!row.enabled) return "disabled";
  if (row.lastError || row.syncStatus === "error") return "error";
  return proxy?.status?.toLowerCase() === "online" ? "online" : "offline";
}

function tomlString(value: string) {
  return JSON.stringify(value);
}

function tomlArray(values: string[]) {
  return `[${values.map(tomlString).join(", ")}]`;
}

function sampleTunnel(): FrpConfigTunnel {
  const preferredPort = config.frpPortStart <= 6000 && config.frpPortEnd >= 6000 ? 6000 : config.frpPortStart;
  return {
    id: "ssh",
    protocol: "tcp",
    localHost: "127.0.0.1",
    localPort: 22,
    remotePort: preferredPort
  } as FrpConfigTunnel;
}

function safeFrpIdentity(value: string) {
  const normalized = value.trim().toLowerCase().replace(/[^a-z0-9_-]+/g, "-").replace(/^-+|-+$/g, "");
  const digest = createHash("sha256").update(value).digest("hex").slice(0, 10);
  return `yanto-${(normalized || "client").slice(0, 42)}-${digest}`;
}

export function frpClientIdentity(nodeId: string) {
  return safeFrpIdentity(nodeId);
}

export function buildFrpcToml(input: {
  serverAddr: string;
  serverPort: number;
  authToken: string;
  tunnels: FrpConfigTunnel[];
  clientId?: string;
  includeSampleWhenEmpty?: boolean;
}) {
  const tunnels = input.tunnels.length ? input.tunnels : [sampleTunnel()];
  const clientId = input.clientId ? safeFrpIdentity(input.clientId) : "yanto-manual-frpc";
  const renderedTunnels = input.tunnels.length || input.includeSampleWhenEmpty !== false ? tunnels : [];
  const lines = [
    `serverAddr = ${tomlString(input.serverAddr || "x.x.x.x")}`,
    `serverPort = ${input.serverPort}`,
    `loginFailExit = false`,
    `clientID = ${tomlString(clientId)}`,
    `user = ${tomlString(clientId)}`,
    "",
    `[auth]`,
    `method = "token"`,
    `token = ${tomlString(input.authToken || "PASTE_FRP_TOKEN_HERE")}`,
    `additionalScopes = ${tomlArray(["HeartBeats", "NewWorkConns"])}`,
    "",
    `[transport]`,
    `wireProtocol = "v2"`,
    "",
    `[transport.tls]`,
    `enable = true`,
    "",
    `[log]`,
    `to = "console"`,
    `level = "info"`,
    `disablePrintColor = true`
  ];
  for (const tunnel of renderedTunnels) {
    lines.push(
      "",
      `[[proxies]]`,
      `name = ${tomlString(tunnel.id)}`,
      `type = ${tomlString(tunnel.protocol)}`,
      `localIP = ${tomlString(tunnel.localHost)}`,
      `localPort = ${tunnel.localPort}`,
      `remotePort = ${tunnel.remotePort}`
    );
  }
  return `${lines.join("\n")}\n`;
}

export function buildNodeFrpcToml(input: {
  nodeId: string;
  serverAddr: string;
  serverPort: number;
  authToken: string;
  tunnels: FrpConfigTunnel[];
}) {
  return buildFrpcToml({ ...input, clientId: input.nodeId, includeSampleWhenEmpty: false });
}

export function buildFrpsToml(input: {
  bindPort: number;
  portStart: number;
  portEnd: number;
  authToken: string;
  bindAddr?: string;
}) {
  return [
    `bindAddr = ${tomlString(input.bindAddr || "0.0.0.0")}`,
    `bindPort = ${input.bindPort}`,
    `transport.tls.force = true`,
    `maxPortsPerClient = 20`,
    `allowPorts = [{ start = ${input.portStart}, end = ${input.portEnd} }]`,
    "",
    `[auth]`,
    `method = "token"`,
    `additionalScopes = ${tomlArray(["HeartBeats", "NewWorkConns"])}`,
    `token = ${tomlString(input.authToken)}`,
    "",
    `[log]`,
    `to = "console"`,
    `level = "info"`,
    `disablePrintColor = true`,
    ""
  ].join("\n");
}

export async function verifyFrpConfigFile(component: "frpc" | "frps", configPath: string) {
  const result = await runCommand(component, ["verify", "-c", configPath], { timeoutMs: 15_000, maxOutputBytes: 64 * 1024 });
  if (result.exitCode !== 0) {
    throw new HttpError(400, `Invalid ${component} configuration: ${result.output.trim() || `verification exited with code ${result.exitCode}`}`);
  }
  return result.output.trim();
}

function buildFrpcInstallScript(frpcToml: string) {
  return `#!/usr/bin/env bash
set -euo pipefail

FRP_VERSION="\${FRP_VERSION:-0.69.0}"
INSTALL_DIR="\${INSTALL_DIR:-/opt/frp}"
CONFIG_PATH="\${CONFIG_PATH:-/etc/frp/frpc.toml}"
SERVICE_NAME="\${SERVICE_NAME:-frpc}"

if [ "$(id -u)" -ne 0 ]; then
  echo "Run this script with sudo."
  exit 1
fi

case "$(uname -m)" in
  x86_64|amd64) ARCH="amd64" ;;
  aarch64|arm64) ARCH="arm64" ;;
  armv7l) ARCH="arm" ;;
  *) echo "Unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

curl -fsSL "https://github.com/fatedier/frp/releases/download/v\${FRP_VERSION}/frp_\${FRP_VERSION}_linux_\${ARCH}.tar.gz" -o "$TMP_DIR/frp.tar.gz"
tar -xzf "$TMP_DIR/frp.tar.gz" -C "$TMP_DIR"
mkdir -p "$INSTALL_DIR" "$(dirname "$CONFIG_PATH")"
install -m 0755 "$TMP_DIR/frp_\${FRP_VERSION}_linux_\${ARCH}/frpc" "$INSTALL_DIR/frpc"
cat > "$CONFIG_PATH" <<'FRPC_TOML'
${frpcToml}FRPC_TOML
chmod 600 "$CONFIG_PATH"

if command -v systemctl >/dev/null 2>&1; then
  cat > "/etc/systemd/system/\${SERVICE_NAME}.service" <<EOF
[Unit]
Description=FRP client
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=$INSTALL_DIR/frpc -c $CONFIG_PATH
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF
  systemctl daemon-reload
  systemctl enable --now "\${SERVICE_NAME}"
  systemctl status "\${SERVICE_NAME}" --no-pager
else
  echo "systemd not found. Run: $INSTALL_DIR/frpc -c $CONFIG_PATH"
fi
`;
}

export async function frpClientSetup(): Promise<FrpClientSetup> {
  const [settings, token, tunnels] = await Promise.all([
    publicFrpSettings(),
    readFrpToken(),
    db.select().from(frpTunnels).where(and(eq(frpTunnels.enabled, true), isNull(frpTunnels.serverId))).orderBy(asc(frpTunnels.createdAt))
  ]);
  const frpcToml = buildFrpcToml({
    serverAddr: settings.publicHost,
    serverPort: settings.bindPort,
    authToken: token,
    tunnels
  });
  return {
    serverAddr: settings.publicHost,
    serverPort: settings.bindPort,
    authToken: token,
    tokenConfigured: Boolean(token),
    tunnelCount: tunnels.length,
    frpcToml,
    installScript: buildFrpcInstallScript(frpcToml)
  };
}

function decryptedServerToken(server: FrpServerRow) {
  return isEncrypted(server.authToken) ? decrypt(server.authToken) : server.authToken;
}

function publicFrpServer(server: FrpServerRow) {
  return {
    id: server.id,
    nodeId: server.nodeId,
    name: server.name,
    publicHost: server.publicHost,
    bindPort: server.bindPort,
    portStart: server.portStart,
    portEnd: server.portEnd,
    configured: Boolean(server.authToken && server.publicHost),
    tokenConfigured: Boolean(server.authToken),
    status: server.status,
    lastError: server.lastError,
    createdAt: server.createdAt.toISOString(),
    updatedAt: server.updatedAt.toISOString()
  };
}

async function ensureDefaultFrpServer() {
  const [existing] = await db.select().from(frpServers).limit(1);
  if (existing) return;
  const [settings, token] = await Promise.all([publicFrpSettings(), readFrpToken()]);
  if (!settings.configured || !token) return;
  await db.insert(frpServers).values({
    id: "frps_default",
    nodeId: config.localNodeId,
    name: "Default VPS FRP server",
    publicHost: settings.publicHost,
    bindPort: settings.bindPort,
    portStart: settings.portStart,
    portEnd: settings.portEnd,
    authToken: encrypt(token),
    status: "online"
  }).onConflictDoNothing();
  await db.insert(frpNodeAssignments).values({
    nodeId: config.localNodeId,
    role: "server",
    status: "online",
    appliedRevision: 1
  }).onConflictDoNothing();
}

export async function listFrpServers() {
  await ensureDefaultFrpServer();
  return (await db.select().from(frpServers).orderBy(asc(frpServers.createdAt))).map(publicFrpServer);
}

export async function createFrpServer(input: FrpServerInput) {
  validatePortRange(input.bindPort, input.portStart, input.portEnd);
  const [node] = await db.select().from(deploymentNodes).where(eq(deploymentNodes.id, input.nodeId)).limit(1);
  if (!node) throw new HttpError(404, "Node not found.");
  const token = input.authToken?.trim() || randomBytes(32).toString("base64url");
  try {
    const [server] = await db.insert(frpServers).values({
      id: createId("frps"),
      nodeId: input.nodeId,
      name: input.name.trim(),
      publicHost: input.publicHost.trim(),
      bindPort: input.bindPort,
      portStart: input.portStart,
      portEnd: input.portEnd,
      authToken: encrypt(token),
      status: "offline"
    }).returning();
    return publicFrpServer(server);
  } catch (error) {
    if (error && typeof error === "object" && "code" in error && error.code === "23505") {
      throw new HttpError(409, "That node already owns an FRP server.");
    }
    throw error;
  }
}

export async function updateFrpServer(id: string, input: Partial<Omit<FrpServerInput, "nodeId">>) {
  const [current] = await db.select().from(frpServers).where(eq(frpServers.id, id)).limit(1);
  if (!current) throw new HttpError(404, "FRP server not found.");
  const bindPort = input.bindPort ?? current.bindPort;
  const portStart = input.portStart ?? current.portStart;
  const portEnd = input.portEnd ?? current.portEnd;
  validatePortRange(bindPort, portStart, portEnd);
  const [server] = await db.update(frpServers).set({
    ...(input.name !== undefined ? { name: input.name.trim() } : {}),
    ...(input.publicHost !== undefined ? { publicHost: input.publicHost.trim() } : {}),
    ...(input.bindPort !== undefined ? { bindPort } : {}),
    ...(input.portStart !== undefined ? { portStart } : {}),
    ...(input.portEnd !== undefined ? { portEnd } : {}),
    ...(input.authToken?.trim() ? { authToken: encrypt(input.authToken.trim()) } : {}),
    status: "offline",
    lastError: null,
    updatedAt: new Date()
  }).where(eq(frpServers.id, id)).returning();
  await db.update(frpNodeAssignments).set({
    desiredRevision: sql`${frpNodeAssignments.desiredRevision} + 1`,
    status: "pending",
    lastError: null,
    updatedAt: new Date()
  }).where(or(eq(frpNodeAssignments.serverId, id), eq(frpNodeAssignments.nodeId, current.nodeId)));
  return publicFrpServer(server);
}

export async function deleteFrpServer(id: string) {
  const [assignment] = await db.select().from(frpNodeAssignments).where(eq(frpNodeAssignments.serverId, id)).limit(1);
  if (assignment) throw new HttpError(409, "Move or disable the FRP clients using this server before deleting it.");
  const [server] = await db.delete(frpServers).where(eq(frpServers.id, id)).returning();
  if (!server) throw new HttpError(404, "FRP server not found.");
  await bumpFrpAssignmentRevision(server.nodeId);
  return publicFrpServer(server);
}

export async function listFrpNodeAssignments() {
  await ensureDefaultFrpServer();
  const nodes = await db.select({ id: deploymentNodes.id }).from(deploymentNodes);
  for (const node of nodes) {
    await db.insert(frpNodeAssignments).values({ nodeId: node.id, role: "disabled", status: "disabled" }).onConflictDoNothing();
  }
  return db.select().from(frpNodeAssignments).orderBy(asc(frpNodeAssignments.nodeId));
}

export async function saveFrpNodeAssignment(nodeId: string, input: { role: FrpRole; serverId?: string | null }) {
  const [node] = await db.select().from(deploymentNodes).where(eq(deploymentNodes.id, nodeId)).limit(1);
  if (!node) throw new HttpError(404, "Node not found.");
  const needsClient = input.role === "client" || input.role === "both";
  const needsServer = input.role === "server" || input.role === "both";
  const selectedServer = needsClient ? await serverForTunnel(input.serverId) : null;
  if (needsClient && !selectedServer) throw new HttpError(400, "FRP client nodes must select an FRP server.");
  if (needsServer) {
    const [ownedServer] = await db.select().from(frpServers).where(eq(frpServers.nodeId, nodeId)).limit(1);
    if (!ownedServer) throw new HttpError(400, "Create an FRP server for this node before enabling the server role.");
  }
  const [assignment] = await db.insert(frpNodeAssignments).values({
    nodeId,
    role: input.role,
    serverId: selectedServer?.id ?? null,
    desiredRevision: 1,
    appliedRevision: 0,
    status: input.role === "disabled" ? "disabled" : "pending",
    lastError: null,
    updatedAt: new Date()
  }).onConflictDoUpdate({
    target: frpNodeAssignments.nodeId,
    set: {
      role: input.role,
      serverId: selectedServer?.id ?? null,
      desiredRevision: sql`${frpNodeAssignments.desiredRevision} + 1`,
      status: input.role === "disabled" ? "disabled" : "pending",
      lastError: null,
      updatedAt: new Date()
    }
  }).returning();
  return assignment;
}

function managedFrpContainerName(component: "frpc" | "frps") {
  return `yanto-${component}`;
}

export async function getFrpDesiredConfig(nodeId: string) {
  await ensureDefaultFrpServer();
  const [assignment] = await db.select().from(frpNodeAssignments).where(eq(frpNodeAssignments.nodeId, nodeId)).limit(1);
  if (!assignment || assignment.role === "disabled") {
    return { nodeId, role: "disabled" as const, revision: assignment?.desiredRevision ?? 0, frpc: null, frps: null };
  }

  const needsClient = assignment.role === "client" || assignment.role === "both";
  const needsServer = assignment.role === "server" || assignment.role === "both";
  let frpc: { containerName: string; configPath: string; configToml: string } | null = null;
  let frps: { containerName: string; configPath: string; configToml: string } | null = null;

  if (needsClient) {
    if (!assignment.serverId) throw new HttpError(409, "FRP client assignment has no server.");
    const server = await serverForTunnel(assignment.serverId);
    if (!server) throw new HttpError(409, "Assigned FRP server no longer exists.");
    const rows = await db.select().from(frpTunnels).where(and(
      eq(frpTunnels.enabled, true),
      or(
        and(eq(frpTunnels.clientNodeId, nodeId), eq(frpTunnels.serverId, server.id)),
        and(isNull(frpTunnels.clientNodeId), eq(frpTunnels.nodeId, nodeId), isNull(frpTunnels.serverId))
      )
    )).orderBy(asc(frpTunnels.createdAt));
    const tunnels: FrpConfigTunnel[] = rows.map((row) => ({
      id: row.id,
      protocol: row.protocol as "tcp" | "udp",
      localHost: row.localHost,
      localPort: row.localPort,
      remotePort: row.remotePort
    }));
    frpc = {
      containerName: managedFrpContainerName("frpc"),
      configPath: "/data/frp/frpc.toml",
      configToml: buildNodeFrpcToml({
        nodeId,
        serverAddr: server.publicHost,
        serverPort: server.bindPort,
        authToken: decryptedServerToken(server),
        tunnels
      })
    };
  }

  if (needsServer) {
    const [server] = await db.select().from(frpServers).where(eq(frpServers.nodeId, nodeId)).limit(1);
    if (!server) throw new HttpError(409, "FRP server assignment has no server configuration.");
    frps = {
      containerName: managedFrpContainerName("frps"),
      configPath: "/data/frp/frps.toml",
      configToml: buildFrpsToml({
        bindPort: server.bindPort,
        portStart: server.portStart,
        portEnd: server.portEnd,
        authToken: decryptedServerToken(server)
      })
    };
  }

  return { nodeId, role: assignment.role as FrpRole, revision: assignment.desiredRevision, frpc, frps };
}

export async function frpNodeClientSetup(nodeId: string): Promise<FrpClientSetup> {
  const desired = await getFrpDesiredConfig(nodeId);
  if (!desired.frpc) throw new HttpError(400, "This node is not configured as an FRP client.");
  const [assignment] = await db.select().from(frpNodeAssignments).where(eq(frpNodeAssignments.nodeId, nodeId)).limit(1);
  if (!assignment?.serverId) throw new HttpError(409, "FRP client assignment has no server.");
  const server = await serverForTunnel(assignment.serverId);
  if (!server) throw new HttpError(409, "Assigned FRP server no longer exists.");
  const authToken = decryptedServerToken(server);
  const frpcToml = desired.frpc.configToml;
  return {
    serverAddr: server.publicHost,
    serverPort: server.bindPort,
    authToken,
    tokenConfigured: Boolean(authToken),
    tunnelCount: (frpcToml.match(/^\[\[proxies\]\]$/gm) ?? []).length,
    frpcToml,
    installScript: buildFrpcInstallScript(frpcToml)
  };
}

export async function reportFrpNodeStatus(nodeId: string, input: FrpNodeStatusInput) {
  const [current] = await db.select().from(frpNodeAssignments).where(eq(frpNodeAssignments.nodeId, nodeId)).limit(1);
  if (!current && input.revision === 0 && input.status === "disabled") return { accepted: true, assignment: null };
  if (!current) throw new HttpError(404, "FRP assignment not found.");
  if (input.revision !== current.desiredRevision) return { accepted: false, assignment: current };
  const applied = input.status === "online" || input.status === "disabled";
  const [assignment] = await db.update(frpNodeAssignments).set({
    ...(applied ? { appliedRevision: input.revision } : {}),
    status: input.status,
    lastError: input.status === "error" ? input.error?.trim() || "FRP reconciliation failed." : null,
    updatedAt: new Date()
  }).where(and(eq(frpNodeAssignments.nodeId, nodeId), eq(frpNodeAssignments.desiredRevision, input.revision))).returning();
  if (!assignment) return { accepted: false, assignment: current };
  if (current.role === "server" || current.role === "both") {
    await db.update(frpServers).set({
      status: input.status === "online" ? "online" : input.status === "error" ? "error" : "offline",
      lastError: input.status === "error" ? input.error?.trim() || "FRP reconciliation failed." : null,
      updatedAt: new Date()
    }).where(eq(frpServers.nodeId, nodeId));
  }
  return { accepted: true, assignment };
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
  return {
    server,
    clients: clientRows,
    proxies: [
      ...tcpRows.map((proxy) => ({ ...proxy, proxyType: "tcp" as const })),
      ...udpRows.map((proxy) => ({ ...proxy, proxyType: "udp" as const }))
    ]
  };
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
  await ensureDefaultFrpServer();
  const [settings, tunnelRows, nodeRows, serverRows, assignmentRows, runtime] = await Promise.all([
    publicFrpSettings(),
    db.select().from(frpTunnels).orderBy(asc(frpTunnels.createdAt)),
    db.select({ id: deploymentNodes.id, name: deploymentNodes.name }).from(deploymentNodes),
    db.select().from(frpServers).orderBy(asc(frpServers.createdAt)),
    db.select().from(frpNodeAssignments).orderBy(asc(frpNodeAssignments.nodeId)),
    frpServerStatus()
  ]);

  const proxies = runtime.dashboard?.proxies ?? [];
  const nodeNames = new Map(nodeRows.map((node) => [node.id, node.name]));
  const serverNames = new Map(serverRows.map((server) => [server.id, server.name]));
  const localServerIds = new Set(serverRows.filter((server) => server.nodeId === config.localNodeId).map((server) => server.id));

  const tunnels: FrpTunnel[] = tunnelRows.map((tunnel) => {
    const proxy = findFrpTunnelProxy(tunnel, tunnel.serverId && !localServerIds.has(tunnel.serverId) ? [] : proxies);
    return {
      id: tunnel.id,
      nodeId: tunnel.nodeId,
      clientNodeId: tunnel.clientNodeId,
      serverId: tunnel.serverId,
      name: tunnel.name,
      protocol: tunnel.protocol as "tcp" | "udp",
      localHost: tunnel.localHost,
      localPort: tunnel.localPort,
      remotePort: tunnel.remotePort,
      enabled: tunnel.enabled,
      nodeName: tunnel.nodeId ? nodeNames.get(tunnel.nodeId) ?? null : null,
      clientNodeName: tunnel.clientNodeId ? nodeNames.get(tunnel.clientNodeId) ?? null : null,
      serverName: tunnel.serverId ? serverNames.get(tunnel.serverId) ?? null : null,
      syncStatus: tunnelStatus(tunnel, proxy),
      lastError: tunnel.lastError,
      lastSyncedAt: tunnel.lastSyncedAt?.toISOString() ?? null,
      trafficInBytes: numberValue(proxy?.todayTrafficIn ?? proxy?.trafficIn ?? proxy?.traffic_in),
      trafficOutBytes: numberValue(proxy?.todayTrafficOut ?? proxy?.trafficOut ?? proxy?.traffic_out),
      currentConnections: numberValue(proxy?.curConns ?? proxy?.cur_conns),
      createdAt: tunnel.createdAt.toISOString(),
      updatedAt: tunnel.updatedAt.toISOString()
    };
  });

  const servers = serverRows.map((server) => ({ ...publicFrpServer(server), nodeName: nodeNames.get(server.nodeId) ?? null }));
  const assignments = assignmentRows.map((assignment) => ({
    ...assignment,
    role: assignment.role as FrpRole,
    nodeName: nodeNames.get(assignment.nodeId) ?? null,
    updatedAt: assignment.updatedAt.toISOString()
  }));

  return { settings, server: runtime.status, tunnels, servers, assignments };
}

export async function controlFrpServer(action: "start" | "stop" | "restart") {
  const result = await runCommand("docker", [action, config.frpContainerName], { timeoutMs: 30_000 });
  if (result.exitCode !== 0) throw new HttpError(503, `Unable to ${action} FRP server container.`);
  return frpServerStatus().then((result) => result.status);
}
