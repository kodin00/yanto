import { Activity, Copy, Edit3, KeyRound, Network, Play, Plus, Power, RotateCw, Trash2, Waypoints } from "lucide-react";
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { FormEvent } from "react";
import type { FrpProtocol, FrpTunnel } from "../../shared/types";
import { Button, CustomSelect, IconButton, StatusBadge, TextField, ToggleField } from "../components/ui";
import { api, type FrpRole, type FrpTunnelPayload, type MultiNodeFrpOverview } from "../lib/api";
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
  clientNodeId: string;
  serverId: string;
};

const emptyServerForm = { nodeId: "", name: "", publicHost: "", bindPort: "7000", portStart: "25560", portEnd: "25600", authToken: "" };

const emptyForm: TunnelForm = {
  id: null,
  name: "",
  protocol: "tcp",
  localHost: "127.0.0.1",
  localPort: "",
  remotePort: "",
  enabled: true,
  clientNodeId: "",
  serverId: ""
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
    enabled: tunnel.enabled,
    clientNodeId: tunnel.clientNodeId ?? tunnel.nodeId ?? "",
    serverId: tunnel.serverId ?? ""
  };
}

export const FrpView = memo(function FrpView({ refreshKey, copyText, toast, setConfirm }: Props) {
  const [overview, setOverview] = useState<MultiNodeFrpOverview | null>(null);
  const [publicHost, setPublicHost] = useState("");
  const [form, setForm] = useState<TunnelForm>(emptyForm);
  const [busy, setBusy] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [assignmentDrafts, setAssignmentDrafts] = useState<Record<string, { role: FrpRole; serverId: string }>>({});
  const [serverForm, setServerForm] = useState(emptyServerForm);
  const endpointDirty = useRef(false);

  const refresh = useCallback(async (quiet = false) => {
    if (!quiet) setLoading(true);
    try {
      const [base, servers, assignments] = await Promise.all([api.frpOverview(), api.frpServers(), api.frpNodeAssignments()]);
      const next = { ...base, servers, assignments };
      setOverview(next);
      setAssignmentDrafts((current) => Object.fromEntries(assignments.map((assignment) => [assignment.nodeId, current[assignment.nodeId] ?? { role: assignment.role, serverId: assignment.serverId ?? "" }])));
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
      enabled: form.enabled,
      clientNodeId: form.clientNodeId || null,
      serverId: form.serverId || null
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

  async function copyClientValue(kind: "token" | "config", nodeId?: string) {
    setBusy(`client-${kind}:${nodeId ?? "manual"}`);
    try {
      const setup = await api.frpClientSetup(nodeId);
      if (kind === "token" && !setup.authToken) throw new Error("FRP client token is not configured.");
      await copyText(kind === "token" ? setup.authToken : setup.frpcToml);
      toast(kind === "token" ? "FRP client token copied." : "frpc.toml copied.");
    } catch (error) {
      toast(error instanceof Error ? error.message : "Unable to copy FRP client configuration.", "error");
    } finally {
      setBusy(null);
    }
  }

  async function saveAssignment(nodeId: string) {
    const draft = assignmentDrafts[nodeId];
    if (!draft) return;
    setBusy(`assignment:${nodeId}`);
    try {
      await api.updateFrpNodeAssignment(nodeId, { role: draft.role, serverId: draft.serverId || null });
      await refresh(true);
      toast("FRP node role updated.");
    } catch (error) {
      toast(error instanceof Error ? error.message : "Unable to update FRP node role.", "error");
    } finally {
      setBusy(null);
    }
  }

  async function createServer(event: FormEvent) {
    event.preventDefault();
    setBusy("server-create");
    try {
      await api.createFrpServer({
        nodeId: serverForm.nodeId,
        name: serverForm.name.trim(),
        publicHost: serverForm.publicHost.trim(),
        bindPort: Number(serverForm.bindPort),
        portStart: Number(serverForm.portStart),
        portEnd: Number(serverForm.portEnd),
        ...(serverForm.authToken.trim() ? { authToken: serverForm.authToken.trim() } : {})
      });
      setServerForm(emptyServerForm);
      await refresh(true);
      toast("FRP server created. You can now assign that node the server role.");
    } catch (error) {
      toast(error instanceof Error ? error.message : "Unable to create FRP server.", "error");
    } finally {
      setBusy(null);
    }
  }

  const settings = overview?.settings;
  const server = overview?.server;
  const frpServers = overview?.servers ?? [];
  const assignments = overview?.assignments ?? [];
  const availableServerNodes = assignments.filter((assignment) => !frpServers.some((candidate) => candidate.nodeId === assignment.nodeId));
  const clientAssignments = assignments.filter((assignment) => assignment.role === "client" || assignment.role === "both");
  const clientOptions = [{ value: "", label: "Manual client" }, ...clientAssignments.map((assignment) => ({ value: assignment.nodeId, label: assignment.nodeName ?? assignment.nodeId }))];
  const serverOptions = [{ value: "", label: "Select server" }, ...frpServers.map((candidate) => ({ value: candidate.id, label: `${candidate.nodeName ?? candidate.publicHost} · ${candidate.publicHost}:${candidate.bindPort}` }))];
  const selectedTunnelServer = frpServers.find((candidate) => candidate.id === form.serverId);
  const tunnelPublicHost = (tunnel: FrpTunnel) => frpServers.find((candidate) => candidate.id === tunnel.serverId)?.publicHost || settings?.publicHost || "VPS";
  const canSaveTunnel = Boolean((settings?.configured || selectedTunnelServer?.configured) && form.name.trim() && form.localHost.trim() && Number(form.localPort) && Number(form.remotePort));
  const firewallCommands = `sudo ufw allow ${settings?.bindPort ?? 7000}/tcp\nsudo ufw allow ${settings?.portStart ?? 25560}:${settings?.portEnd ?? 25600}/tcp\nsudo ufw allow ${settings?.portStart ?? 25560}:${settings?.portEnd ?? 25600}/udp`;

  return (
    <section className="panel frp-page">
      <header className="frp-page-head">
        <div className="frp-page-intro">
          <span className="section-kicker">Reverse proxy</span>
          <h2>FRP gateway</h2>
          <p>Expose private services through this VPS and manage every forward from one place.</p>
        </div>
        <div className="frp-server-controls">
          <StatusBadge
            status={!overview && loading ? "checking" : server?.running ? "online" : "offline"}
            label={!overview && loading ? "Checking server" : server?.running ? "Server running" : "Server stopped"}
          />
          <div className="actions">
            <Button variant="secondary" disabled={Boolean(busy)} onClick={() => void control("start")} icon={<Play size={15} />}>Start</Button>
            <Button variant="secondary" disabled={Boolean(busy)} onClick={() => void control("restart")} icon={<RotateCw size={15} />}>Restart</Button>
            <Button variant="danger" disabled={Boolean(busy)} onClick={() => void control("stop")} icon={<Power size={15} />}>Stop</Button>
          </div>
        </div>
      </header>

      <div className="frp-stat-grid">
        <div><span>Version</span><strong>{server?.version ?? "-"}</strong></div>
        <div><span>Uptime</span><strong>{duration(server?.uptimeSeconds ?? null)}</strong></div>
        <div><span>Received</span><strong>{bytes(server?.trafficInBytes ?? 0)}</strong></div>
        <div><span>Sent</span><strong>{bytes(server?.trafficOutBytes ?? 0)}</strong></div>
      </div>
      {server?.error ? <p className="form-error frp-server-error">{server.error}</p> : null}

      <section className="frp-page-section">
        <div className="frp-section-head">
          <div className="frp-section-title">
            <span className="frp-section-icon"><Network size={18} /></span>
            <div>
              <h3>Client connection</h3>
              <p>Set the public address once, then copy the credentials to your FRP client.</p>
            </div>
          </div>
        </div>
        <form className="frp-endpoint-form" onSubmit={saveSettings}>
          <div className="frp-endpoint-field">
            <TextField label="VPS IP or DNS-only hostname" value={publicHost} onChange={(value) => { setPublicHost(value); endpointDirty.current = true; }} placeholder="203.0.113.10" />
          </div>
          <div className="actions frp-endpoint-actions">
            <Button type="submit" loading={busy === "settings"}>Save endpoint</Button>
            <Button variant="secondary" onClick={() => void copyClientValue("token")} loading={busy === "client-token:manual"} icon={<KeyRound size={15} />}>Copy client token</Button>
            <Button variant="secondary" onClick={() => void copyClientValue("config")} loading={busy === "client-config:manual"} icon={<Copy size={15} />}>Copy manual frpc.toml</Button>
          </div>
          <p className="muted">Use a direct IP or DNS-only hostname. FRPC connects to port {settings?.bindPort ?? 7000}.</p>
        </form>
      </section>

      {assignments.length ? <section className="frp-page-section">
        <div className="frp-section-head">
          <div className="frp-section-title"><span className="frp-section-icon"><Network size={18} /></span><div><h3>Node FRP roles</h3><p>Choose which computers accept connections and which connect through them.</p></div></div>
          <span className="count">{assignments.length} nodes</span>
        </div>
        <div className="frp-node-grid">
          {assignments.map((assignment) => {
            const draft = assignmentDrafts[assignment.nodeId] ?? { role: assignment.role, serverId: assignment.serverId ?? "" };
            const needsServer = draft.role === "client" || draft.role === "both";
            return <article className="frp-node-card" key={assignment.nodeId}>
              <header><div><strong>{assignment.nodeName ?? assignment.nodeId}</strong><span>{assignment.nodeId}</span></div><StatusBadge status={assignment.status ?? draft.role} /></header>
              <div className="frp-node-fields">
                <CustomSelect label="FRP role" value={draft.role} options={[
                  { value: "disabled", label: "Disabled" }, { value: "client", label: "Client" }, { value: "server", label: "Server" }, { value: "both", label: "Both" }
                ]} onChange={(role) => setAssignmentDrafts((current) => ({ ...current, [assignment.nodeId]: { ...draft, role } }))} />
                <CustomSelect label="Connect to server" value={draft.serverId} options={serverOptions} disabled={!needsServer} onChange={(serverId) => setAssignmentDrafts((current) => ({ ...current, [assignment.nodeId]: { ...draft, serverId } }))} placeholder="No FRP servers" />
              </div>
              {assignment.lastError ? <p className="form-error">{assignment.lastError}</p> : null}
              <div className="actions">
                {needsServer ? <Button variant="secondary" onClick={() => void copyClientValue("config", assignment.nodeId)} loading={busy === `client-config:${assignment.nodeId}`} icon={<Copy size={14} />}>Copy config</Button> : null}
                <Button onClick={() => void saveAssignment(assignment.nodeId)} loading={busy === `assignment:${assignment.nodeId}`} disabled={needsServer && !draft.serverId}>Apply role</Button>
              </div>
            </article>;
          })}
        </div>
      </section> : null}

      <section className="frp-page-section">
        <div className="frp-section-head"><div><h3>FRP servers</h3><p className="muted">Create the gateway first, then assign that computer the server or both role.</p></div><span className="count">{frpServers.length} servers</span></div>
        <form className="frp-server-form" onSubmit={createServer}>
          <CustomSelect label="Server node" value={serverForm.nodeId} options={[{ value: "", label: "Select node" }, ...availableServerNodes.map((assignment) => ({ value: assignment.nodeId, label: assignment.nodeName ?? assignment.nodeId }))]} onChange={(nodeId) => setServerForm((current) => ({ ...current, nodeId }))} />
          <TextField label="Gateway name" value={serverForm.name} onChange={(name) => setServerForm((current) => ({ ...current, name }))} placeholder="Homeserver gateway" />
          <TextField label="Public IP or hostname" value={serverForm.publicHost} onChange={(publicHost) => setServerForm((current) => ({ ...current, publicHost }))} placeholder="vps.example.com" />
          <TextField label="Control port" value={serverForm.bindPort} onChange={(bindPort) => setServerForm((current) => ({ ...current, bindPort: bindPort.replace(/\D/g, "") }))} />
          <TextField label="Public port start" value={serverForm.portStart} onChange={(portStart) => setServerForm((current) => ({ ...current, portStart: portStart.replace(/\D/g, "") }))} />
          <TextField label="Public port end" value={serverForm.portEnd} onChange={(portEnd) => setServerForm((current) => ({ ...current, portEnd: portEnd.replace(/\D/g, "") }))} />
          <TextField label="Token (optional)" value={serverForm.authToken} onChange={(authToken) => setServerForm((current) => ({ ...current, authToken }))} type="password" placeholder="Generated automatically" />
          <Button type="submit" loading={busy === "server-create"} disabled={!serverForm.nodeId || !serverForm.name.trim() || !serverForm.publicHost.trim()}>Create server</Button>
        </form>
        <div className="frp-server-grid">
          {frpServers.map((candidate) => <article key={candidate.id}>
            <div><strong>{candidate.nodeName ?? candidate.nodeId}</strong><StatusBadge status={candidate.status ?? (candidate.configured ? "configured" : "pending")} /></div>
            <span>{candidate.publicHost}:{candidate.bindPort}</span>
            <span>Ports {candidate.portStart}–{candidate.portEnd}</span>
            {candidate.lastError ? <p className="form-error">{candidate.lastError}</p> : null}
          </article>)}
        </div>
      </section>

      <section className="frp-page-section">
        <div className="frp-section-head">
          <div className="frp-section-title">
            <span className="frp-section-icon"><Waypoints size={18} /></span>
            <div>
              <h3>{form.id ? "Edit port forward" : "Add port forward"}</h3>
              <p>Map a public VPS port to a service reachable from the FRP client.</p>
            </div>
          </div>
          {form.id ? <StatusBadge status="open-editor" label="Editing" /> : null}
        </div>
        <form className="form-grid" onSubmit={saveTunnel}>
          <div className="frp-form-grid">
            <TextField label="Name" value={form.name} onChange={(name) => setForm((current) => ({ ...current, name }))} placeholder="Web server" />
            <CustomSelect label="Client node" value={form.clientNodeId} options={clientOptions} onChange={(clientNodeId) => setForm((current) => ({ ...current, clientNodeId }))} placeholder="Manual client" />
            <CustomSelect label="FRP server" value={form.serverId} options={serverOptions} onChange={(serverId) => setForm((current) => ({ ...current, serverId }))} placeholder="Default server" />
            <CustomSelect label="Protocol" value={form.protocol} options={protocolOptions} onChange={(protocol) => setForm((current) => ({ ...current, protocol }))} />
            <TextField label="Local host" value={form.localHost} onChange={(localHost) => setForm((current) => ({ ...current, localHost }))} />
            <TextField label="Local port" value={form.localPort} onChange={(localPort) => setForm((current) => ({ ...current, localPort: localPort.replace(/\D/g, "") }))} placeholder="3000" />
            <TextField label="Public port" value={form.remotePort} onChange={(remotePort) => setForm((current) => ({ ...current, remotePort: remotePort.replace(/\D/g, "") }))} placeholder={String(selectedTunnelServer?.portStart ?? settings?.portStart ?? 25560)} />
          </div>
          <div className="frp-editor-footer">
            <ToggleField label="Enabled" value={form.enabled} onChange={(enabled) => setForm((current) => ({ ...current, enabled }))} description={`Public ports: ${selectedTunnelServer?.portStart ?? settings?.portStart ?? 25560}–${selectedTunnelServer?.portEnd ?? settings?.portEnd ?? 25600}`} />
            <p className="muted">For a service on the client machine, <code>127.0.0.1</code> is usually the right local host.</p>
            <div className="actions">
              {form.id ? <Button variant="ghost" onClick={() => setForm(emptyForm)}>Cancel</Button> : null}
              <Button type="submit" disabled={!canSaveTunnel} loading={busy === "tunnel-save"} icon={<Plus size={15} />}>{form.id ? "Save forward" : "Create forward"}</Button>
            </div>
          </div>
        </form>
      </section>

      <section className="frp-page-section frp-forwards-section">
        <div className="frp-section-head">
          <div>
            <h3>Port forwards</h3>
            <p className="muted">Live status and traffic from the FRP server.</p>
          </div>
          <span className="count">{loading ? "Loading" : `${overview?.tunnels.length ?? 0} rules`}</span>
        </div>
        <div className="table-wrap frp-table-wrap">
          <table>
            <thead><tr><th>Name</th><th>Mapping</th><th>Status</th><th>Traffic</th><th className="action-cell">Actions</th></tr></thead>
            <tbody>
              {overview?.tunnels.map((tunnel) => (
                <tr key={tunnel.id}>
                  <td><strong>{tunnel.name}</strong><br /><span className="muted">{tunnel.clientNodeName ?? tunnel.nodeName ?? "Manual client"} · {tunnel.serverName ?? "Default server"} · {tunnel.protocol.toUpperCase()}</span></td>
                  <td className="frp-mapping"><span>{tunnel.localHost}:{tunnel.localPort}</span><strong>→ {tunnelPublicHost(tunnel)}:{tunnel.remotePort}</strong></td>
                  <td><StatusBadge status={tunnel.syncStatus} /></td>
                  <td>{bytes(tunnel.trafficInBytes)} ↓ / {bytes(tunnel.trafficOutBytes)} ↑<br /><span className="muted">{tunnel.currentConnections} active</span></td>
                  <td className="action-cell">
                    <div className="table-actions icon-actions">
                      <IconButton label="Copy endpoint" onClick={() => void copyText(`${tunnelPublicHost(tunnel)}:${tunnel.remotePort}`)}><Copy size={14} /></IconButton>
                      <IconButton label="Edit tunnel" onClick={() => setForm(formFor(tunnel))}><Edit3 size={14} /></IconButton>
                      <IconButton label={tunnel.enabled ? "Disable tunnel" : "Enable tunnel"} disabled={busy === `tunnel-toggle:${tunnel.id}`} onClick={() => void toggleTunnel(tunnel)}>{tunnel.enabled ? <Power size={14} /> : <Play size={14} />}</IconButton>
                      <IconButton label="Delete tunnel" variant="danger" onClick={() => setConfirm({
                        title: "Delete FRP tunnel",
                        body: `Delete ${tunnel.name}? Copy the updated frpc.toml and restart your FRP client so it stops using this proxy.`,
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

      <details className="frp-firewall-details">
        <summary>
          <div className="frp-section-title">
            <span className="frp-section-icon"><Activity size={18} /></span>
            <div>
              <h3>VPS firewall commands</h3>
              <p>Open the FRP control and public forwarding ports with UFW.</p>
            </div>
          </div>
          <span className="frp-details-action">View commands</span>
        </summary>
        <div className="frp-firewall-content">
          <pre>{firewallCommands}</pre>
          <Button variant="secondary" onClick={() => void copyText(firewallCommands)} icon={<Copy size={15} />}>Copy commands</Button>
        </div>
      </details>
    </section>
  );
});
