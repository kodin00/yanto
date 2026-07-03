import fs from "node:fs/promises";
import { asc, eq } from "drizzle-orm";
import type { FrpClientSetup, FrpOverview, FrpServerStatus, FrpTunnel, FrpTunnelStatus } from "../../shared/types.js";
import { config } from "../config.js";
import { db } from "../db/index.js";
import { appSettings, deploymentNodes, frpTunnels, type FrpTunnelRow } from "../db/schema.js";
import { runCommand } from "./commands.js";
import { createId } from "./tokens.js";
import { HttpError } from "../http-utils.js";

const publicHostKey = "frp.public_host";

export type FrpTunnelInput = {
  name: string;
  nodeId?: string | null;
  protocol: "tcp" | "udp";
  localHost: string;
  localPort: number;
  remotePort: number;
  enabled: boolean;
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

type ManualFrpcTunnel = Pick<FrpTunnelRow, "id" | "protocol" | "localHost" | "localPort" | "remotePort">;

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

function rethrowTunnelWriteError(error: unknown): never {
  if (error && typeof error === "object" && "code" in error && error.code === "23505") {
    throw new HttpError(409, "That public port is already used by another tunnel with the same protocol.");
  }
  throw error;
}

export async function createFrpTunnel(input: FrpTunnelInput) {
  validateFrpRemotePort(input.remotePort);
  if (input.enabled && !(await publicFrpSettings()).configured) throw new HttpError(400, "Save the VPS public endpoint before enabling a tunnel.");
  try {
    const [row] = await db.insert(frpTunnels).values({
      id: createId("frp"),
      ...input,
      nodeId: input.nodeId ?? null,
      syncStatus: input.enabled ? "offline" : "disabled"
    }).returning();
    return row;
  } catch (error) {
    rethrowTunnelWriteError(error);
  }
}

export async function updateFrpTunnel(id: string, input: Partial<FrpTunnelInput>) {
  const [current] = await db.select().from(frpTunnels).where(eq(frpTunnels.id, id)).limit(1);
  if (!current) throw new HttpError(404, "FRP tunnel not found.");
  if (input.remotePort !== undefined) validateFrpRemotePort(input.remotePort);
  const enabled = input.enabled ?? current.enabled;
  if (enabled && !(await publicFrpSettings()).configured) throw new HttpError(400, "Save the VPS public endpoint before enabling a tunnel.");
  try {
    const [row] = await db.update(frpTunnels).set({
      ...input,
      syncStatus: enabled ? "offline" : "disabled",
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

function sampleTunnel(): ManualFrpcTunnel {
  const preferredPort = config.frpPortStart <= 6000 && config.frpPortEnd >= 6000 ? 6000 : config.frpPortStart;
  return {
    id: "ssh",
    protocol: "tcp",
    localHost: "127.0.0.1",
    localPort: 22,
    remotePort: preferredPort
  } as ManualFrpcTunnel;
}

export function buildFrpcToml(input: { serverAddr: string; serverPort: number; authToken: string; tunnels: ManualFrpcTunnel[] }) {
  const tunnels = input.tunnels.length ? input.tunnels : [sampleTunnel()];
  const lines = [
    `serverAddr = ${tomlString(input.serverAddr || "x.x.x.x")}`,
    `serverPort = ${input.serverPort}`,
    `loginFailExit = false`,
    `clientID = "yanto-manual-frpc"`,
    `user = "yanto-manual-frpc"`,
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
  for (const tunnel of tunnels) {
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
    db.select().from(frpTunnels).where(eq(frpTunnels.enabled, true)).orderBy(asc(frpTunnels.createdAt))
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
  const [settings, tunnelRows, runtime] = await Promise.all([
    publicFrpSettings(),
    db.select({ tunnel: frpTunnels, nodeName: deploymentNodes.name }).from(frpTunnels).leftJoin(deploymentNodes, eq(frpTunnels.nodeId, deploymentNodes.id)).orderBy(asc(frpTunnels.createdAt)),
    frpServerStatus()
  ]);

  const proxies = runtime.dashboard?.proxies ?? [];

  const tunnels: FrpTunnel[] = tunnelRows.map(({ tunnel, nodeName }) => {
    const proxy = findFrpTunnelProxy(tunnel, proxies);
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

  return { settings, server: runtime.status, tunnels };
}

export async function controlFrpServer(action: "start" | "stop" | "restart") {
  const result = await runCommand("docker", [action, config.frpContainerName], { timeoutMs: 30_000 });
  if (result.exitCode !== 0) throw new HttpError(503, `Unable to ${action} FRP server container.`);
  return frpServerStatus().then((result) => result.status);
}
