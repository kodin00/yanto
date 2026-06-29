import { Activity, Copy, Edit3, Gamepad2, Network, Play, Plus, Power, RotateCw, Server, Trash2 } from "lucide-react";
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { FormEvent } from "react";
import type { FrpOverview, FrpProtocol, FrpTunnel } from "../../shared/types";
import { Button, CustomSelect, IconButton, StatusBadge, TextField, ToggleField } from "../components/ui";
import { api, type FrpTunnelPayload } from "../lib/api";
import type { ConfirmState } from "./types";

type Props = {
  refreshKey: number;
  copyText: (value: string) => Promise<void>;
  toast: (message: string, kind?: "ok" | "error" | "loading") => void;
  setConfirm: (state: ConfirmState) => void;
};

type TunnelForm = {
  id: string | null;
  name: string;
  nodeId: string;
  protocol: FrpProtocol;
  localHost: string;
  localPort: string;
  remotePort: string;
  enabled: boolean;
};

const emptyForm: TunnelForm = {
  id: null,
  name: "",
  nodeId: "",
  protocol: "tcp",
  localHost: "host.docker.internal",
  localPort: "25565",
  remotePort: "25565",
  enabled: true
};

function bytes(value: number) {
  if (!value) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const index = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1);
  return `${(value / 1024 ** index).toFixed(index ? 1 : 0)} ${units[index]}`;
}

function duration(seconds: number | null) {
  if (!seconds) return "-";
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  return days ? `${days}d ${hours}h` : `${hours}h ${Math.floor((seconds % 3600) / 60)}m`;
}

function formFor(tunnel: FrpTunnel): TunnelForm {
  return {
    id: tunnel.id,
    name: tunnel.name,
    nodeId: tunnel.nodeId,
    protocol: tunnel.protocol,
    localHost: tunnel.localHost,
    localPort: String(tunnel.localPort),
    remotePort: String(tunnel.remotePort),
    enabled: tunnel.enabled
  };
}

export const FrpView = memo(function FrpView({ refreshKey, copyText, toast, setConfirm }: Props) {
  const [overview, setOverview] = useState<FrpOverview | null>(null);
  const [publicHost, setPublicHost] = useState("");
  const [form, setForm] = useState<TunnelForm>(emptyForm);
  const [busy, setBusy] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const endpointDirty = useRef(false);

  const refresh = useCallback(async (quiet = false) => {
    if (!quiet) setLoading(true);
    try {
      const next = await api.frpOverview();
      setOverview(next);
      if (!endpointDirty.current) setPublicHost(next.settings.publicHost);
      setForm((current) => current.nodeId || !next.clients.length ? current : { ...current, nodeId: next.clients[0].nodeId });
    } catch (error) {
      if (!quiet) toast(error instanceof Error ? error.message : "Unable to load FRP.", "error");
    } finally {
      if (!quiet) setLoading(false);
    }
  }, [toast]);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(true), 10_000);
    return () => window.clearInterval(timer);
  }, [refresh, refreshKey]);

  const nodeOptions = useMemo(() => (overview?.clients ?? []).map((client) => ({ value: client.nodeId, label: client.nodeName })), [overview?.clients]);
  const protocolOptions = useMemo(() => [{ value: "tcp" as const, label: "TCP" }, { value: "udp" as const, label: "UDP" }], []);

  function nextPort(protocol: FrpProtocol) {
    const settings = overview?.settings;
    if (!settings) return 25565;
    const used = new Set(overview?.tunnels.filter((tunnel) => tunnel.protocol === protocol).map((tunnel) => tunnel.remotePort));
    for (let port = settings.portStart; port <= settings.portEnd; port++) {
      if (!used.has(port)) return port;
    }
    return settings.portStart;
  }

  function applyPreset(kind: "java" | "bedrock") {
    const protocol = kind === "java" ? "tcp" : "udp";
    const preferred = kind === "java" && overview && overview.settings.portStart <= 25565 && overview.settings.portEnd >= 25565 ? 25565 : nextPort(protocol);
    setForm((current) => ({
      ...emptyForm,
      nodeId: current.nodeId || overview?.clients[0]?.nodeId || "",
      name: kind === "java" ? "Minecraft Java" : "Minecraft Bedrock",
      protocol,
      localPort: kind === "java" ? "25565" : "19132",
      remotePort: String(preferred)
    }));
  }

  async function saveSettings(event: FormEvent) {
    event.preventDefault();
    setBusy("settings");
    try {
      await api.saveFrpSettings(publicHost.trim());
      endpointDirty.current = false;
      toast("FRP public endpoint saved.");
      await refresh(true);
    } catch (error) {
      toast(error instanceof Error ? error.message : "Unable to save FRP settings.", "error");
    } finally {
      setBusy(null);
    }
  }

  async function control(action: "start" | "stop" | "restart") {
    setBusy(`server-${action}`);
    toast(`${action[0].toUpperCase() + action.slice(1)}ing FRP server...`, "loading");
    try {
      await api.controlFrpServer(action);
      toast(`FRP server ${action === "stop" ? "stopped" : "started"}.`);
      await refresh(true);
    } catch (error) {
      toast(error instanceof Error ? error.message : `Unable to ${action} FRP server.`, "error");
    } finally {
      setBusy(null);
    }
  }

  async function saveTunnel(event: FormEvent) {
    event.preventDefault();
    const payload: FrpTunnelPayload = {
      name: form.name.trim(),
      nodeId: form.nodeId,
      protocol: form.protocol,
      localHost: form.localHost.trim(),
      localPort: Number(form.localPort),
      remotePort: Number(form.remotePort),
      enabled: form.enabled
    };
    setBusy("tunnel-save");
    try {
      if (form.id) await api.updateFrpTunnel(form.id, payload);
      else await api.createFrpTunnel(payload);
      toast(form.id ? "FRP tunnel updated." : "FRP tunnel created.");
      setForm({ ...emptyForm, nodeId: overview?.clients[0]?.nodeId ?? "" });
      await refresh(true);
    } catch (error) {
      toast(error instanceof Error ? error.message : "Unable to save FRP tunnel.", "error");
    } finally {
      setBusy(null);
    }
  }

  async function toggleTunnel(tunnel: FrpTunnel) {
    setBusy(`tunnel-toggle:${tunnel.id}`);
    try {
      await api.updateFrpTunnel(tunnel.id, { enabled: !tunnel.enabled });
      await refresh(true);
      toast(`Tunnel ${tunnel.enabled ? "disabled" : "enabled"}.`);
    } catch (error) {
      toast(error instanceof Error ? error.message : "Unable to update tunnel.", "error");
    } finally {
      setBusy(null);
    }
  }

  async function copyWorkerCommand() {
    setBusy("worker-command");
    try {
      const result = await api.workerJoinToken();
      await copyText(result.command);
      toast("Worker install command copied.");
    } catch (error) {
      toast(error instanceof Error ? error.message : "Unable to create worker command.", "error");
    } finally {
      setBusy(null);
    }
  }

  const settings = overview?.settings;
  const server = overview?.server;
  const canSaveTunnel = Boolean(settings?.configured && form.name.trim() && form.nodeId && form.localHost.trim() && Number(form.localPort) && Number(form.remotePort));

  return (
    <div className="frp-layout">
      <section className="panel frp-server-panel">
        <div className="panel-head">
          <h2>FRP server</h2>
          <StatusBadge status={server?.running ? "online" : "offline"} label={server?.running ? "Running" : "Stopped"} />
        </div>
        <div className="frp-stat-grid">
          <div><span>Version</span><strong>{server?.version ?? "-"}</strong></div>
          <div><span>Uptime</span><strong>{duration(server?.uptimeSeconds ?? null)}</strong></div>
          <div><span>Received</span><strong>{bytes(server?.trafficInBytes ?? 0)}</strong></div>
          <div><span>Sent</span><strong>{bytes(server?.trafficOutBytes ?? 0)}</strong></div>
        </div>
        {server?.error ? <p className="form-error">{server.error}</p> : null}
        <div className="actions">
          <Button variant="secondary" disabled={Boolean(busy)} onClick={() => void control("start")} icon={<Play size={15} />}>Start</Button>
          <Button variant="secondary" disabled={Boolean(busy)} onClick={() => void control("restart")} icon={<RotateCw size={15} />}>Restart</Button>
          <Button variant="danger" disabled={Boolean(busy)} onClick={() => void control("stop")} icon={<Power size={15} />}>Stop</Button>
        </div>
      </section>

      <section className="panel frp-settings-panel">
        <div className="panel-head"><h2>Public endpoint</h2><Network size={19} /></div>
        <form className="form-grid" onSubmit={saveSettings}>
          <TextField label="VPS IP or DNS-only hostname" value={publicHost} onChange={(value) => { setPublicHost(value); endpointDirty.current = true; }} placeholder="203.0.113.10" />
          <p className="muted">Do not use a Cloudflare-proxied hostname. FRPC connects to port {settings?.bindPort ?? 7000}.</p>
          <div className="actions"><Button type="submit" loading={busy === "settings"}>Save endpoint</Button></div>
        </form>
      </section>

      <section className="panel frp-clients-panel">
        <div className="panel-head"><h2>Home clients</h2><span className="count">{overview?.clients.length ?? 0} enrolled</span></div>
        <div className="frp-client-grid">
          {overview?.clients.map((client) => (
            <div className="frp-client-card" key={client.nodeId}>
              <div><Server size={17} /><strong>{client.nodeName}</strong></div>
              <div className="frp-client-statuses"><StatusBadge status={client.workerStatus} label={`Worker ${client.workerStatus}`} /><StatusBadge status={client.frpcStatus} label={`FRPC ${client.frpcStatus}`} /></div>
              <span>{client.frpcVersion ?? "FRPC not reported"}{client.protocol ? ` · wire ${client.protocol}` : ""}</span>
              {client.lastError ? <p className="form-error">{client.lastError}</p> : null}
            </div>
          ))}
        </div>
        {!overview?.clients.length ? (
          <div className="frp-empty-state">
            <p className="muted">Enroll the home server with Yanto’s outbound-only worker. No inbound home port is opened.</p>
            <Button onClick={() => void copyWorkerCommand()} loading={busy === "worker-command"} icon={<Copy size={15} />}>Copy worker install command</Button>
          </div>
        ) : null}
      </section>

      <section className="panel frp-editor-panel">
        <div className="panel-head"><h2>{form.id ? "Edit tunnel" : "Add tunnel"}</h2><Gamepad2 size={19} /></div>
        <div className="frp-presets">
          <Button variant="secondary" onClick={() => applyPreset("java")}>Minecraft Java</Button>
          <Button variant="secondary" onClick={() => applyPreset("bedrock")}>Minecraft Bedrock</Button>
        </div>
        <form className="form-grid" onSubmit={saveTunnel}>
          <div className="frp-form-grid">
            <TextField label="Name" value={form.name} onChange={(name) => setForm((current) => ({ ...current, name }))} placeholder="Minecraft Java" />
            {nodeOptions.length ? <CustomSelect label="Home server" value={form.nodeId} options={nodeOptions} onChange={(nodeId) => setForm((current) => ({ ...current, nodeId }))} /> : <TextField label="Home server" value="Enroll a worker first" onChange={() => undefined} disabled />}
            <CustomSelect label="Protocol" value={form.protocol} options={protocolOptions} onChange={(protocol) => setForm((current) => ({ ...current, protocol }))} />
            <TextField label="Local host" value={form.localHost} onChange={(localHost) => setForm((current) => ({ ...current, localHost }))} />
            <TextField label="Local port" value={form.localPort} onChange={(localPort) => setForm((current) => ({ ...current, localPort: localPort.replace(/\D/g, "") }))} />
            <TextField label="VPS public port" value={form.remotePort} onChange={(remotePort) => setForm((current) => ({ ...current, remotePort: remotePort.replace(/\D/g, "") }))} />
          </div>
          <p className="muted">The default local host reaches a native or Docker-published service on the home server. The service must listen on an interface reachable from Docker.</p>
          <ToggleField label="Enabled" value={form.enabled} onChange={(enabled) => setForm((current) => ({ ...current, enabled }))} description={`Allowed public range: ${settings?.portStart ?? 25560}–${settings?.portEnd ?? 25600}`} />
          <div className="actions">
            {form.id ? <Button variant="ghost" onClick={() => setForm({ ...emptyForm, nodeId: overview?.clients[0]?.nodeId ?? "" })}>Cancel</Button> : null}
            <Button type="submit" disabled={!canSaveTunnel} loading={busy === "tunnel-save"} icon={<Plus size={15} />}>{form.id ? "Save tunnel" : "Create tunnel"}</Button>
          </div>
        </form>
      </section>

      <section className="panel frp-tunnels-panel">
        <div className="panel-head"><h2>Port forwards</h2><span className="count">{loading ? "Loading" : `${overview?.tunnels.length ?? 0} rules`}</span></div>
        <div className="table-wrap">
          <table>
            <thead><tr><th>Name</th><th>Client</th><th>Mapping</th><th>Status</th><th>Traffic</th><th className="action-cell">Actions</th></tr></thead>
            <tbody>
              {overview?.tunnels.map((tunnel) => (
                <tr key={tunnel.id}>
                  <td><strong>{tunnel.name}</strong><br /><span className="muted">{tunnel.protocol.toUpperCase()}</span></td>
                  <td>{tunnel.nodeName ?? tunnel.nodeId}</td>
                  <td className="frp-mapping"><span>{tunnel.localHost}:{tunnel.localPort}</span><strong>→ {settings?.publicHost || "VPS"}:{tunnel.remotePort}</strong></td>
                  <td><StatusBadge status={tunnel.syncStatus} /></td>
                  <td>{bytes(tunnel.trafficInBytes)} ↓ / {bytes(tunnel.trafficOutBytes)} ↑<br /><span className="muted">{tunnel.currentConnections} active</span></td>
                  <td className="action-cell">
                    <div className="table-actions icon-actions">
                      <IconButton label="Copy endpoint" onClick={() => void copyText(`${settings?.publicHost || "VPS"}:${tunnel.remotePort}`)}><Copy size={14} /></IconButton>
                      <IconButton label="Edit tunnel" onClick={() => setForm(formFor(tunnel))}><Edit3 size={14} /></IconButton>
                      <IconButton label={tunnel.enabled ? "Disable tunnel" : "Enable tunnel"} disabled={busy === `tunnel-toggle:${tunnel.id}`} onClick={() => void toggleTunnel(tunnel)}>{tunnel.enabled ? <Power size={14} /> : <Play size={14} />}</IconButton>
                      <IconButton label="Delete tunnel" variant="danger" onClick={() => setConfirm({
                        title: "Delete FRP tunnel",
                        body: `Delete ${tunnel.name}? The public port will stop forwarding after the worker's next poll.`,
                        label: "Delete",
                        danger: true,
                        loadingMessage: "Deleting FRP tunnel...",
                        successMessage: "FRP tunnel deleted.",
                        action: async () => { await api.deleteFrpTunnel(tunnel.id); await refresh(true); }
                      })}><Trash2 size={14} /></IconButton>
                    </div>
                  </td>
                </tr>
              ))}
              {!overview?.tunnels.length ? <tr><td colSpan={6}>{loading ? "Loading FRP tunnels..." : "No port forwards configured."}</td></tr> : null}
            </tbody>
          </table>
        </div>
      </section>

      <section className="panel frp-firewall-panel">
        <div className="panel-head"><h2>VPS firewall</h2><Activity size={19} /></div>
        <p className="muted">Yanto does not change firewall rules. Open these ports on the VPS:</p>
        <pre>{`sudo ufw allow ${settings?.bindPort ?? 7000}/tcp\nsudo ufw allow ${settings?.portStart ?? 25560}:${settings?.portEnd ?? 25600}/tcp\nsudo ufw allow ${settings?.portStart ?? 25560}:${settings?.portEnd ?? 25600}/udp`}</pre>
        <div className="actions"><Button variant="secondary" onClick={() => void copyText(`sudo ufw allow ${settings?.bindPort ?? 7000}/tcp\nsudo ufw allow ${settings?.portStart ?? 25560}:${settings?.portEnd ?? 25600}/tcp\nsudo ufw allow ${settings?.portStart ?? 25560}:${settings?.portEnd ?? 25600}/udp`)} icon={<Copy size={15} />}>Copy commands</Button></div>
      </section>
    </div>
  );
});
