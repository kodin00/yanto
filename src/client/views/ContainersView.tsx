import { memo } from "react";
import type { ContainerInfo } from "../../shared/types";
import { ContainerGroups } from "../data-tables";
import type { ConfirmState } from "./types";

type Props = {
  containers: ContainerInfo[];
  loading?: boolean;
  openContainerLogs: (container: ContainerInfo) => void;
  setConfirm: (state: ConfirmState) => void;
  refreshContainers: () => Promise<void>;
};

export const ContainersView = memo(function ContainersView(props: Props) {
  const { containers, loading, openContainerLogs, setConfirm, refreshContainers } = props;

  return (
    <section className="panel">
      <div className="panel-head">
        <h2>Docker containers</h2>
        <span className="count">{containers.length} found</span>
      </div>
      <ContainerGroups containers={containers} loading={loading} onLogs={openContainerLogs} onConfirm={(next) => setConfirm(next)} onReload={refreshContainers} />
    </section>
  );
});
