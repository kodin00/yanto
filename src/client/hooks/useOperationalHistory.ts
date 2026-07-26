import { useCallback, useState, type Dispatch, type SetStateAction } from "react";
import type { AuditLog, Backup, Deployment, PostgresBackupTarget } from "../../shared/types";
import { pageSize } from "../app-utils";
import { api } from "../lib/api";

type HistoryView = "deployments" | "backups" | "audit";
type ViewLoading = Partial<Record<HistoryView, boolean>>;
type ViewErrors = Partial<Record<HistoryView, string | undefined>>;

type Options<L extends ViewLoading, E extends ViewErrors> = {
  setViewLoading: Dispatch<SetStateAction<L>>;
  setViewErrors: Dispatch<SetStateAction<E>>;
  onError: (message: string) => void;
  onPageLoaded: (view: HistoryView) => void;
};

export function useOperationalHistory<L extends ViewLoading, E extends ViewErrors>({ setViewLoading, setViewErrors, onError, onPageLoaded }: Options<L, E>) {
  const [deploymentRows, setDeploymentRows] = useState<Deployment[]>([]);
  const [deploymentTotal, setDeploymentTotal] = useState(0);
  const [deploymentPage, setDeploymentPage] = useState(1);
  const [backups, setBackups] = useState<Backup[]>([]);
  const [backupTotal, setBackupTotal] = useState(0);
  const [backupPage, setBackupPage] = useState(1);
  const [postgresTargets, setPostgresTargets] = useState<PostgresBackupTarget[]>([]);
  const [auditEntries, setAuditEntries] = useState<AuditLog[]>([]);
  const [auditTotal, setAuditTotal] = useState(0);
  const [auditPage, setAuditPage] = useState(1);

  const loadDeployments = useCallback(async (page = deploymentPage) => {
    const result = await api.deploymentsPage(page, pageSize);
    setDeploymentRows(result.items);
    setDeploymentTotal(result.total);
  }, [deploymentPage]);

  const loadBackups = useCallback(async (page = backupPage, includeTargets = true) => {
    const [result, targets] = await Promise.all([
      api.backupsPage(page, pageSize),
      includeTargets ? api.postgresBackupTargets() : Promise.resolve<PostgresBackupTarget[] | null>(null)
    ]);
    setBackups(result.items);
    setBackupTotal(result.total);
    if (targets) setPostgresTargets(targets);
  }, [backupPage]);

  const loadAudit = useCallback(async (page = auditPage) => {
    const result = await api.auditLogPage(page, pageSize);
    setAuditEntries(result.items);
    setAuditTotal(result.total);
  }, [auditPage]);

  const refresh = useCallback(async (view: HistoryView, action: () => Promise<void>) => {
    setViewLoading((current) => ({ ...current, [view]: true } as L));
    try {
      await action();
      setViewErrors((current) => ({ ...current, [view]: undefined } as E));
    } catch (error) {
      const message = error instanceof Error ? error.message : `Unable to load ${view}.`;
      setViewErrors((current) => ({ ...current, [view]: message } as E));
      throw error;
    } finally {
      setViewLoading((current) => ({ ...current, [view]: false } as L));
    }
  }, [setViewErrors, setViewLoading]);

  const refreshDeployments = useCallback(() => refresh("deployments", () => loadDeployments()), [loadDeployments, refresh]);
  const refreshBackups = useCallback(() => refresh("backups", () => loadBackups()), [loadBackups, refresh]);

  const selectDeploymentPage = useCallback(async (page: number) => {
    try {
      await refresh("deployments", () => loadDeployments(page));
      setDeploymentPage(page);
      onPageLoaded("deployments");
    } catch (error) {
      onError(error instanceof Error ? error.message : "Unable to load deployments.");
    }
  }, [loadDeployments, onError, onPageLoaded, refresh]);

  const selectBackupPage = useCallback(async (page: number) => {
    try {
      await refresh("backups", () => loadBackups(page, false));
      setBackupPage(page);
      onPageLoaded("backups");
    } catch (error) {
      onError(error instanceof Error ? error.message : "Unable to load backups.");
    }
  }, [loadBackups, onError, onPageLoaded, refresh]);

  const selectAuditPage = useCallback(async (page: number) => {
    try {
      await refresh("audit", () => loadAudit(page));
      setAuditPage(page);
      onPageLoaded("audit");
    } catch (error) {
      onError(error instanceof Error ? error.message : "Unable to load audit logs.");
    }
  }, [loadAudit, onError, onPageLoaded, refresh]);

  return {
    deploymentRows,
    deploymentTotal,
    deploymentPage,
    backups,
    backupTotal,
    backupPage,
    postgresTargets,
    auditEntries,
    auditTotal,
    auditPage,
    loadDeployments,
    loadBackups,
    loadAudit,
    refreshDeployments,
    refreshBackups,
    selectDeploymentPage,
    selectBackupPage,
    selectAuditPage
  };
}
