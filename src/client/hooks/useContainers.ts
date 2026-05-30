import { useCallback, useMemo, useRef, useState } from "react";
import type { ContainerInfo } from "../../shared/types";
import { api } from "../lib/api";
import { useLogModal } from "../contexts/LogModalContext";

export function useContainers() {
  const [containers, setContainers] = useState<ContainerInfo[]>([]);
  const { openStreamingLog } = useLogModal();
  const statsRefreshRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const unhealthyContainers = useMemo(() => containers.filter((c) => !["running", "created"].includes(c.state)), [containers]);

  const fetchContainers = useCallback(async () => {
    try {
      return await api.containers();
    } catch {
      return null;
    }
  }, []);

  const fetchContainersSummary = useCallback(async () => {
    try {
      return await api.containersSummary();
    } catch {
      return null;
    }
  }, []);

  const refreshContainers = useCallback(async () => {
    const rows = await fetchContainers();
    if (rows) setContainers(rows);
  }, [fetchContainers]);

  const refreshContainersFast = useCallback(async () => {
    // Phase 1: Load summary (fast, no docker stats)
    const summary = await fetchContainersSummary();
    if (summary) setContainers(summary);

    // Phase 2: Load full stats in background
    if (statsRefreshRef.current) clearTimeout(statsRefreshRef.current);
    statsRefreshRef.current = setTimeout(async () => {
      const full = await fetchContainers();
      if (full) setContainers(full);
    }, 50);
  }, [fetchContainersSummary, fetchContainers]);

  function openContainerLogs(container: ContainerInfo) {
    openStreamingLog(`${container.name} logs`, api.containerLogStream(container.id), container.state);
  }

  return {
    containers, setContainers, unhealthyContainers,
    fetchContainers, fetchContainersSummary, refreshContainers, refreshContainersFast, openContainerLogs
  };
}
