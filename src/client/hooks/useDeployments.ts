import { useCallback, useEffect, useMemo, useState } from "react";
import type { Deployment } from "../../shared/types";
import { pageItems, totalPages } from "../app-utils";
import { api } from "../lib/api";
import { useLogModal } from "../contexts/LogModalContext";

export function useDeployments() {
  const [deployments, setDeployments] = useState<Deployment[]>([]);
  const [deploymentPage, setDeploymentPage] = useState(1);
  const { openStreamingLog } = useLogModal();

  const visibleDeployments = useMemo(() => pageItems(deployments, deploymentPage), [deploymentPage, deployments]);
  const runningDeployments = useMemo(() => deployments.filter((d) => d.status === "running"), [deployments]);

  useEffect(() => {
    setDeploymentPage((page) => Math.min(page, totalPages(deployments)));
  }, [deployments]);

  const refreshDeployments = useCallback(async () => {
    setDeployments(await api.deployments());
  }, []);

  function openDeploymentLogs(deployment: Deployment) {
    openStreamingLog(
      `${deployment.projectName ?? deployment.projectId} deployment`,
      api.deploymentLogStream(deployment.id),
      deployment.status
    );
  }

  return {
    deployments, setDeployments, deploymentPage, setDeploymentPage,
    visibleDeployments, runningDeployments,
    refreshDeployments, openDeploymentLogs
  };
}
