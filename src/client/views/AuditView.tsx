import { ChevronLeft, ChevronRight } from "lucide-react";
import { memo } from "react";
import { pageSize, totalPages } from "../app-utils";
import { Button } from "../components/ui";
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

function Pagination({
  label,
  page,
  totalItems,
  onPageChange,
}: {
  label: string;
  page: number;
  totalItems: number;
  onPageChange: (page: number) => void;
}) {
  const pages = totalPages(Array.from({ length: totalItems }));
  if (totalItems <= pageSize) return null;

  const start = (page - 1) * pageSize + 1;
  const end = Math.min(page * pageSize, totalItems);

  return (
    <div className="pagination" aria-label={`${label} pagination`}>
      <span>
        {label} {start}-{end} of {totalItems}
      </span>
      <div>
        <Button variant="secondary" disabled={page <= 1} onClick={() => onPageChange(Math.max(1, page - 1))} icon={<ChevronLeft size={15} />}>
          Prev
        </Button>
        <span className="page-count">
          {page} / {pages}
        </span>
        <Button variant="secondary" disabled={page >= pages} onClick={() => onPageChange(Math.min(pages, page + 1))} icon={<ChevronRight size={15} />}>
          Next
        </Button>
      </div>
    </div>
  );
}
