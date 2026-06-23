import { useEffect, useMemo, useState } from "react";
import { Plus, RefreshCw, Trash2 } from "lucide-react";
import type { CloudflareClient, CloudflareRoute, CloudflareTunnel, CloudflareTunnelAssignment, CloudflareZone, ContainerInfo, Project } from "../../shared/types";
import { api } from "../lib/api";
import { Button, CustomSelect, StatusBadge, TextField, ToggleField } from "../components/ui";

type Tab = "hostnames" | "tunnels" | "clients";
type Toast = (message: string, kind?: "ok" | "error" | "loading") => void;

export function HostnamesView({ projects, containers, toast }: { projects: Project[]; containers: ContainerInfo[]; toast: Toast }) {
  const [tab, setTab] = useState<Tab>("hostnames");
  const [clients, setClients] = useState<CloudflareClient[]>([]);
  const [tunnels, setTunnels] = useState<CloudflareTunnel[]>([]);
  const [assignments, setAssignments] = useState<CloudflareTunnelAssignment[]>([]);
  const [hostnames, setHostnames] = useState<CloudflareRoute[]>([]);
  const [zones, setZones] = useState<Record<string, CloudflareZone[]>>({});
  const [busy, setBusy] = useState(false);
  const [clientForm, setClientForm] = useState({ name: "", accountId: "", apiToken: "" });
  const [tunnelForm, setTunnelForm] = useState({ clientId: "", name: "" });
  const [assignmentForm, setAssignmentForm] = useState({ tunnelId: "", target: "" });
  const [hostnameForm, setHostnameForm] = useState({ tunnelId: "", assignmentId: "", zoneId: "", hostname: "", protocol: "http" as "http" | "https", port: "3000", noTlsVerify: false });

  const load = async () => {
    const [nextClients, nextTunnels, nextAssignments, nextHostnames] = await Promise.all([api.cloudflareClients(), api.cloudflareTunnels(), api.cloudflareAssignments(), api.cloudflareHostnames()]);
    setClients(nextClients); setTunnels(nextTunnels); setAssignments(nextAssignments); setHostnames(nextHostnames);
  };

  useEffect(() => { void load().catch((error) => toast(error instanceof Error ? error.message : "Unable to load Cloudflare data.", "error")); }, []);

  const selectedTunnel = tunnels.find((item) => item.id === hostnameForm.tunnelId);
  const tunnelAssignments = assignments.filter((item) => item.tunnelId === hostnameForm.tunnelId);
  const clientOptions = useMemo(() => [{ label: "Choose client", value: "" }, ...clients.map((client) => ({ label: client.name, value: client.id }))], [clients]);
  const tunnelOptions = useMemo(() => [{ label: "Choose tunnel", value: "" }, ...tunnels.map((tunnel) => ({ label: tunnel.tunnelName, value: tunnel.id }))], [tunnels]);
  const zoneOptions = useMemo(() => [{ label: selectedTunnel ? "Choose zone" : "Choose tunnel first", value: "" }, ...(selectedTunnel ? zones[selectedTunnel.clientId] ?? [] : []).map((zone) => ({ label: zone.name, value: zone.id }))], [selectedTunnel, zones]);
  const assignmentOptions = useMemo(() => [{ label: hostnameForm.tunnelId ? "Choose assigned target" : "Choose tunnel first", value: "" }, ...tunnelAssignments.map((item) => ({ label: item.targetType === "compose_service" ? `${item.composeProject} / ${item.composeService}` : item.containerName ?? "Container", value: item.id }))], [hostnameForm.tunnelId, tunnelAssignments]);
  const targetOptions = useMemo(() => {
    const seen = new Set<string>();
    const options = containers.filter((container) => {
      const key = container.composeProject && container.composeService ? `compose:${container.composeProject}:${container.composeService}` : `container:${container.name}`;
      if (seen.has(key)) return false; seen.add(key); return true;
    }).map((container) => ({
      value: container.composeProject && container.composeService ? `compose:${container.composeProject}:${container.composeService}` : `container:${container.name}`,
      label: container.composeProject && container.composeService ? `${container.composeProject} / ${container.composeService}` : container.name
    }));
    return [{ label: "Choose target", value: "" }, ...options];
  }, [containers]);

  async function act(label: string, action: () => Promise<void>) {
    setBusy(true); toast(label, "loading");
    try { await action(); toast("Cloudflare configuration updated."); }
    catch (error) { toast(error instanceof Error ? error.message : "Cloudflare operation failed.", "error"); }
    finally { await load().catch(() => undefined); setBusy(false); }
  }

  async function selectHostnameTunnel(tunnelId: string) {
    const tunnel = tunnels.find((item) => item.id === tunnelId);
    setHostnameForm((current) => ({ ...current, tunnelId, assignmentId: "", zoneId: "" }));
    if (tunnel && !zones[tunnel.clientId]) {
      const next = await api.cloudflareZones(tunnel.clientId);
      setZones((current) => ({ ...current, [tunnel.clientId]: next }));
    }
  }

  return (
    <section className="cloudflare-manager">
      <div className="panel-head"><div><h2>Cloudflare</h2><p className="muted">Client-owned tunnels, isolated Docker networks, and public hostnames.</p></div><Button variant="secondary" onClick={() => void load()} icon={<RefreshCw size={15} />}>Refresh</Button></div>
      <div className="cloudflare-tabs">
        {(["hostnames", "tunnels", "clients"] as Tab[]).map((item) => <button type="button" className={tab === item ? "active" : ""} onClick={() => setTab(item)} key={item}>{item === "tunnels" ? "Tunnels & Networks" : item[0].toUpperCase() + item.slice(1)}</button>)}
      </div>

      {tab === "clients" ? <>
        <section className="panel compact-form"><h3>Add client</h3><div className="cf-manager-form"><TextField label="Client name" value={clientForm.name} onChange={(name) => setClientForm({ ...clientForm, name })} /><TextField label="Account ID" value={clientForm.accountId} onChange={(accountId) => setClientForm({ ...clientForm, accountId })} /><TextField label="Limited API token" type="password" value={clientForm.apiToken} onChange={(apiToken) => setClientForm({ ...clientForm, apiToken })} /><Button disabled={busy || !clientForm.name || !clientForm.accountId || !clientForm.apiToken} icon={<Plus size={15} />} onClick={() => void act("Validating and saving client...", async () => { await api.createCloudflareClient(clientForm); setClientForm({ name: "", accountId: "", apiToken: "" }); })}>Add client</Button></div></section>
        <section className="panel"><h3>Clients</h3><div className="cf-manager-list">{clients.map((client) => <div className="cf-manager-row" key={client.id}><div><strong>{client.name}</strong><span>{client.accountId}</span></div><StatusBadge status={client.hasApiToken ? "ready" : "missing"} label={client.hasApiToken ? "Token saved" : "No token"} /><Button variant="danger" disabled={busy} onClick={() => void act("Deleting client...", () => api.deleteCloudflareClient(client.id))} icon={<Trash2 size={14} />}>Delete</Button></div>)}</div></section>
      </> : null}

      {tab === "tunnels" ? <>
        <section className="panel compact-form"><h3>Create tunnel</h3><div className="cf-manager-form"><CustomSelect label="Client" value={tunnelForm.clientId} options={clientOptions} onChange={(clientId) => setTunnelForm({ ...tunnelForm, clientId })} disabled={busy} /><TextField label="Tunnel name" value={tunnelForm.name} onChange={(name) => setTunnelForm({ ...tunnelForm, name })} /><Button disabled={busy || !tunnelForm.clientId || !tunnelForm.name} icon={<Plus size={15} />} onClick={() => void act("Creating tunnel and Docker network...", async () => { await api.createCloudflareTunnel(tunnelForm); setTunnelForm({ clientId: "", name: "" }); })}>Create tunnel</Button></div></section>
        <section className="panel compact-form"><h3>Assign service or container</h3><div className="cf-manager-form"><CustomSelect label="Tunnel" value={assignmentForm.tunnelId} options={tunnelOptions} onChange={(tunnelId) => setAssignmentForm({ ...assignmentForm, tunnelId })} disabled={busy} /><CustomSelect label="Project service / container" value={assignmentForm.target} options={targetOptions} onChange={(target) => setAssignmentForm({ ...assignmentForm, target })} disabled={busy} /><Button disabled={busy || !assignmentForm.tunnelId || !assignmentForm.target} onClick={() => void act("Connecting target to tunnel network...", async () => { const [type, first, second] = assignmentForm.target.split(":"); const project = projects.find((item) => item.folderName === first); await api.createCloudflareAssignment(type === "compose" ? { tunnelId: assignmentForm.tunnelId, projectId: project?.id, composeProject: first, composeService: second } : { tunnelId: assignmentForm.tunnelId, containerName: first }); setAssignmentForm({ tunnelId: "", target: "" }); })}>Assign</Button></div></section>
        <section className="panel"><h3>Tunnels & networks</h3><div className="cf-manager-list">{tunnels.map((tunnel) => <div className="cf-manager-tunnel" key={tunnel.id}><div className="cf-manager-row"><div><strong>{tunnel.tunnelName}</strong><span>{tunnel.dockerNetworkName}</span></div><StatusBadge status={tunnel.status} /><div className="actions"><Button variant="ghost" onClick={() => void act("Starting tunnel...", async () => { await api.startCloudflared(tunnel.id); })}>Start</Button><Button variant="ghost" onClick={() => void act("Restarting tunnel...", async () => { await api.restartCloudflared(tunnel.id); })}>Restart</Button><Button variant="danger" onClick={() => void act("Deleting tunnel...", () => api.deleteCloudflareTunnel(tunnel.id, true))}>Delete</Button></div></div>{assignments.filter((item) => item.tunnelId === tunnel.id).map((item) => <div className="cf-assignment-row" key={item.id}><span>{item.targetType === "compose_service" ? `${item.composeProject} / ${item.composeService}` : item.containerName}</span><button type="button" onClick={() => void act("Removing network assignment...", () => api.deleteCloudflareAssignment(item.id))}><Trash2 size={13} /></button></div>)}</div>)}</div></section>
      </> : null}

      {tab === "hostnames" ? <>
        <section className="panel compact-form"><h3>Create route</h3><p className="muted">A route publishes a hostname to an assigned tunnel target and keeps Cloudflare DNS in sync.</p><div className="cf-manager-form hostname-form"><CustomSelect label="Tunnel" value={hostnameForm.tunnelId} options={tunnelOptions} onChange={(tunnelId) => void selectHostnameTunnel(tunnelId)} disabled={busy} /><CustomSelect label="Zone" value={hostnameForm.zoneId} options={zoneOptions} onChange={(zoneId) => setHostnameForm({ ...hostnameForm, zoneId })} disabled={busy || !selectedTunnel} /><CustomSelect label="Assigned target" value={hostnameForm.assignmentId} options={assignmentOptions} onChange={(assignmentId) => setHostnameForm({ ...hostnameForm, assignmentId })} disabled={busy || !hostnameForm.tunnelId} /><TextField label="Hostname" value={hostnameForm.hostname} onChange={(hostname) => setHostnameForm({ ...hostnameForm, hostname })} placeholder="app.example.com" /><CustomSelect<"http" | "https"> label="Protocol" value={hostnameForm.protocol} options={[{ label: "HTTP", value: "http" }, { label: "HTTPS", value: "https" }]} onChange={(protocol) => setHostnameForm({ ...hostnameForm, protocol })} disabled={busy} /><TextField label="Port" value={hostnameForm.port} onChange={(port) => setHostnameForm({ ...hostnameForm, port })} />{hostnameForm.protocol === "https" ? <ToggleField label="No TLS verify" value={hostnameForm.noTlsVerify} onChange={(noTlsVerify) => setHostnameForm({ ...hostnameForm, noTlsVerify })} /> : null}<Button disabled={busy || !hostnameForm.tunnelId || !hostnameForm.assignmentId || !hostnameForm.zoneId || !hostnameForm.hostname} icon={<Plus size={15} />} onClick={() => void act("Publishing route and DNS...", async () => { await api.createCloudflareHostname({ ...hostnameForm, port: Number(hostnameForm.port) }); setHostnameForm({ tunnelId: "", assignmentId: "", zoneId: "", hostname: "", protocol: "http", port: "3000", noTlsVerify: false }); })}>Create route</Button></div></section>
        <section className="panel"><h3>Managed routes & hostnames</h3><div className="cf-manager-list">{hostnames.map((route) => <div className="cf-manager-row" key={route.id}><div><a href={`https://${route.hostname}`} target="_blank" rel="noreferrer"><strong>{route.hostname}</strong></a><span>{route.serviceTarget}</span>{route.lastError ? <span className="danger-text">{route.lastError}</span> : null}</div><StatusBadge status={route.syncStatus} /><div className="actions">{route.syncStatus === "error" ? <Button variant="secondary" disabled={busy} onClick={() => void act("Retrying hostname synchronization...", async () => { await api.retryCloudflareHostname(route.id); })}>Retry</Button> : null}<Button variant="danger" disabled={busy} onClick={() => void act("Deleting hostname and DNS...", () => api.deleteCloudflareHostname(route.id))} icon={<Trash2 size={14} />}>Delete</Button></div></div>)}</div></section>
      </> : null}
    </section>
  );
}
