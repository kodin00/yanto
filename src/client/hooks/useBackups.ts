import { useCallback, useEffect, useMemo, useState } from "react";
import type { Backup } from "../../shared/types";
import { pageItems, totalPages } from "../app-utils";
import { api, type PostgresTarget } from "../lib/api";
import { useToast } from "../contexts/ToastContext";
import { useConfirm } from "../contexts/ConfirmContext";

export function useBackups() {
  const [backups, setBackups] = useState<Backup[]>([]);
  const [postgresTargets, setPostgresTargets] = useState<PostgresTarget[]>([]);
  const [backupPage, setBackupPage] = useState(1);
  const [busy, setBusy] = useState<string | null>(null);
  const { showToast } = useToast();
  const { showConfirm } = useConfirm();

  const visibleBackups = useMemo(() => pageItems(backups, backupPage), [backupPage, backups]);

  useEffect(() => {
    setBackupPage((page) => Math.min(page, totalPages(backups)));
  }, [backups]);

  const r2Ready = false; // will be set via settings

  const refreshBackups = useCallback(async () => {
    const [backupRows, postgresRows] = await Promise.all([
      api.backups().catch(() => []),
      api.postgresBackupTargets().catch(() => [])
    ]);
    setBackups(backupRows);
    setPostgresTargets(postgresRows);
  }, []);

  async function dumpPostgresTarget(containerId?: string) {
    const busyKey = containerId ? `backup:${containerId}` : "backup:yanto";
    setBusy(busyKey);
    showToast("Creating Postgres backup...", "loading");
    try {
      await api.createBackup(containerId);
      await refreshBackups();
      showToast("Postgres backup created.");
    } catch (error) {
      showToast(error instanceof Error ? error.message : "Unable to create backup.", "error");
    } finally {
      setBusy(null);
    }
  }

  async function restorePostgresTarget(target: PostgresTarget, file: File) {
    showConfirm({
      title: "Restore Postgres dump",
      body: `Replace ${target.databaseName} on ${target.containerName} with ${file.name}? The current public schema will be dropped before importing the dump.`,
      label: "Restore",
      danger: true,
      action: async () => {
        setBusy(`restore:${target.containerId}`);
        showToast("Restoring Postgres dump...", "loading");
        try {
          await api.restorePostgresTarget(target.containerId, file);
          await refreshBackups();
          showToast("Postgres dump restored.");
        } finally {
          setBusy(null);
        }
      }
    });
  }

  async function uploadBackupR2(backup: Backup) {
    setBusy(`r2:${backup.id}`);
    showToast("Uploading dump to Cloudflare R2...", "loading");
    try {
      const result = await api.uploadBackupToR2(backup.id);
      showToast(`Uploaded to R2: ${result.key}`);
    } catch (error) {
      showToast(error instanceof Error ? error.message : "Unable to upload to R2.", "error");
    } finally {
      setBusy(null);
    }
  }

  return {
    backups, postgresTargets, visibleBackups, backupPage, setBackupPage, busy, r2Ready,
    refreshBackups, dumpPostgresTarget, restorePostgresTarget, uploadBackupR2
  };
}
