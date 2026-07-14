import { memo } from "react";
import type { Deployment } from "../../shared/types";
import { Pagination } from "../components/Pagination";
import { DeploymentTable } from "../data-tables";

type Props = {
  deployments: Deployment[];
  visibleDeployments: Deployment[];
  deploymentPage: number;
  busy: string | null;
  loading?: boolean;
  openDeploymentLogs: (deployment: Deployment) => void;
  retryDeployment: (deployment: Deployment) => void;
  canRetry: (deployment: Deployment) => boolean;
  setDeploymentPage: (page: number) => void;
};

export const DeploymentsView = memo(function DeploymentsView(props: Props) {
  const { deployments, visibleDeployments, deploymentPage, busy, loading, openDeploymentLogs, retryDeployment, canRetry, setDeploymentPage } = props;

  return (
    <section className="panel">
      <div className="panel-head">
        <h2>Deployment history</h2>
        <span className="count">{deployments.length} records</span>
      </div>
      <DeploymentTable deployments={visibleDeployments} busy={busy} loading={loading} onLogs={openDeploymentLogs} onRetry={retryDeployment} canRetry={canRetry} />
      <Pagination label="Deployments" page={deploymentPage} totalItems={deployments.length} onPageChange={setDeploymentPage} />
    </section>
  );
});
