import { useCallback, useMemo, useState } from "react";
import type { ContainerInfo } from "../../shared/types";
import { api } from "../lib/api";
import { useLogModal } from "../contexts/LogModalContext";

export function useContainers() {
  const [containers, setContainers] = useState<ContainerInfo[]>([]);
  const { openStreamingLog } = useLogModal();

  const unhealthyContainers = useMemo(() => containers.filter((c) => !["running", "created"].includes(c.state)), [containers]);

  const fetchContainers = useCallback(async () => {
    try {
      return await api.containers();
    } catch {
      return null;
    }
  }, []);

  const refreshContainers = useCallback(async () => {
    const rows = await fetchContainers();
    if (rows) setContainers(rows);
  }, [fetchContainers]);

  function openContainerLogs(container: ContainerInfo) {
    openStreamingLog(`${container.name} logs`, api.containerLogStream(container.id), container.state);
  }

  return {
    containers, setContainers, unhealthyContainers,
    fetchContainers, refreshContainers, openContainerLogs
  };
}
