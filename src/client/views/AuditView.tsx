import { memo } from "react";
import { Pagination } from "../components/Pagination";
import { AuditTable } from "../data-tables";
import type { AuditLogEntry } from "../lib/api";

type Props = {
  auditEntries: AuditLogEntry[];
  visibleAuditEntries: AuditLogEntry[];
  auditPage: number;
  setAuditPage: (page: number) => void;
};

export const AuditView = memo(function AuditView(props: Props) {
  const { auditEntries, visibleAuditEntries, auditPage, setAuditPage } = props;

  return (
    <section className="panel">
      <div className="panel-head">
        <h2>Audit log</h2>
        <span className="count">{auditEntries.length} events</span>
      </div>
      <AuditTable entries={visibleAuditEntries} />
      <Pagination label="Audit events" page={auditPage} totalItems={auditEntries.length} onPageChange={setAuditPage} />
    </section>
  );
});
