import { Activity, Copy, Edit3, KeyRound, Network, Play, Plus, Power, RotateCw, Trash2, Waypoints } from "lucide-react";
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
  protocol: FrpProtocol;
  localHost: string;
  localPort: string;
  remotePort: string;
  enabled: boolean;
};

const emptyForm: TunnelForm = {
  id: null,
  name: "",
  protocol: "tcp",
  localHost: "127.0.0.1",
  localPort: "",
  remotePort: "",
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

  const protocolOptions = useMemo(() => [{ value: "tcp" as const, label: "TCP" }, { value: "udp" as const, label: "UDP" }], []);

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
      setForm(emptyForm);
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

  async function copyClientValue(kind: "token" | "config") {
    setBusy(`client-${kind}`);
    try {
      const setup = await api.frpClientSetup();
      if (kind === "token" && !setup.authToken) throw new Error("FRP client token is not configured.");
      await copyText(kind === "token" ? setup.authToken : setup.frpcToml);
      toast(kind === "token" ? "FRP client token copied." : "frpc.toml copied.");
    } catch (error) {
      toast(error instanceof Error ? error.message : "Unable to copy FRP client configuration.", "error");
    } finally {
      setBusy(null);
    }
  }

  const settings = overview?.settings;
  const server = overview?.server;
  const canSaveTunnel = Boolean(settings?.configured && form.name.trim() && form.localHost.trim() && Number(form.localPort) && Number(form.remotePort));

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
          <div className="actions">
            <Button type="submit" loading={busy === "settings"}>Save endpoint</Button>
            <Button variant="secondary" onClick={() => void copyClientValue("token")} loading={busy === "client-token"} icon={<KeyRound size={15} />}>Copy client token</Button>
            <Button variant="secondary" onClick={() => void copyClientValue("config")} loading={busy === "client-config"} icon={<Copy size={15} />}>Copy frpc.toml</Button>
          </div>
        </form>
      </section>

      <section className="panel frp-editor-panel">
        <div className="panel-head"><h2>{form.id ? "Edit tunnel" : "Add tunnel"}</h2><Waypoints size={19} /></div>
        <form className="form-grid" onSubmit={saveTunnel}>
          <div className="frp-form-grid">
            <TextField label="Name" value={form.name} onChange={(name) => setForm((current) => ({ ...current, name }))} placeholder="Web server" />
            <CustomSelect label="Protocol" value={form.protocol} options={protocolOptions} onChange={(protocol) => setForm((current) => ({ ...current, protocol }))} />
            <TextField label="Local host" value={form.localHost} onChange={(localHost) => setForm((current) => ({ ...current, localHost }))} />
            <TextField label="Local port" value={form.localPort} onChange={(localPort) => setForm((current) => ({ ...current, localPort: localPort.replace(/\D/g, "") }))} placeholder="3000" />
            <TextField label="VPS public port" value={form.remotePort} onChange={(remotePort) => setForm((current) => ({ ...current, remotePort: remotePort.replace(/\D/g, "") }))} placeholder={String(settings?.portStart ?? 25560)} />
          </div>
          <p className="muted">Use the address as seen by `frpc` on the client server. For a service running on the same machine, `127.0.0.1` is usually right.</p>
          <ToggleField label="Enabled" value={form.enabled} onChange={(enabled) => setForm((current) => ({ ...current, enabled }))} description={`Allowed public range: ${settings?.portStart ?? 25560}–${settings?.portEnd ?? 25600}`} />
          <div className="actions">
            {form.id ? <Button variant="ghost" onClick={() => setForm(emptyForm)}>Cancel</Button> : null}
            <Button type="submit" disabled={!canSaveTunnel} loading={busy === "tunnel-save"} icon={<Plus size={15} />}>{form.id ? "Save tunnel" : "Create tunnel"}</Button>
          </div>
        </form>
      </section>

      <section className="panel frp-tunnels-panel">
        <div className="panel-head"><h2>Port forwards</h2><span className="count">{loading ? "Loading" : `${overview?.tunnels.length ?? 0} rules`}</span></div>
        <div className="table-wrap">
          <table>
            <thead><tr><th>Name</th><th>Mapping</th><th>Status</th><th>Traffic</th><th className="action-cell">Actions</th></tr></thead>
            <tbody>
              {overview?.tunnels.map((tunnel) => (
                <tr key={tunnel.id}>
                  <td><strong>{tunnel.name}</strong><br /><span className="muted">{tunnel.protocol.toUpperCase()}</span></td>
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
                        body: `Delete ${tunnel.name}? Copy and restart the client setup again so frpc stops using that proxy.`,
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
              {!overview?.tunnels.length ? <tr><td colSpan={5}>{loading ? "Loading FRP tunnels..." : "No port forwards configured."}</td></tr> : null}
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
