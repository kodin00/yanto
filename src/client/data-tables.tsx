import { Archive, Download, RotateCw, ScrollText, Square, Trash2, Upload } from "lucide-react";
import type { ContainerInfo, Deployment } from "../shared/types";
import { bytes, dateTime, deploymentChanges, durationBetween, durationSince, isProtectedYantoContainer, usedMemoryMb } from "./app-utils";
import { Button, IconButton, StatusBadge } from "./components/ui";
import { api, type AuditLogEntry, type BackupRecord, type PostgresTarget } from "./lib/api";

type ConfirmState = { title: string; body: string; label: string; danger?: boolean; action: () => Promise<void> };

export function PostgresTargetTable({
  targets,
  busy,
  onDump,
  onRestore
}: {
  targets: PostgresTarget[];
  busy: string | null;
  onDump: (containerId: string) => Promise<void>;
  onRestore: (target: PostgresTarget, file: File) => Promise<void>;
}) {
  if (!targets.length) {
    return <p className="muted">No likely Postgres containers detected yet.</p>;
  }

  return (
    <div className="table-wrap postgres-target-wrap">
      <table>
        <thead>
          <tr>
            <th>Project</th>
            <th>Container</th>
            <th>Database</th>
            <th>Status</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {targets.map((target) => {
            const running = target.state === "running";
            return (
              <tr key={target.containerId}>
                <td>
                  <div className="stacked-cell">
                    <strong>{target.projectName ?? target.composeProject ?? "Standalone"}</strong>
                    <span>{target.composeService ?? target.image}</span>
                  </div>
                </td>
                <td>{target.containerName}</td>
                <td>
                  <div className="stacked-cell">
                    <strong>{target.databaseName}</strong>
                    <span>{target.databaseUser}</span>
                  </div>
                </td>
                <td>
                  <StatusBadge status={running ? "postgres" : target.state} />
                </td>
                <td className="table-actions">
                  <Button disabled={!running || busy === `backup:${target.containerId}`} onClick={() => void onDump(target.containerId)} icon={<Archive size={15} />}>
                    Dump
                  </Button>
                  <label className={`button secondary file-button ${!running || busy === `restore:${target.containerId}` ? "disabled" : ""}`}>
                    <Upload size={15} />
                    <span>{busy === `restore:${target.containerId}` ? "Restoring" : "Restore"}</span>
                    <input
                      type="file"
                      accept=".sql,.gz,.dump,.backup,application/sql,application/gzip,application/octet-stream"
                      disabled={!running || busy === `restore:${target.containerId}`}
                      onChange={(event) => {
                        const file = event.target.files?.[0];
                        event.currentTarget.value = "";
                        if (file) {
                          void onRestore(target, file);
                        }
                      }}
                    />
                  </label>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

export function BackupTable({
  backups,
  busy,
  r2Ready,
  onDelete,
  onUploadR2
}: {
  backups: BackupRecord[];
  busy: string | null;
  r2Ready: boolean;
  onDelete: (backup: BackupRecord) => void;
  onUploadR2: (backup: BackupRecord) => Promise<void>;
}) {
  if (!backups.length) {
    return <p className="muted">No backups recorded yet.</p>;
  }

  return (
    <div className="table-wrap">
      <table>
        <thead>
          <tr>
            <th>Backup</th>
            <th>Source</th>
            <th>Status</th>
            <th>Size</th>
            <th>Created</th>
            <th>Duration</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {backups.map((backup) => (
            <tr key={backup.id}>
              <td>{backup.filename || backup.id}</td>
              <td>{backup.note ?? backup.kind}</td>
              <td>
                <StatusBadge status={backup.status} />
              </td>
              <td>{backup.fileSizeBytes ? bytes(backup.fileSizeBytes) : "-"}</td>
              <td>{dateTime(backup.createdAt)}</td>
              <td>{durationBetween(backup.createdAt, backup.finishedAt)}</td>
              <td className="table-actions">
                <a className={`button secondary link-button ${backup.status !== "success" ? "disabled" : ""}`} href={backup.status === "success" ? api.backupDownloadUrl(backup.id) : undefined}>
                  <Download size={15} />
                  <span>Download</span>
                </a>
                <Button disabled={backup.status !== "success" || !r2Ready || busy === `r2:${backup.id}`} variant="secondary" onClick={() => void onUploadR2(backup)} icon={<Upload size={15} />}>
                  {busy === `r2:${backup.id}` ? "Uploading" : "R2"}
                </Button>
                <IconButton label="Remove backup" variant="danger" onClick={() => onDelete(backup)}>
                  <Trash2 size={15} />
                </IconButton>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function AuditTable({ entries }: { entries: AuditLogEntry[] }) {
  if (!entries.length) {
    return <p className="muted">No audit events recorded yet.</p>;
  }

  return (
    <div className="table-wrap">
      <table>
        <thead>
          <tr>
            <th>Time</th>
            <th>Actor</th>
            <th>Action</th>
            <th>Target</th>
            <th>Status</th>
            <th>Message</th>
          </tr>
        </thead>
        <tbody>
          {entries.map((entry) => (
            <tr key={entry.id}>
              <td>{dateTime(entry.createdAt)}</td>
              <td>{entry.actor ?? "system"}</td>
              <td>{entry.action}</td>
              <td>{entry.entityId ? `${entry.entityType}:${entry.entityId}` : entry.entityType}</td>
              <td>
                <StatusBadge status="recorded" />
              </td>
              <td>{JSON.stringify(entry.metadata ?? {})}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function ContainerGroups({
  containers,
  onLogs,
  onConfirm,
  onReload
}: {
  containers: ContainerInfo[];
  onLogs: (container: ContainerInfo) => void;
  onConfirm: (confirm: ConfirmState) => void;
  onReload: () => Promise<void>;
}) {
  const groups = Array.from(
    containers.reduce((map, container) => {
      const key = container.composeProject || "standalone";
      map.set(key, [...(map.get(key) ?? []), container]);
      return map;
    }, new Map<string, ContainerInfo[]>())
  ).sort(([a], [b]) => a.localeCompare(b));

  if (!groups.length) {
    return <p className="muted">No containers found yet.</p>;
  }

  return (
    <div className="container-groups">
      {groups.map(([group, rows]) => (
        <details key={group} open>
          <summary>
            <span>{group}</span>
            <small>
              {rows.filter((container) => container.state === "running").length} / {rows.length} running
            </small>
          </summary>
          <div className="table-wrap">
            <table>
              <thead>
                <tr>
                  <th>Name</th>
                  <th>Image</th>
                  <th>Role</th>
                  <th>Status</th>
                  <th>Uptime</th>
                  <th>Ports</th>
                  <th>CPU</th>
                  <th>Memory</th>
                  <th></th>
                </tr>
              </thead>
              <tbody>
                {rows.map((container) => {
                  const protectedContainer = isProtectedYantoContainer(container);
                  return (
                    <tr key={container.id}>
                      <td>{container.name}</td>
                      <td>{container.image}</td>
                      <td>{container.isPostgresCandidate ? <StatusBadge status="postgres" /> : "-"}</td>
                      <td>
                        <StatusBadge status={container.state} />
                      </td>
                      <td title={dateTime(container.createdAt)}>{durationSince(container.createdAt)}</td>
                      <td className="ports-cell">{container.ports || "-"}</td>
                      <td>{container.cpuPercent}</td>
                      <td>
                        {usedMemoryMb(container.memoryUsage)} ({container.memoryPercent})
                      </td>
                      <td className="action-cell">
                        {protectedContainer ? (
                          <span className="protected-label">Protected</span>
                        ) : (
                          <div className="table-actions icon-actions">
                            <IconButton label="View logs" variant="secondary" onClick={() => void onLogs(container)}>
                              <ScrollText size={15} />
                            </IconButton>
                            <IconButton
                              label="Restart container"
                              variant="secondary"
                              onClick={() =>
                                onConfirm({
                                  title: "Restart container",
                                  body: `Restart ${container.name}?`,
                                  label: "Restart",
                                  action: async () => {
                                    await api.restartContainer(container.id);
                                    await onReload();
                                  }
                                })
                              }
                            >
                              <RotateCw size={15} />
                            </IconButton>
                            <IconButton
                              label="Stop container"
                              variant="danger"
                              onClick={() =>
                                onConfirm({
                                  title: "Stop container",
                                  body: `Stop ${container.name}?`,
                                  label: "Stop",
                                  danger: true,
                                  action: async () => {
                                    await api.stopContainer(container.id);
                                    await onReload();
                                  }
                                })
                              }
                            >
                              <Square size={15} />
                            </IconButton>
                          </div>
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </details>
      ))}
    </div>
  );
}

export function DeploymentTable({ deployments, onLogs, compact }: { deployments: Deployment[]; onLogs: (deployment: Deployment) => void; compact?: boolean }) {
  if (!deployments.length) {
    return <p className="muted">No deployments recorded yet.</p>;
  }

  return (
    <div className="table-wrap">
      <table>
        <thead>
          <tr>
            <th>Project</th>
            <th>Trigger</th>
            <th>Started</th>
            <th>Status</th>
            <th>Duration</th>
            {!compact ? <th>Changes</th> : null}
            <th></th>
          </tr>
        </thead>
        <tbody>
          {deployments.map((deployment) => (
            <tr key={deployment.id}>
              <td>{deployment.projectName ?? deployment.projectId}</td>
              <td>{deployment.trigger}</td>
              <td>{dateTime(deployment.startedAt)}</td>
              <td>
                <StatusBadge status={deployment.status} />
              </td>
              <td>{durationBetween(deployment.startedAt, deployment.finishedAt)}</td>
              {!compact ? <td>{deploymentChanges(deployment)}</td> : null}
              <td className="table-actions">
                <Button variant="secondary" onClick={() => onLogs(deployment)}>
                  Logs
                </Button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
