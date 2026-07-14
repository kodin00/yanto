import { useEffect, useMemo, useState } from "react";
import { Plus, Trash2 } from "lucide-react";
import type { CloudflareClient, CloudflareRoute, CloudflareTunnel, CloudflareTunnelAssignment, CloudflareZone, ContainerInfo, Project } from "../../shared/types";
import { cloudflareAssignmentTcpPorts, cloudflareServiceUrl } from "../app-utils";
import { api } from "../lib/api";
import { Button, ConfirmDialog, CustomSelect, LoadingInline, StatusBadge, TextField, ToggleField } from "../components/ui";

type Tab = "hostnames" | "tunnels" | "clients";
type Toast = (message: string, kind?: "ok" | "error" | "loading") => void;
type ConfirmState = { title: string; body: string; label: string; actionLabel: string; action: () => Promise<string | void> };

type HostnamesProps = { projects: Project[]; containers: ContainerInfo[]; refreshKey: number; isOwner: boolean; toast: Toast };

export function HostnamesView(props: HostnamesProps) {
  return props.isOwner ? <OwnerHostnamesView {...props} /> : <ProjectHostnamesView {...props} />;
}

function OwnerHostnamesView({ projects, containers, refreshKey, toast }: HostnamesProps) {
  const [tab, setTab] = useState<Tab>("hostnames");
  const [clients, setClients] = useState<CloudflareClient[]>([]);
  const [tunnels, setTunnels] = useState<CloudflareTunnel[]>([]);
  const [assignments, setAssignments] = useState<CloudflareTunnelAssignment[]>([]);
  const [hostnames, setHostnames] = useState<CloudflareRoute[]>([]);
  const [zones, setZones] = useState<Record<string, CloudflareZone[]>>({});
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<ConfirmState | null>(null);
  const [clientForm, setClientForm] = useState({ name: "", accountId: "", zoneId: "", apiToken: "" });
  const [tunnelForm, setTunnelForm] = useState({ clientId: "", name: "" });
  const [assignmentForm, setAssignmentForm] = useState({ tunnelId: "", target: "" });
  const [hostnameForm, setHostnameForm] = useState({ tunnelId: "", assignmentId: "", zoneId: "", hostname: "", protocol: "http" as "http" | "https", port: "", noTlsVerify: false });

  const load = async () => {
    setLoading(true);
    try {
      const [nextClients, nextTunnels, nextAssignments, nextHostnames] = await Promise.all([api.cloudflareClients(), api.cloudflareTunnels(), api.cloudflareAssignments(), api.cloudflareHostnames()]);
      setClients(nextClients); setTunnels(nextTunnels); setAssignments(nextAssignments); setHostnames(nextHostnames);
      setError(null);
    } catch (loadError) {
      setError(loadError instanceof Error ? loadError.message : "Unable to load Cloudflare data.");
      throw loadError;
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => { void load().catch((error) => toast(error instanceof Error ? error.message : "Unable to load Cloudflare data.", "error")); }, [refreshKey]);

  const selectedTunnel = tunnels.find((item) => item.id === hostnameForm.tunnelId);
  const selectedClient = selectedTunnel ? clients.find((client) => client.id === selectedTunnel.clientId) : undefined;
  const tunnelAssignments = assignments.filter((item) => item.tunnelId === hostnameForm.tunnelId);
  const selectedAssignment = assignments.find((item) => item.id === hostnameForm.assignmentId);
  const selectedTargetPorts = useMemo(
    () => selectedAssignment ? cloudflareAssignmentTcpPorts(selectedAssignment, containers) : [],
    [containers, selectedAssignment]
  );
  const clientOptions = useMemo(() => [{ label: "Choose client", value: "" }, ...clients.map((client) => ({ label: client.name, value: client.id }))], [clients]);
  const tunnelOptions = useMemo(() => [{ label: "Choose tunnel", value: "" }, ...tunnels.map((tunnel) => ({ label: tunnel.tunnelName, value: tunnel.id }))], [tunnels]);
  const zoneOptions = useMemo(() => {
    const loadedZones = selectedTunnel ? zones[selectedTunnel.clientId] ?? [] : [];
    const options = loadedZones.map((zone) => ({ label: zone.name, value: zone.id }));
    if (selectedClient?.zoneId && !options.some((option) => option.value === selectedClient.zoneId)) {
      options.unshift({ label: selectedClient.zoneId, value: selectedClient.zoneId });
    }
    return [{ label: selectedTunnel ? "Choose zone" : "Choose tunnel first", value: "" }, ...options];
  }, [selectedClient, selectedTunnel, zones]);
  const assignmentOptions = useMemo(() => [{ label: hostnameForm.tunnelId ? "Choose assigned target" : "Choose tunnel first", value: "" }, ...tunnelAssignments.map((item) => {
    const label = item.targetType === "compose_service" ? `${item.composeProject} / ${item.composeService}` : item.containerName ?? "Container";
    const ports = cloudflareAssignmentTcpPorts(item, containers);
    const portLabel = ports.length === 1 ? ` · :${ports[0]}` : ports.length > 1 ? ` · ${ports.length} TCP ports` : "";
    return { label: `${label}${portLabel}`, value: item.id };
  })], [containers, hostnameForm.tunnelId, tunnelAssignments]);
  const targetPortOptions = useMemo(
    () => selectedTargetPorts.map((port) => ({ label: `${port} / TCP`, value: String(port) })),
    [selectedTargetPorts]
  );
  useEffect(() => {
    if (!selectedAssignment) return;
    setHostnameForm((current) => {
      if (selectedTargetPorts.includes(Number(current.port))) return current;
      return { ...current, port: selectedTargetPorts[0] ? String(selectedTargetPorts[0]) : "" };
    });
  }, [selectedAssignment, selectedTargetPorts]);
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

  async function act(label: string, action: () => Promise<string | void>) {
    setBusy(true); toast(label, "loading");
    try { const successMessage = await action(); toast(successMessage || "Cloudflare configuration updated."); }
    catch (error) { toast(error instanceof Error ? error.message : "Cloudflare operation failed.", "error"); }
    finally { await load().catch(() => undefined); setBusy(false); }
  }

  async function selectHostnameTunnel(tunnelId: string) {
    const tunnel = tunnels.find((item) => item.id === tunnelId);
    const client = tunnel ? clients.find((item) => item.id === tunnel.clientId) : undefined;
    setHostnameForm((current) => ({ ...current, tunnelId, assignmentId: "", zoneId: client?.zoneId ?? "", port: "" }));
    if (tunnel && !zones[tunnel.clientId]) {
      try {
        const next = await api.cloudflareZones(tunnel.clientId);
        setZones((current) => ({ ...current, [tunnel.clientId]: next }));
      } catch (zoneError) {
        const message = zoneError instanceof Error ? zoneError.message : "Unable to load Cloudflare zones.";
        setError(message);
        toast(message, "error");
      }
    }
  }

  function selectHostnameAssignment(assignmentId: string) {
    const assignment = assignments.find((item) => item.id === assignmentId);
    const port = assignment ? cloudflareAssignmentTcpPorts(assignment, containers)[0] : undefined;
    setHostnameForm((current) => ({ ...current, assignmentId, port: port ? String(port) : "" }));
  }

  return (
    <section className="cloudflare-manager">
      <div className="panel-head"><div><h2>Cloudflare</h2><p className="muted">Client-owned tunnels, isolated Docker networks, and public hostnames.</p></div></div>
      <div className="cloudflare-tabs" role="navigation" aria-label="Cloudflare sections">
        {(["hostnames", "tunnels", "clients"] as Tab[]).map((item) => <button type="button" aria-pressed={tab === item} className={tab === item ? "active" : ""} onClick={() => setTab(item)} key={item}>{item === "tunnels" ? "Tunnels & Networks" : item[0].toUpperCase() + item.slice(1)}</button>)}
      </div>

      {error ? <section className="panel view-inline-error" role="alert"><strong>Cloudflare data could not be loaded.</strong><p>{error}</p><Button variant="secondary" disabled={loading} onClick={() => void load().catch((loadError) => toast(loadError instanceof Error ? loadError.message : "Unable to load Cloudflare data.", "error"))}>Try again</Button></section> : null}
      {loading && !clients.length && !tunnels.length && !hostnames.length ? <section className="panel"><LoadingInline label="Loading Cloudflare configuration…" /></section> : null}

      {tab === "clients" ? <>
        <section className="panel compact-form"><h3>Add client</h3><div className="cf-manager-form"><TextField label="Client name" value={clientForm.name} onChange={(name) => setClientForm({ ...clientForm, name })} /><TextField label="Account ID" value={clientForm.accountId} onChange={(accountId) => setClientForm({ ...clientForm, accountId })} /><TextField label="Zone ID" value={clientForm.zoneId} onChange={(zoneId) => setClientForm({ ...clientForm, zoneId })} /><TextField label="Limited API token" type="password" value={clientForm.apiToken} onChange={(apiToken) => setClientForm({ ...clientForm, apiToken })} /><Button disabled={busy || !clientForm.name || !clientForm.accountId || !clientForm.zoneId || !clientForm.apiToken} icon={<Plus size={15} />} onClick={() => void act("Validating and saving client...", async () => { await api.createCloudflareClient(clientForm); setClientForm({ name: "", accountId: "", zoneId: "", apiToken: "" }); })}>Add client</Button></div></section>
        <section className="panel"><h3>Clients</h3><div className="cf-manager-list">{clients.map((client) => <div className="cf-manager-row" key={client.id}><div><strong>{client.name}</strong><span>{client.accountId}</span><span>{client.zoneId}</span></div><StatusBadge status={client.hasApiToken ? "ready" : "missing"} label={client.hasApiToken ? "Token saved" : "No token"} /><Button variant="danger" disabled={busy} onClick={() => setConfirm({ title: "Delete Cloudflare client?", body: `Delete ${client.name}? Tunnels still using this client must be removed first.`, label: "Delete client", actionLabel: "Deleting client...", action: () => api.deleteCloudflareClient(client.id) })} icon={<Trash2 size={14} />}>Delete</Button></div>)}{!clients.length && !loading ? <p className="muted">No Cloudflare clients configured.</p> : null}</div></section>
      </> : null}

      {tab === "tunnels" ? <>
        <section className="panel compact-form"><h3>Create tunnel</h3><div className="cf-manager-form"><CustomSelect label="Client" value={tunnelForm.clientId} options={clientOptions} onChange={(clientId) => setTunnelForm({ ...tunnelForm, clientId })} disabled={busy} /><TextField label="Tunnel name" value={tunnelForm.name} onChange={(name) => setTunnelForm({ ...tunnelForm, name })} /><Button disabled={busy || !tunnelForm.clientId || !tunnelForm.name} icon={<Plus size={15} />} onClick={() => void act("Creating tunnel and Docker network...", async () => { await api.createCloudflareTunnel(tunnelForm); setTunnelForm({ clientId: "", name: "" }); })}>Create tunnel</Button></div></section>
        <section className="panel compact-form"><h3>Assign service or container</h3><div className="cf-manager-form"><CustomSelect label="Tunnel" value={assignmentForm.tunnelId} options={tunnelOptions} onChange={(tunnelId) => setAssignmentForm({ ...assignmentForm, tunnelId })} disabled={busy} /><CustomSelect label="Project service / container" value={assignmentForm.target} options={targetOptions} onChange={(target) => setAssignmentForm({ ...assignmentForm, target })} disabled={busy} /><Button disabled={busy || !assignmentForm.tunnelId || !assignmentForm.target} onClick={() => void act("Connecting target to tunnel network...", async () => { const [type, first, second] = assignmentForm.target.split(":"); const project = projects.find((item) => item.folderName === first); await api.createCloudflareAssignment(type === "compose" ? { tunnelId: assignmentForm.tunnelId, projectId: project?.id, composeProject: first, composeService: second } : { tunnelId: assignmentForm.tunnelId, containerName: first }); setAssignmentForm({ tunnelId: "", target: "" }); })}>Assign</Button></div></section>
        <section className="panel"><h3>Tunnels & networks</h3><div className="cf-manager-list">{tunnels.map((tunnel) => <div className="cf-manager-tunnel" key={tunnel.id}><div className="cf-manager-row"><div><strong>{tunnel.tunnelName}</strong><span>{tunnel.dockerNetworkName}</span></div><StatusBadge status={tunnel.status} /><div className="actions"><Button variant="ghost" disabled={busy} onClick={() => void act("Starting tunnel...", async () => { await api.startCloudflared(tunnel.nodeId); })}>Start</Button><Button variant="ghost" disabled={busy} onClick={() => void act("Restarting tunnel...", async () => { await api.restartCloudflared(tunnel.nodeId); })}>Restart</Button><Button variant="ghost" disabled={busy} onClick={() => void act("Stopping tunnel...", async () => { await api.stopCloudflared(tunnel.nodeId); })}>Stop</Button><Button variant="danger" disabled={busy} onClick={() => setConfirm({ title: "Delete Cloudflare tunnel?", body: `Delete ${tunnel.tunnelName}, its Docker network, assignments, hostnames, and managed DNS records?`, label: "Delete tunnel", actionLabel: "Deleting tunnel...", action: () => api.deleteCloudflareTunnel(tunnel.id, true) })}>Delete</Button></div></div>{assignments.filter((item) => item.tunnelId === tunnel.id).map((item) => <div className="cf-assignment-row" key={item.id}><span>{item.targetType === "compose_service" ? `${item.composeProject} / ${item.composeService}` : item.containerName}</span><button type="button" aria-label="Remove network assignment" title="Remove network assignment" disabled={busy} onClick={() => setConfirm({ title: "Remove network assignment?", body: "Remove this target from the tunnel network? Public hostnames using it may stop working.", label: "Remove assignment", actionLabel: "Removing network assignment...", action: () => api.deleteCloudflareAssignment(item.id) })}><Trash2 size={13} /></button></div>)}</div>)}{!tunnels.length && !loading ? <p className="muted">No Cloudflare tunnels configured.</p> : null}</div></section>
      </> : null}

      {tab === "hostnames" ? <>
        <section className="panel compact-form">
          <h3>Create route</h3>
          <p className="muted">A route publishes a hostname to an assigned tunnel target and keeps Cloudflare DNS in sync.</p>
          <div className="cf-manager-form hostname-form">
            <CustomSelect label="Tunnel" value={hostnameForm.tunnelId} options={tunnelOptions} onChange={(tunnelId) => void selectHostnameTunnel(tunnelId)} disabled={busy} />
            <CustomSelect label="Zone" value={hostnameForm.zoneId} options={zoneOptions} onChange={(zoneId) => setHostnameForm({ ...hostnameForm, zoneId })} disabled={busy || !selectedTunnel} />
            <CustomSelect label="Assigned target" value={hostnameForm.assignmentId} options={assignmentOptions} onChange={selectHostnameAssignment} disabled={busy || !hostnameForm.tunnelId} />
            <TextField label="Hostname" value={hostnameForm.hostname} onChange={(hostname) => setHostnameForm({ ...hostnameForm, hostname })} placeholder="app.example.com" />
            <CustomSelect<"http" | "https"> label="Protocol" value={hostnameForm.protocol} options={[{ label: "HTTP", value: "http" }, { label: "HTTPS", value: "https" }]} onChange={(protocol) => setHostnameForm({ ...hostnameForm, protocol })} disabled={busy} />
            {selectedTargetPorts.length > 1 ? (
              <CustomSelect label="Service port" value={hostnameForm.port} options={targetPortOptions} onChange={(port) => setHostnameForm({ ...hostnameForm, port })} disabled={busy} />
            ) : (
              <div className={`cf-detected-port ${selectedAssignment && !selectedTargetPorts.length ? "missing" : ""}`}>
                <span>Service port</span>
                <div>
                  <strong>{selectedTargetPorts.length === 1 ? `${selectedTargetPorts[0]} / TCP` : selectedAssignment ? "No TCP port detected" : "Choose a target"}</strong>
                  <small>{selectedTargetPorts.length === 1 ? "Detected from Docker" : selectedAssignment ? "Expose the service port in Docker Compose or the image." : "The target port will be detected automatically."}</small>
                </div>
              </div>
            )}
            {hostnameForm.protocol === "https" ? <ToggleField label="No TLS verify" value={hostnameForm.noTlsVerify} onChange={(noTlsVerify) => setHostnameForm({ ...hostnameForm, noTlsVerify })} /> : null}
            <Button
              disabled={busy || !hostnameForm.tunnelId || !hostnameForm.assignmentId || !hostnameForm.zoneId || !hostnameForm.hostname || !hostnameForm.port}
              icon={<Plus size={15} />}
              onClick={() => void act("Publishing route and DNS...", async () => {
                await api.createCloudflareHostname({ ...hostnameForm, port: Number(hostnameForm.port) });
                setHostnameForm({ tunnelId: "", assignmentId: "", zoneId: "", hostname: "", protocol: "http", port: "", noTlsVerify: false });
              })}
            >
              Create route
            </Button>
          </div>
        </section>
        <section className="panel"><h3>Managed routes & hostnames</h3><div className="cf-manager-list">{hostnames.map((route) => <div className="cf-manager-row" key={route.id}><div><a href={`https://${route.hostname}`} target="_blank" rel="noreferrer"><strong>{route.hostname}</strong></a><span>{route.serviceTarget}</span>{route.lastError ? <span className="danger-text">{route.lastError}</span> : null}</div><StatusBadge status={route.syncStatus} /><div className="actions">{route.syncStatus === "error" ? <Button variant="secondary" disabled={busy} onClick={() => void act("Retrying hostname synchronization...", async () => { await api.retryCloudflareHostname(route.id); })}>Retry</Button> : null}<Button variant="danger" disabled={busy} onClick={() => setConfirm({ title: "Delete public hostname?", body: `Delete ${route.hostname} and its managed Cloudflare DNS record?`, label: "Delete hostname", actionLabel: "Deleting hostname and DNS...", action: async () => { const result = await api.deleteCloudflareHostname(route.id); return result?.warnings.length ? `Hostname deleted with warnings: ${result.warnings.join("; ")}` : undefined; } })} icon={<Trash2 size={14} />}>Delete</Button></div></div>)}{!hostnames.length && !loading ? <p className="muted">No managed hostnames configured.</p> : null}</div></section>
      </> : null}
      {confirm ? <ConfirmDialog title={confirm.title} body={confirm.body} confirmLabel={confirm.label} danger onClose={() => setConfirm(null)} onConfirm={() => { const pending = confirm; setConfirm(null); void act(pending.actionLabel, pending.action); }} /> : null}
    </section>
  );
}

function ProjectHostnamesView({ projects, containers, refreshKey, toast }: HostnamesProps) {
  const [projectId, setProjectId] = useState(projects[0]?.id ?? "");
  const [routes, setRoutes] = useState<Record<string, CloudflareRoute[]>>({});
  const [form, setForm] = useState({ hostname: "", serviceTarget: "", noTlsVerify: false });
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [removeRoute, setRemoveRoute] = useState<CloudflareRoute | null>(null);
  const project = projects.find((entry) => entry.id === projectId);

  useEffect(() => {
    if (projects.some((entry) => entry.id === projectId)) return;
    setProjectId(projects[0]?.id ?? "");
  }, [projectId, projects]);

  useEffect(() => {
    let active = true;
    setLoading(true);
    void Promise.all(projects.map(async (entry) => [entry.id, await api.projectCfRoutes(entry.id)] as const))
      .then((entries) => { if (active) setRoutes(Object.fromEntries(entries)); })
      .catch((error) => { if (active) toast(error instanceof Error ? error.message : "Unable to load hostnames.", "error"); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [projects, refreshKey]);

  useEffect(() => {
    if (!project) return;
    const projectContainers = containers.filter((container) => container.composeProject === project.folderName);
    setForm((current) => current.serviceTarget ? current : { ...current, serviceTarget: cloudflareServiceUrl(project, projectContainers) });
  }, [containers, project]);

  async function publish() {
    if (!project || !form.hostname.trim() || !form.serviceTarget.trim()) return;
    setBusy(true);
    toast("Publishing hostname...", "loading");
    try {
      const route = await api.publishCfRoute(project.id, form);
      setRoutes((current) => ({ ...current, [project.id]: [route] }));
      setForm((current) => ({ ...current, hostname: "" }));
      toast("Hostname published.");
    } catch (error) {
      toast(error instanceof Error ? error.message : "Unable to publish hostname.", "error");
    } finally {
      setBusy(false);
    }
  }

  async function remove(route: CloudflareRoute) {
    setBusy(true);
    setRemoveRoute(null);
    try {
      await api.deleteCfRoute(route.id);
      setRoutes((current) => Object.fromEntries(Object.entries(current).map(([id, entries]) => [id, entries.filter((entry) => entry.id !== route.id)])));
      toast("Hostname removed.");
    } catch (error) {
      toast(error instanceof Error ? error.message : "Unable to remove hostname.", "error");
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="cloudflare-manager">
      <div className="panel-head"><div><h2>Project hostnames</h2><p className="muted">Publish and manage routes for projects assigned to you.</p></div></div>
      <section className="panel compact-form">
        <CustomSelect label="Project" value={projectId} options={projects.map((entry) => ({ label: entry.name, value: entry.id }))} onChange={(nextProjectId) => { setProjectId(nextProjectId); setForm({ hostname: "", serviceTarget: "", noTlsVerify: false }); }} />
        <TextField label="Hostname" value={form.hostname} onChange={(hostname) => setForm({ ...form, hostname })} placeholder="app.example.com" />
        <TextField label="Service target" value={form.serviceTarget} onChange={(serviceTarget) => setForm({ ...form, serviceTarget })} placeholder="http://service:3000" />
        <ToggleField label="No TLS verify" value={form.noTlsVerify} onChange={(noTlsVerify) => setForm({ ...form, noTlsVerify })} />
        <div className="actions"><Button disabled={busy || !projectId || !form.hostname.trim() || !form.serviceTarget.trim()} onClick={() => void publish()} icon={<Plus size={15} />}>Publish hostname</Button></div>
      </section>
      <section className="panel">
        <h3>Managed routes</h3>
        <div className="cf-manager-list">
          {(routes[projectId] ?? []).map((route) => <div className="cf-manager-row" key={route.id}><div><a href={`https://${route.hostname}`} target="_blank" rel="noreferrer"><strong>{route.hostname}</strong></a><span>{route.serviceTarget}</span></div><StatusBadge status={route.enabled ? "enabled" : "disabled"} /><Button variant="danger" disabled={busy} onClick={() => setRemoveRoute(route)} icon={<Trash2 size={14} />}>Delete</Button></div>)}
          {!loading && !(routes[projectId] ?? []).length ? <p className="muted">No hostname configured for this project.</p> : null}
          {loading ? <LoadingInline label="Loading project hostnames…" /> : null}
        </div>
      </section>
      {removeRoute ? <ConfirmDialog title="Delete public hostname?" body={`Delete ${removeRoute.hostname} and its managed DNS record?`} confirmLabel="Delete hostname" danger onClose={() => setRemoveRoute(null)} onConfirm={() => void remove(removeRoute)} /> : null}
    </section>
  );
}
