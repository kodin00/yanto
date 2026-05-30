import { Archive, ChevronLeft, ChevronRight } from "lucide-react";
import { memo } from "react";
import type { Backup } from "../../shared/types";
import { pageSize, totalPages } from "../app-utils";
import { Button } from "../components/ui";
import { BackupTable, PostgresTargetTable } from "../data-tables";
import { api, type PostgresTarget } from "../lib/api";
import type { ConfirmState } from "./types";

type Props = {
  postgresTargets: PostgresTarget[];
  visibleBackups: Backup[];
  backups: Backup[];
  busy: string | null;
  r2Ready: boolean;
  backupPage: number;
  dumpPostgresTarget: (containerId?: string) => Promise<void>;
  restorePostgresTarget: (target: PostgresTarget, file: File) => Promise<void>;
  uploadBackupR2: (backup: Backup) => Promise<void>;
  setConfirm: (state: ConfirmState) => void;
  refreshBackups: () => Promise<void>;
  setBackupPage: (page: number) => void;
};

export const BackupsView = memo(function BackupsView(props: Props) {
  const {
    postgresTargets,
    visibleBackups,
    backups,
    busy,
    r2Ready,
    backupPage,
    dumpPostgresTarget,
    restorePostgresTarget,
    uploadBackupR2,
    setConfirm,
    refreshBackups,
    setBackupPage,
  } = props;

  return (
    <div className="backup-layout">
      <section className="panel">
        <div className="panel-head">
          <h2>Postgres targets</h2>
          <div className="actions">
            <span className="count">{postgresTargets.length} detected</span>
            <Button disabled={busy === "backup:yanto"} onClick={() => void dumpPostgresTarget()} icon={<Archive size={16} />}>
              Dump Yanto DB
            </Button>
          </div>
        </div>
        <PostgresTargetTable targets={postgresTargets} busy={busy} onDump={dumpPostgresTarget} onRestore={restorePostgresTarget} />
      </section>
      <section className="panel">
        <div className="panel-head">
          <h2>Backup history</h2>
          <span className="count">{backups.length} dumps</span>
        </div>
        <BackupTable
          backups={visibleBackups}
          busy={busy}
          r2Ready={r2Ready}
          onUploadR2={uploadBackupR2}
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
