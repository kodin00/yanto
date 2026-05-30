import { ChevronLeft, ChevronRight } from "lucide-react";
import { memo } from "react";
import type { Deployment } from "../../shared/types";
import { pageSize, totalPages } from "../app-utils";
import { Button } from "../components/ui";
import { DeploymentTable } from "../data-tables";

type Props = {
  deployments: Deployment[];
  visibleDeployments: Deployment[];
  deploymentPage: number;
  openDeploymentLogs: (deployment: Deployment) => void;
  setDeploymentPage: (page: number) => void;
};

export const DeploymentsView = memo(function DeploymentsView(props: Props) {
  const { deployments, visibleDeployments, deploymentPage, openDeploymentLogs, setDeploymentPage } = props;

  return (
    <section className="panel">
      <div className="panel-head">
        <h2>Deployment history</h2>
        <span className="count">{deployments.length} records</span>
      </div>
      <DeploymentTable deployments={visibleDeployments} onLogs={openDeploymentLogs} />
      <Pagination label="Deployments" page={deploymentPage} totalItems={deployments.length} onPageChange={setDeploymentPage} />
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
