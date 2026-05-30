import { useCallback, useEffect, useMemo, useState } from "react";
import { pageItems, totalPages } from "../app-utils";
import { api, type AuditLogEntry } from "../lib/api";

export function useAuditLog() {
  const [auditEntries, setAuditEntries] = useState<AuditLogEntry[]>([]);
  const [auditPage, setAuditPage] = useState(1);

  const visibleAuditEntries = useMemo(() => pageItems(auditEntries, auditPage), [auditEntries, auditPage]);

  useEffect(() => {
    setAuditPage((page) => Math.min(page, totalPages(auditEntries)));
  }, [auditEntries]);

  const refreshAuditLog = useCallback(async () => {
    setAuditEntries(await api.auditLog().catch(() => []));
  }, []);

  return {
    auditEntries, setAuditEntries, auditPage, setAuditPage,
    visibleAuditEntries, refreshAuditLog
  };
}
