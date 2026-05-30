import { useCallback, useState } from "react";
import type { DeploymentNode } from "../../shared/types";
import { api } from "../lib/api";

export function useNodes() {
  const [nodes, setNodes] = useState<DeploymentNode[]>([]);

  const refreshNodes = useCallback(async () => {
    setNodes(await api.nodes().catch(() => []));
  }, []);

  return { nodes, setNodes, refreshNodes };
}
