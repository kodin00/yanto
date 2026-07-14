import { Settings } from "lucide-react";
import { memo } from "react";
import type { ContainerInfo, Deployment, DeploymentNode, Project, SystemUsage } from "../../shared/types";
import { Button, StatusBadge } from "../components/ui";
import { DeploymentTable } from "../data-tables";
import type { SettingsState } from "./types";

type Props = {
  isOwner: boolean;
  projects: Project[];
  nodes: DeploymentNode[];
  containers: ContainerInfo[];
  deployments: Deployment[];
  runningDeployments: Deployment[];
  usage: SystemUsage | null;
  settings: SettingsState;
  setupCanReopen: boolean;
  failingProjects: Project[];
  unhealthyContainers: ContainerInfo[];
  warningDisks: SystemUsage["storage"];
  openSetupWizard: () => void;
  openDeploymentLogs: (deployment: Deployment) => void;
};

export const DashboardView = memo(function DashboardView(props: Props) {
  const {
    isOwner,
    projects,
    nodes,
    containers,
    deployments,
    runningDeployments,
    usage,
    settings,
    setupCanReopen,
    failingProjects,
    unhealthyContainers,
    warningDisks,
    openSetupWizard,
    openDeploymentLogs,
  } = props;

  return (
    <section className="dashboard">
      <section className="stat-grid">
        <StatTile label="Projects" value={projects.length} detail={isOwner ? `${settings.hostProjectsRoot} root` : "Assigned to you"} />
        {isOwner ? <StatTile label="Nodes" value={nodes.length || 1} detail={`${nodes.filter((node) => node.status === "online").length || 1} online`} /> : null}
        {runningDeployments.length ? <StatTile label="Active deploys" value={runningDeployments.length} detail="Deployment in progress" /> : null}
        <StatTile label="Running containers" value={containers.filter((container) => container.state === "running").length} detail={`${containers.length} total containers`} />
        {isOwner ? <StatTile label="RAM used" value={usage ? `${usage.memory.usedPercent}%` : "-"} detail={usage ? `${bytes(usage.memory.used)} of ${bytes(usage.memory.total)}` : "Unavailable"} /> : null}
      </section>

      {setupCanReopen ? (
        <section className="setup-banner">
          <div>
            <strong>Quick setup</strong>
            <p>SSH, Cloudflare Tunnel, and R2 can be added now or later.</p>
          </div>
          <Button variant="secondary" onClick={() => openSetupWizard()} icon={<Settings size={16} />}>
            Open setup
          </Button>
        </section>
      ) : null}

      <div className="dashboard-main-grid">
        <div className="dashboard-left-column">
          <WarningsPanel failingProjects={failingProjects} unhealthyContainers={unhealthyContainers} warningDisks={warningDisks} />
          {isOwner ? <UsagePanel usage={usage} /> : null}
          <section className="panel">
            <div className="section-kicker">Build history</div>
            <div className="panel-head">
              <h2>Recent deployments</h2>
              <StatusBadge status={runningDeployments.length ? "running" : "idle"} />
            </div>
            <DeploymentTable deployments={deployments.slice(0, 6)} onLogs={openDeploymentLogs} compact />
          </section>
        </div>
        <section className="panel container-overview">
          <div className="section-kicker">Runtime inventory</div>
          <div className="panel-head">
            <h2>Containers</h2>
            <span className="count">{containers.filter((container) => container.state === "running").length} running</span>
          </div>
          <div className="compact-list">
            {containers.slice(0, 7).map((container) => (
              <div className="row-item technical-row" key={container.id}>
                <div>
                  <strong>{container.name}</strong>
                  <span>{container.image}</span>
                </div>
                <div className="row-metrics">
                  <span>{container.cpuPercent}</span>
                  <span>{container.memoryPercent}</span>
                  <StatusBadge status={container.state} />
                </div>
              </div>
            ))}
            {!containers.length ? <p className="muted">No containers found yet.</p> : null}
          </div>
        </section>
      </div>
    </section>
  );
});

// Re-used helper components from App.tsx
import { Activity, AlertTriangle, HardDrive, MemoryStick, Server } from "lucide-react";
import type { ReactNode } from "react";
import { bytes } from "../app-utils";

function UsagePanel({ usage }: { usage: SystemUsage | null }) {
  const storage = usage?.storage.find((disk) => disk.mount === "/projects") ?? usage?.storage[0];

  return (
    <section className="panel">
      <div className="section-kicker">Host telemetry</div>
      <div className="panel-head">
        <h2>VPS usage</h2>
        <Server size={19} />
      </div>
      {usage ? (
        <div className="meter-grid">
          <SegmentedMeter label="CPU load" value={usage.cpuLoadPercent} icon={<Activity size={15} />} />
          <SegmentedMeter label="RAM" value={usage.memory.usedPercent} detail={`${bytes(usage.memory.used)} / ${bytes(usage.memory.total)}`} icon={<MemoryStick size={15} />} />
          {storage ? <SegmentedMeter label={`Storage ${storage.mount}`} value={storage.usedPercent} detail={`${bytes(storage.used)} / ${bytes(storage.size)}`} icon={<HardDrive size={15} />} /> : null}
        </div>
      ) : (
        <p className="muted">System usage is unavailable. Check the container permissions and mounted project path.</p>
      )}
    </section>
  );
}

function WarningsPanel({
  failingProjects,
  unhealthyContainers,
  warningDisks,
}: {
  failingProjects: Project[];
  unhealthyContainers: ContainerInfo[];
  warningDisks: SystemUsage["storage"];
}) {
  const warningCount = failingProjects.length + unhealthyContainers.length + warningDisks.length;
  if (!warningCount) return null;

  return (
    <section className="panel warning-panel">
      <div className="panel-head">
        <h2>Warnings</h2>
        <AlertTriangle size={19} />
      </div>
      <div className="warning-list">
        {failingProjects.map((project) => (
          <div key={`project:${project.id}`}>
            <StatusBadge status="failed" />
            <span>{project.name}</span>
            <small>Latest deployment failed</small>
          </div>
        ))}
        {unhealthyContainers.slice(0, 5).map((container) => (
          <div key={`container:${container.id}`}>
            <StatusBadge status={container.state} />
            <span>{container.name}</span>
            <small>Container is not running</small>
          </div>
        ))}
        {warningDisks.map((disk) => (
          <div key={`disk:${disk.filesystem}:${disk.mount}`}>
            <StatusBadge status="warning" />
            <span>{disk.mount}</span>
            <small>{disk.usedPercent}% disk used</small>
          </div>
        ))}
      </div>
    </section>
  );
}

function StatTile({ label, value, detail }: { label: string; value: string | number; detail: string }) {
  return (
    <article className="stat-tile">
      <span>{label}</span>
      <strong>{value}</strong>
      <small>{detail}</small>
    </article>
  );
}

function SegmentedMeter({ label, value, detail, icon }: { label: string; value: number; detail?: string; icon?: ReactNode }) {
  const activeBlocks = Math.round((Math.min(100, Math.max(0, value)) / 100) * 50);
  return (
    <div className="meter">
      <div>
        <span>{icon}{label}</span>
        <strong>{value}%</strong>
      </div>
      <div className="segmented-meter" aria-label={`${label} ${value}%`}>
        {Array.from({ length: 50 }).map((_, index) => (
          <span key={index} className={index < activeBlocks ? "on" : ""} />
        ))}
      </div>
      {detail ? <small>{detail}</small> : null}
    </div>
  );
}
