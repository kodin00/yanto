import { memo } from "react";
import { Pagination } from "../components/Pagination";
import { AuditTable } from "../data-tables";
import type { AuditLogEntry } from "../lib/api";

type Props = {
  visibleAuditEntries: AuditLogEntry[];
  totalItems: number;
  loading?: boolean;
  auditPage: number;
  setAuditPage: (page: number) => void;
};

export const AuditView = memo(function AuditView(props: Props) {
  const { visibleAuditEntries, totalItems, loading, auditPage, setAuditPage } = props;

  return (
    <section className="panel audit-panel">
      <div className="panel-head">
        <h2>Logs</h2>
        <span className="count">{totalItems} events</span>
      </div>
      <AuditTable entries={visibleAuditEntries} />
      <Pagination label="Log events" page={auditPage} totalItems={totalItems} disabled={loading} onPageChange={setAuditPage} />
    </section>
  );
});
