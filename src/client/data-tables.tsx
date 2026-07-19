import { Archive, DatabaseZap, Download, Inbox, Play, RotateCw, ScrollText, Square, Terminal, Trash2, Upload } from "lucide-react";
import type { ContainerInfo, Deployment } from "../shared/types";
import { bytes, dateTime, deploymentChanges, durationBetween, durationSince, isProtectedYantoContainer, usedMemoryMb } from "./app-utils";
import { Button, IconButton, LoadingInline, StatusBadge } from "./components/ui";
import { api, type AuditLogEntry, type BackupRecord, type PostgresTarget } from "./lib/api";

function truncateInline(value: string, max = 42) {
  return value.length > max ? `${value.slice(0, max - 1)}…` : value;
}

function formatMetaValue(value: unknown): string {
  if (value === null || value === undefined) return "-";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

type ConfirmState = { title: string; body: string; label: string; danger?: boolean; action: () => Promise<void> };
type LoadingConfirmState = ConfirmState & { loadingMessage?: string; successMessage?: string };

export function PostgresTargetTable({
  targets,
  busy,
  loading,
  onDump,
  onRestore
}: {
  targets: PostgresTarget[];
  busy: string | null;
  loading?: boolean;
  onDump: (containerId: string) => Promise<void>;
  onRestore: (target: PostgresTarget, file: File) => Promise<void>;
}) {
  if (loading && !targets.length) {
    return (
      <div className="table-empty-state">
        <LoadingInline label="Loading Postgres targets..." />
      </div>
    );
  }

  if (!targets.length) {
    return (
      <div className="table-empty-state">
        <DatabaseZap size={24} />
        <p className="muted">No likely Postgres containers detected yet.</p>
      </div>
    );
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
                <td>
                  <span className="mono-cell">{target.containerName}</span>
                </td>
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
                  <Button disabled={!running} loading={busy === `backup:${target.containerId}`} onClick={() => void onDump(target.containerId)} icon={<Archive size={15} />}>
                    {busy === `backup:${target.containerId}` ? "Dumping" : "Dump"}
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
  loading,
  r2Ready,
  onDelete,
  onUploadR2
}: {
  backups: BackupRecord[];
  busy: string | null;
  loading?: boolean;
  r2Ready: boolean;
  onDelete: (backup: BackupRecord) => void;
  onUploadR2: (backup: BackupRecord) => Promise<void>;
}) {
  if (loading && !backups.length) {
    return (
      <div className="table-empty-state">
        <LoadingInline label="Loading backups..." />
      </div>
    );
  }

  if (!backups.length) {
    return (
      <div className="table-empty-state">
        <Archive size={24} />
        <p className="muted">No backups recorded yet.</p>
      </div>
    );
  }

  return (
    <div className="table-wrap backup-table-wrap">
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
              <td>
                <div className="stacked-cell">
                  <strong className="mono-cell">{backup.filename || backup.id}</strong>
                  {backup.error ? <span className="backup-error-note" title={backup.error}>{truncateInline(backup.error, 56)}</span> : null}
                </div>
              </td>
              <td>{backup.note ?? backup.kind}</td>
              <td>
                <StatusBadge status={backup.status} />
              </td>
              <td className="tabular-cell">{backup.fileSizeBytes ? bytes(backup.fileSizeBytes) : "-"}</td>
              <td title={dateTime(backup.createdAt)}>{dateTime(backup.createdAt)}</td>
              <td className="tabular-cell">{durationBetween(backup.createdAt, backup.finishedAt)}</td>
              <td className="table-actions">
                <a className={`button secondary link-button ${backup.status !== "success" ? "disabled" : ""}`} href={backup.status === "success" ? api.backupDownloadUrl(backup.id) : undefined}>
                  <Download size={15} />
                  <span>Download</span>
                </a>
                <Button disabled={backup.status !== "success" || !r2Ready} loading={busy === `r2:${backup.id}`} variant="secondary" onClick={() => void onUploadR2(backup)} icon={<Upload size={15} />}>
                  {busy === `r2:${backup.id}` ? "Uploading" : "R2"}
                </Button>
                <IconButton label="Remove backup" variant="danger" disabled={Boolean(busy?.startsWith(`r2:${backup.id}`))} onClick={() => onDelete(backup)}>
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
    return (
      <div className="audit-empty-state">
        <Inbox size={26} />
        <p className="muted">No audit events recorded yet.</p>
        <span>Actions taken across the dashboard will show up here.</span>
      </div>
    );
  }

  return (
    <div className="audit-table-wrap">
      <div className="audit-list">
        {entries.map((entry) => {
          const metaEntries = Object.entries(entry.metadata ?? {});
          const target = entry.entityId ? `${entry.entityType}:${entry.entityId}` : entry.entityType;
          return (
            <article className="audit-row" key={entry.id}>
              <div className="audit-row-time" title={dateTime(entry.createdAt)}>
                <strong>{durationSince(entry.createdAt)} ago</strong>
                <span>{dateTime(entry.createdAt)}</span>
              </div>
              <div className="audit-row-main">
                <div className="audit-row-head">
                  <span className="audit-actor">{entry.actor || "system"}</span>
                  <span className="audit-action">{entry.action}</span>
                  {target ? <span className="audit-target">{target}</span> : null}
                </div>
                {metaEntries.length ? (
                  <div className="audit-meta">
                    {metaEntries.map(([key, value]) => {
                      const display = formatMetaValue(value);
                      return (
                        <span className="audit-meta-chip" key={key} title={`${key}: ${display}`}>
                          <span className="audit-meta-key">{key}</span>
                          <span className="audit-meta-value">{truncateInline(display)}</span>
                        </span>
                      );
                    })}
                  </div>
                ) : (
                  <span className="audit-meta-empty">No additional metadata</span>
                )}
              </div>
              <div className="audit-row-status">
                <StatusBadge status="recorded" />
              </div>
            </article>
          );
        })}
      </div>
    </div>
  );
}

export function ContainerGroups({
  containers,
  loading,
  onLogs,
  onTerminal,
  onConfirm,
  onReload,
  canControl = () => true
}: {
  containers: ContainerInfo[];
  loading?: boolean;
  onLogs: (container: ContainerInfo) => void;
  onTerminal: (container: ContainerInfo) => void;
  onConfirm: (confirm: LoadingConfirmState) => void;
  onReload: () => Promise<void>;
  canControl?: (container: ContainerInfo) => boolean;
}) {
  const createdAtTime = (container: ContainerInfo) => container.createdAt ? Date.parse(container.createdAt) : 0;
  const groups = Array.from(
    containers.reduce((map, container) => {
      const key = container.composeProject || "standalone";
      map.set(key, [...(map.get(key) ?? []), container]);
      return map;
    }, new Map<string, ContainerInfo[]>())
  )
    .map(([group, rows]) => [group, [...rows].sort((a, b) => createdAtTime(b) - createdAtTime(a))] as const)
    .sort(([, aRows], [, bRows]) => createdAtTime(bRows[0]) - createdAtTime(aRows[0]));

  if (loading && !groups.length) {
    return <p className="muted">Loading containers...</p>;
  }

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
          <div className="container-list">
            {rows.map((container) => {
              const protectedContainer = isProtectedYantoContainer(container);
              const running = container.state === "running";
              const runtimeAllowed = canControl(container);
              return (
                <article className="container-row-card" key={container.id}>
                  <div className="container-row-main">
                    <div className="container-row-title">
                      <strong>{container.name}</strong>
                      <span>{container.image}</span>
                    </div>
                    <div className="container-row-meta">
                      <StatusBadge status={container.state} />
                      {container.isPostgresCandidate ? <StatusBadge status="postgres" /> : null}
                      <span title={dateTime(container.createdAt)}>Uptime {durationSince(container.createdAt)}</span>
                      <span>CPU {container.cpuPercent}</span>
                      <span>Memory {usedMemoryMb(container.memoryUsage)} ({container.memoryPercent})</span>
                    </div>
                    <div className="container-row-ports">{container.ports || "No published ports"}</div>
                  </div>
                  <div className="container-row-actions">
                    {protectedContainer ? (
                      <span className="protected-label">Protected</span>
                    ) : (
                      <>
                        <IconButton label="View logs" variant="secondary" onClick={() => void onLogs(container)}>
                          <ScrollText size={15} />
                        </IconButton>
                        {runtimeAllowed && running ? (
                          <IconButton label="Run command" variant="secondary" onClick={() => onTerminal(container)}>
                            <Terminal size={15} />
                          </IconButton>
                        ) : null}
                        {runtimeAllowed ? <IconButton
                          label="Restart container"
                          variant="secondary"
                          onClick={() =>
                            onConfirm({
                              title: "Restart container",
                              body: `Restart ${container.name}?`,
                              label: "Restart",
                              loadingMessage: `Restarting ${container.name}...`,
                              successMessage: "Container restarted.",
                              action: async () => {
                                await api.restartContainer(container.id);
                                await onReload();
                              }
                            })
                          }
                        >
                          <RotateCw size={15} />
                        </IconButton> : null}
                        {runtimeAllowed && running ? (
                          <IconButton
                            label="Stop container"
                            variant="danger"
                            onClick={() =>
                              onConfirm({
                                title: "Stop container",
                                body: `Stop ${container.name}?`,
                                label: "Stop",
                                danger: true,
                                loadingMessage: `Stopping ${container.name}...`,
                                successMessage: "Container stopped.",
                                action: async () => {
                                  await api.stopContainer(container.id);
                                  await onReload();
                                }
                              })
                            }
                          >
                            <Square size={15} />
                          </IconButton>
                        ) : runtimeAllowed ? (
                          <IconButton
                            label="Start container"
                            variant="secondary"
                            onClick={() =>
                              onConfirm({
                                title: "Start container",
                                body: `Start ${container.name}?`,
                                label: "Start",
                                loadingMessage: `Starting ${container.name}...`,
                                successMessage: "Container started.",
                                action: async () => {
                                  await api.startContainer(container.id);
                                  await onReload();
                                }
                              })
                            }
                          >
                            <Play size={15} />
                          </IconButton>
                        ) : null}
                      </>
                    )}
                  </div>
                </article>
              );
            })}
          </div>
        </details>
      ))}
    </div>
  );
}

export function DeploymentTable({
  deployments,
  busy,
  loading,
  onLogs,
  onRetry,
  compact,
  canRetry = () => true
}: {
  deployments: Deployment[];
  busy?: string | null;
  loading?: boolean;
  onLogs: (deployment: Deployment) => void;
  onRetry?: (deployment: Deployment) => void;
  compact?: boolean;
  canRetry?: (deployment: Deployment) => boolean;
}) {
  if (loading && !deployments.length) {
    return <p className="muted">Loading deployments...</p>;
  }

  if (!deployments.length) {
    return <p className="muted">No deployments recorded yet.</p>;
  }

  return (
    <div className="table-wrap">
      <table>
        <thead>
          <tr>
            <th>Project</th>
            <th>Node</th>
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
              <td>{deployment.nodeName ?? deployment.nodeId}</td>
              <td>{deployment.trigger}</td>
              <td>{dateTime(deployment.startedAt)}</td>
              <td>
                <StatusBadge status={deployment.status} />
              </td>
              <td>{durationBetween(deployment.startedAt, deployment.finishedAt)}</td>
              {!compact ? <td>{deploymentChanges(deployment)}</td> : null}
              <td className="table-actions">
                {!compact && deployment.status === "failed" && onRetry && canRetry(deployment) ? (
                  <Button variant="secondary" loading={busy === `deploy:${deployment.projectId}`} onClick={() => onRetry(deployment)} icon={<RotateCw size={15} />}>
                    Retry
                  </Button>
                ) : null}
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
