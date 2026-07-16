import { Archive, Clock3, Plus, RotateCw, Trash2 } from "lucide-react";
import { memo, useCallback, useEffect, useMemo, useState } from "react";
import type { FormEvent } from "react";
import type { Backup, DeploymentNode } from "../../shared/types";
import { dateTime } from "../app-utils";
import { Button, CustomSelect, IconButton, StatusBadge, TextField, ToggleField } from "../components/ui";
import { Pagination } from "../components/Pagination";
import { BackupTable, PostgresTargetTable } from "../data-tables";
import { api, type BackupPolicyRecord, type PostgresTarget } from "../lib/api";
import type { ConfirmState } from "./types";

type Props = {
  isOwner: boolean;
  postgresTargets: PostgresTarget[];
  nodes: DeploymentNode[];
  localNodeId: string;
  visibleBackups: Backup[];
  backups: Backup[];
  busy: string | null;
  loading?: boolean;
  r2Ready: boolean;
  backupPage: number;
  dumpPostgresTarget: (containerId?: string, sourceNodeId?: string) => Promise<void>;
  restorePostgresTarget: (target: PostgresTarget, file: File) => Promise<void>;
  uploadBackupR2: (backup: Backup) => Promise<void>;
  setConfirm: (state: ConfirmState) => void;
  refreshBackups: () => Promise<void>;
  setBackupPage: (page: number) => void;
  toast: (message: string, kind?: "ok" | "error" | "loading") => void;
};

type PolicyForm = {
  name: string;
  targetKey: string;
  destinationNodeIds: string[];
  enabled: boolean;
  hourlyAtMinute: string;
  hourlyRetention: string;
  dailyRetention: string;
};

const emptyPolicy: PolicyForm = {
  name: "",
  targetKey: "",
  destinationNodeIds: [],
  enabled: true,
  hourlyAtMinute: "0",
  hourlyRetention: "24",
  dailyRetention: "30"
};

function targetKey(target: PostgresTarget) {
  return `${target.nodeId ?? "local"}::${target.containerId}`;
}

export const BackupsView = memo(function BackupsView(props: Props) {
  const {
    isOwner,
    postgresTargets,
    nodes,
    localNodeId,
    visibleBackups,
    backups,
    busy,
    loading,
    r2Ready,
    backupPage,
    dumpPostgresTarget,
    restorePostgresTarget,
    uploadBackupR2,
    setConfirm,
    refreshBackups,
    setBackupPage,
    toast,
  } = props;
  const [policies, setPolicies] = useState<BackupPolicyRecord[]>([]);
  const [policyForm, setPolicyForm] = useState<PolicyForm>(emptyPolicy);
  const [policyBusy, setPolicyBusy] = useState<string | null>(null);
  const [policiesLoading, setPoliciesLoading] = useState(true);

  const refreshPolicies = useCallback(async () => {
    try {
      setPolicies(await api.backupPolicies());
    } catch (error) {
      toast(error instanceof Error ? error.message : "Unable to load backup policies.", "error");
    } finally {
      setPoliciesLoading(false);
    }
  }, [toast]);

  useEffect(() => { void refreshPolicies(); }, [refreshPolicies]);

  const targetOptions = useMemo(() => postgresTargets.map((target) => ({
    value: targetKey(target),
    label: `${target.nodeName ?? "Local master"} · ${target.projectName ?? target.composeProject ?? target.containerName} · ${target.databaseName}`
  })), [postgresTargets]);
  const selectedPolicyTarget = postgresTargets.find((target) => targetKey(target) === policyForm.targetKey);

  async function createPolicy(event: FormEvent) {
    event.preventDefault();
    const target = postgresTargets.find((candidate) => targetKey(candidate) === policyForm.targetKey);
    if (!target) return;
    setPolicyBusy("create");
    try {
      await api.createBackupPolicy({
        name: policyForm.name.trim(),
        sourceNodeId: target.nodeId ?? nodes.find((node) => node.role === "master")?.id ?? "local",
        targetContainerId: target.containerId,
        enabled: policyForm.enabled,
        hourlyAtMinute: Number(policyForm.hourlyAtMinute),
        hourlyRetention: Number(policyForm.hourlyRetention),
        dailyRetention: Number(policyForm.dailyRetention),
        destinationNodeIds: policyForm.destinationNodeIds
      });
      setPolicyForm(emptyPolicy);
      await refreshPolicies();
      toast("Backup policy created.");
    } catch (error) {
      toast(error instanceof Error ? error.message : "Unable to create backup policy.", "error");
    } finally {
      setPolicyBusy(null);
    }
  }

  async function togglePolicy(policy: BackupPolicyRecord) {
    setPolicyBusy(policy.id);
    try {
      await api.updateBackupPolicy(policy.id, { enabled: !policy.enabled });
      await refreshPolicies();
    } catch (error) {
      toast(error instanceof Error ? error.message : "Unable to update backup policy.", "error");
    } finally {
      setPolicyBusy(null);
    }
  }

  async function retryReplica(replicaId: string) {
    setPolicyBusy(`replica:${replicaId}`);
    try {
      await api.retryBackupReplica(replicaId);
      await refreshBackups();
      toast("Backup copy queued for retry.");
    } catch (error) {
      toast(error instanceof Error ? error.message : "Unable to retry backup copy.", "error");
    } finally {
      setPolicyBusy(null);
    }
  }

  return (
    <div className="backup-layout">
      <section className="panel">
        <div className="panel-head">
          <h2>Postgres targets</h2>
          <div className="actions">
            <span className="count">{postgresTargets.length} detected</span>
            {isOwner ? <Button loading={busy === "backup:yanto"} onClick={() => void dumpPostgresTarget()} icon={<Archive size={16} />}>
              {busy === "backup:yanto" ? "Dumping" : "Dump Yanto DB"}
            </Button> : null}
          </div>
        </div>
        <PostgresTargetTable targets={postgresTargets} busy={busy} loading={loading} localNodeId={localNodeId} onDump={dumpPostgresTarget} onRestore={restorePostgresTarget} />
      </section>
      {isOwner ? <section className="panel backup-policy-panel">
        <div className="panel-head">
          <div>
            <h2>Automatic backup policies</h2>
            <p className="muted">Create an hourly dump on its source node, then copy it to every selected destination.</p>
          </div>
          <span className="count">{policiesLoading ? "Loading" : `${policies.length} policies`}</span>
        </div>
        <form className="backup-policy-form" onSubmit={createPolicy}>
          <TextField label="Policy name" value={policyForm.name} onChange={(name) => setPolicyForm((current) => ({ ...current, name }))} placeholder="Homeserver Postgres" required />
          <CustomSelect label="Postgres target" value={policyForm.targetKey} options={targetOptions} onChange={(value) => {
            const target = postgresTargets.find((candidate) => targetKey(candidate) === value);
            setPolicyForm((current) => ({ ...current, targetKey: value, destinationNodeIds: current.destinationNodeIds.filter((id) => id !== target?.nodeId) }));
          }} />
          <div className="backup-policy-number-grid">
            <TextField label="Minute each hour" value={policyForm.hourlyAtMinute} onChange={(value) => setPolicyForm((current) => ({ ...current, hourlyAtMinute: value.replace(/\D/g, "").slice(0, 2) }))} placeholder="0" />
            <TextField label="Hourly copies" value={policyForm.hourlyRetention} onChange={(value) => setPolicyForm((current) => ({ ...current, hourlyRetention: value.replace(/\D/g, "") }))} />
            <TextField label="Daily copies" value={policyForm.dailyRetention} onChange={(value) => setPolicyForm((current) => ({ ...current, dailyRetention: value.replace(/\D/g, "") }))} />
          </div>
          <fieldset className="backup-destination-picker">
            <legend>Copy to nodes</legend>
            {nodes.map((node) => (
              <label key={node.id}>
                <input type="checkbox" disabled={node.id === selectedPolicyTarget?.nodeId} checked={policyForm.destinationNodeIds.includes(node.id)} onChange={(event) => setPolicyForm((current) => ({
                  ...current,
                  destinationNodeIds: event.target.checked ? [...current.destinationNodeIds, node.id] : current.destinationNodeIds.filter((id) => id !== node.id)
                }))} />
                <span><strong>{node.name}</strong><small>{node.id === selectedPolicyTarget?.nodeId ? "Source node" : `${node.status} · ${node.role}`}</small></span>
              </label>
            ))}
            {!nodes.length ? <p className="muted">Register another node before configuring replication.</p> : null}
          </fieldset>
          <div className="backup-policy-form-footer">
            <ToggleField label="Enabled" value={policyForm.enabled} onChange={(enabled) => setPolicyForm((current) => ({ ...current, enabled }))} description="Run once every hour." />
            <Button type="submit" loading={policyBusy === "create"} disabled={!policyForm.name.trim() || !policyForm.targetKey || !policyForm.destinationNodeIds.length || Number(policyForm.hourlyAtMinute) > 59 || Number(policyForm.hourlyRetention) < 1 || Number(policyForm.dailyRetention) < 1} icon={<Plus size={15} />}>Create policy</Button>
          </div>
        </form>
        <div className="backup-policy-list">
          {policies.map((policy) => {
            const source = nodes.find((node) => node.id === policy.sourceNodeId);
            const destinations = policy.destinationNodeIds.map((id) => nodes.find((node) => node.id === id)?.name ?? id);
            return <article className="backup-policy-card" key={policy.id}>
              <div>
                <div className="backup-policy-title"><strong>{policy.name}</strong><StatusBadge status={policy.enabled ? "enabled" : "disabled"} /></div>
                <p>{source?.name ?? policy.sourceNodeId} → {destinations.join(", ") || "no destinations"}</p>
                <span><Clock3 size={13} /> Every hour at :{String(policy.hourlyAtMinute).padStart(2, "0")} · keep {policy.hourlyRetention} hourly / {policy.dailyRetention} daily</span>
                {policy.lastRunAt ? <span>Last run {dateTime(policy.lastRunAt)}{policy.nextRunAt ? ` · next ${dateTime(policy.nextRunAt)}` : ""}</span> : null}
              </div>
              <div className="table-actions icon-actions">
                <IconButton label={policy.enabled ? "Pause policy" : "Enable policy"} disabled={policyBusy === policy.id} onClick={() => void togglePolicy(policy)}>{policy.enabled ? <RotateCw size={14} /> : <Archive size={14} />}</IconButton>
                <IconButton label="Delete policy" variant="danger" onClick={() => setConfirm({
                  title: "Delete backup policy",
                  body: `Delete ${policy.name}? Existing backup files and replicas are kept.`,
                  label: "Delete",
                  danger: true,
                  action: async () => { await api.deleteBackupPolicy(policy.id); await refreshPolicies(); }
                })}><Trash2 size={14} /></IconButton>
              </div>
            </article>;
          })}
          {!policiesLoading && !policies.length ? <p className="muted">No automatic backup policies yet.</p> : null}
        </div>
      </section> : null}
      <section className="panel">
        <div className="panel-head">
          <h2>Backup history</h2>
          <span className="count">{backups.length} dumps</span>
        </div>
        <BackupTable
          backups={visibleBackups}
          busy={policyBusy?.startsWith("replica:") ? policyBusy : busy}
          loading={loading}
          r2Ready={r2Ready}
          onUploadR2={uploadBackupR2}
          onRetryReplica={retryReplica}
          onDelete={(backup) =>
            setConfirm({
              title: "Remove backup",
              body: `Remove ${backup.filename || backup.id}? The dump file will be deleted from disk.`,
              label: "Remove",
              danger: true,
              loadingMessage: "Removing backup...",
              successMessage: "Backup removed.",
              action: async () => {
                await api.deleteBackup(backup.id);
                await refreshBackups();
              },
            })
          }
        />
        <Pagination label="Backups" page={backupPage} totalItems={backups.length} onPageChange={setBackupPage} />
      </section>
    </div>
  );
});
