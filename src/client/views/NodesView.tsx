import { Save } from "lucide-react";
import { memo, useCallback, useEffect, useState } from "react";
import type { DeploymentNode } from "../../shared/types";
import { dateTime } from "../app-utils";
import { Button, StatusBadge, TextField } from "../components/ui";
import { api, type FrpNodeAssignmentRecord, type FrpRole, type FrpServerRecord } from "../lib/api";

type Props = {
  nodes: DeploymentNode[];
};

type DestinationDraft = { sshHost: string; sshPort: string; sshUser: string; directory: string; privateKeyPath: string; saved: boolean };

function destinationFor(node: DeploymentNode): DestinationDraft {
  return {
    sshHost: node.labels["backup.sshHost"] ?? "",
    sshPort: node.labels["backup.sshPort"] ?? "22",
    sshUser: node.labels["backup.sshUser"] ?? "",
    directory: node.labels["backup.directory"] ?? "",
    privateKeyPath: node.labels["backup.privateKeyPath"] ?? "",
    saved: Boolean(node.labels["backup.sshHost"] && node.labels["backup.sshUser"] && node.labels["backup.directory"])
  };
}

export const NodesView = memo(function NodesView(props: Props) {
  const { nodes } = props;
  const [assignments, setAssignments] = useState<FrpNodeAssignmentRecord[]>([]);
  const [servers, setServers] = useState<FrpServerRecord[]>([]);
  const [drafts, setDrafts] = useState<Record<string, { role: FrpRole; serverId: string }>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [destinations, setDestinations] = useState<Record<string, DestinationDraft>>(() => Object.fromEntries(nodes.map((node) => [node.id, destinationFor(node)])));

  useEffect(() => {
    setDestinations((current) => Object.fromEntries(nodes.map((node) => [node.id, current[node.id] ?? destinationFor(node)])));
  }, [nodes]);

  const refreshFrp = useCallback(async () => {
    try {
      const [nextAssignments, nextServers] = await Promise.all([api.frpNodeAssignments(), api.frpServers()]);
      setAssignments(nextAssignments);
      setServers(nextServers);
      setDrafts(Object.fromEntries(nodes.map((node) => {
        const assignment = nextAssignments.find((candidate) => candidate.nodeId === node.id);
        return [node.id, { role: assignment?.role ?? "disabled", serverId: assignment?.serverId ?? "" }];
      })));
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Unable to load FRP node assignments.");
    }
  }, [nodes]);

  useEffect(() => { void refreshFrp(); }, [refreshFrp]);

  async function saveAssignment(nodeId: string) {
    const draft = drafts[nodeId] ?? { role: "disabled" as const, serverId: "" };
    setBusy(nodeId);
    try {
      await api.updateFrpNodeAssignment(nodeId, { role: draft.role, serverId: draft.serverId || null });
      await refreshFrp();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Unable to update FRP role.");
    } finally {
      setBusy(null);
    }
  }

  async function saveDestination(nodeId: string) {
    const draft = destinations[nodeId];
    if (!draft) return;
    setBusy(`destination:${nodeId}`);
    try {
      await api.updateNodeBackupDestination(nodeId, {
        sshHost: draft.sshHost.trim(),
        sshPort: Number(draft.sshPort || 22),
        sshUser: draft.sshUser.trim(),
        directory: draft.directory.trim(),
        privateKeyPath: draft.privateKeyPath.trim()
      });
      setDestinations((current) => ({ ...current, [nodeId]: { ...draft, saved: true } }));
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Unable to save backup destination.");
    } finally {
      setBusy(null);
    }
  }

  return (
    <section className="panel">
      <div className="panel-head">
        <div><h2>Deployment nodes</h2><p className="muted">Deployment and FRP roles are independent. A worker can be an FRP client, server, or both.</p></div>
        <span className="count">{nodes.length} registered</span>
      </div>
      <div className="table-wrap">
        <table>
          <thead>
            <tr>
              <th>Name</th>
              <th>Role</th>
              <th>FRP role</th>
              <th>FRP server</th>
              <th>FRP status</th>
              <th>Status</th>
              <th>Docker</th>
              <th>Projects</th>
              <th>Active</th>
              <th>Last seen</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {nodes.map((node) => {
              const assignment = assignments.find((candidate) => candidate.nodeId === node.id);
              const draft = drafts[node.id] ?? { role: assignment?.role ?? "disabled", serverId: assignment?.serverId ?? "" };
              const needsServer = draft.role === "client" || draft.role === "both";
              return (
              <tr key={node.id}>
                <td>{node.name}</td>
                <td>{node.role}</td>
                <td>
                  <select className="compact-native-select" aria-label={`FRP role for ${node.name}`} value={draft.role} onChange={(event) => setDrafts((current) => ({ ...current, [node.id]: { ...draft, role: event.target.value as FrpRole } }))}>
                    <option value="disabled">Disabled</option>
                    <option value="client">Client</option>
                    <option value="server">Server</option>
                    <option value="both">Both</option>
                  </select>
                </td>
                <td>
                  <select className="compact-native-select" aria-label={`FRP server for ${node.name}`} value={draft.serverId} disabled={!needsServer} onChange={(event) => setDrafts((current) => ({ ...current, [node.id]: { ...draft, serverId: event.target.value } }))}>
                    <option value="">Select server</option>
                    {servers.filter((server) => server.nodeId !== node.id || draft.role === "both").map((server) => <option value={server.id} key={server.id}>{server.nodeName ?? server.publicHost}</option>)}
                  </select>
                </td>
                <td><StatusBadge status={assignment?.status ?? (assignment?.role === "disabled" ? "disabled" : "pending")} />{assignment?.lastError ? <span className="node-frp-error" title={assignment.lastError}>{assignment.lastError}</span> : null}</td>
                <td><StatusBadge status={node.status} /></td>
                <td>{node.dockerVersion ?? "-"}</td>
                <td>{node.projectCount ?? 0}</td>
                <td>{node.runningDeploymentCount ?? 0}</td>
                <td>{node.lastSeenAt ? dateTime(node.lastSeenAt) : "-"}</td>
                <td><Button variant="secondary" loading={busy === node.id} disabled={needsServer && !draft.serverId} onClick={() => void saveAssignment(node.id)} icon={<Save size={14} />}>Apply</Button></td>
              </tr>
            );})}
          </tbody>
        </table>
      </div>
      {error ? <p className="form-error">{error}</p> : null}
      {!nodes.length ? <p className="muted">No nodes registered yet.</p> : null}
      {nodes.length ? <section className="node-destination-section">
        <div className="panel-head">
          <div><h3>Backup destinations</h3><p className="muted">SSH and rsync details used when another node copies a backup here.</p></div>
        </div>
        <div className="node-destination-grid">
          {nodes.map((node) => {
            const draft = destinations[node.id] ?? destinationFor(node);
            const update = (patch: Partial<DestinationDraft>) => setDestinations((current) => ({ ...current, [node.id]: { ...draft, ...patch, saved: false } }));
            const ready = Boolean(draft.sshHost.trim() && draft.sshUser.trim() && draft.directory.trim() && Number(draft.sshPort));
            return <article className="node-destination-card" key={node.id}>
              <header><div><strong>{node.name}</strong><span>{node.role}</span></div><StatusBadge status={draft.saved ? "configured" : "not-configured"} label={draft.saved ? "Configured" : "Not configured"} /></header>
              <div className="node-destination-fields">
                <TextField label="SSH host" value={draft.sshHost} onChange={(sshHost) => update({ sshHost })} placeholder="10.0.0.20 or VPS hostname" />
                <TextField label="Port" value={draft.sshPort} onChange={(sshPort) => update({ sshPort: sshPort.replace(/\D/g, "") })} />
                <TextField label="SSH user" value={draft.sshUser} onChange={(sshUser) => update({ sshUser })} placeholder="yanto-backup" />
                <TextField label="Destination directory" value={draft.directory} onChange={(directory) => update({ directory })} placeholder="/srv/yanto/backups" />
                <TextField label="Private key path" value={draft.privateKeyPath} onChange={(privateKeyPath) => update({ privateKeyPath })} placeholder="/run/secrets/backup_ssh_key" />
              </div>
              <div className="node-destination-footer"><span className="muted">For CGNAT nodes, use the VPS host and the SSH port forwarded by FRP.</span><Button variant="secondary" disabled={!ready} loading={busy === `destination:${node.id}`} onClick={() => void saveDestination(node.id)} icon={<Save size={14} />}>Save destination</Button></div>
            </article>;
          })}
        </div>
      </section> : null}
    </section>
  );
});
