import { AlertTriangle, Copy, KeyRound, Play, Plus, RotateCw, Square, Undo2 } from "lucide-react";
import { memo } from "react";
import type { CloudflareRoute, CloudflareRouteDiagnostic, ContainerInfo, Deployment, DeploymentNode, Project, SessionUser } from "../../shared/types";
import { endpoint, githubWebhookEndpoint } from "../app-utils";
import { Pagination } from "../components/Pagination";
import { Button, StatusBadge } from "../components/ui";
import { api } from "../lib/api";
import { canManageProject } from "../permissions";
import type { ConfirmState, SettingsState } from "./types";

function deploymentBadge(deployment?: Deployment) {
  if (!deployment) return { status: "ready", label: "Deployment ready" };
  return {
    status: deployment.status,
    label: `Deployment ${deployment.status}`
  };
}

function containerBadge(containers: ContainerInfo[], fallbackCount = 0) {
  if (!containers.length) {
    return fallbackCount ? { status: "warning", label: "Containers unknown" } : { status: "idle", label: "No containers" };
  }
  const total = containers.length;
  const running = containers.filter((container) => container.state === "running").length;
  if (running === total) return { status: "running", label: total === 1 ? "Container running" : "Containers running" };
  if (running === 0) return { status: "exited", label: total === 1 ? "Container stopped" : "Containers stopped" };
  return { status: "warning", label: "Containers mixed" };
}

type Props = {
  session: SessionUser;
  visibleProjects: Project[];
  projects: Project[];
  nodes: DeploymentNode[];
  containersByProjectFolder: Map<string, ContainerInfo[]>;
  cfRoutesByProject: Record<string, CloudflareRoute[]>;
  routeDiagnosticsByRouteId: Record<string, CloudflareRouteDiagnostic>;
  latestDeploymentByProject: Map<string, Deployment>;
  settings: SettingsState;
  busy: string | null;
  loading?: boolean;
  projectPage: number;
  openProject: (project?: Project) => void;
  openRollback: (project: Project) => void;
  deploy: (project: Project) => void;
  copyText: (value: string) => Promise<void>;
  copyDeployToken: (project: Project) => Promise<void>;
  setConfirm: (state: ConfirmState) => void;
  refreshProjects: () => Promise<void>;
  setProjectPage: (page: number) => void;
};

export const ProjectsView = memo(function ProjectsView(props: Props) {
  const {
    visibleProjects,
    session,
    projects,
    nodes,
    containersByProjectFolder,
    cfRoutesByProject,
    routeDiagnosticsByRouteId,
    latestDeploymentByProject,
    settings,
    busy,
    loading,
    projectPage,
    openProject,
    openRollback,
    deploy,
    copyText,
    copyDeployToken,
    setConfirm,
    refreshProjects,
    setProjectPage,
  } = props;

  return (
    <section className="panel">
      <div className="panel-head">
        <h2>Registered projects</h2>
        {session.role === "owner" ? <Button onClick={() => openProject()} icon={<Plus size={16} />}>Add project</Button> : null}
      </div>
      {loading && !visibleProjects.length ? <p className="muted">Loading projects...</p> : null}
      <div className="project-grid">
        {visibleProjects.map((project) => {
          const projectContainers = containersByProjectFolder.get(project.folderName) ?? [];
          const projectRoutes = cfRoutesByProject[project.id] ?? project.cloudflareRoutes ?? [];
          const activeRoutes = projectRoutes.filter((route) => route.enabled);
          const primaryRoute = activeRoutes[0] ?? projectRoutes[0];
          const primaryDiagnostic = primaryRoute ? routeDiagnosticsByRouteId[primaryRoute.id] : undefined;
          const runningCount = projectContainers.filter((container) => container.state === "running").length;
          const deploymentStatus = deploymentBadge(latestDeploymentByProject.get(project.id));
          const containerStatus = containerBadge(projectContainers, project.containerCount);
          const canConfigure = canManageProject(session, project.id, "config");
          const canDeploy = canManageProject(session, project.id, "deploy");
          const canUseSecrets = canManageProject(session, project.id, "secrets");
          const canUseRuntime = canManageProject(session, project.id, "runtime");
          const localRuntimeControl = session.role === "owner"
            ? nodes.find((node) => node.id === project.targetNodeId)?.role === "master"
            : project.targetNodeId === session.localNodeId;
          return (
            <article
              className={`project-card ${canConfigure ? "" : "read-only"}`}
              key={project.id}
              role={canConfigure ? "button" : undefined}
              tabIndex={canConfigure ? 0 : undefined}
              onClick={() => { if (canConfigure) openProject(project); }}
              onKeyDown={(event) => {
                if (canConfigure && (event.key === "Enter" || event.key === " ")) {
                  event.preventDefault();
                  openProject(project);
                }
              }}
            >
              <div className="project-card-main">
                <div className="project-card-title">
                  <h3>{project.name}</h3>
                  <p>{project.gitUrl || "Compose file project"}</p>
                </div>
                <div className="project-card-statuses">
                  <StatusBadge status={deploymentStatus.status} label={deploymentStatus.label} />
                  <StatusBadge status={containerStatus.status} label={containerStatus.label} />
                </div>
                <div className="project-card-meta">
                  <span>{runningCount}/{projectContainers.length || project.containerCount || 0} running</span>
                  <span>{project.folderName}</span>
                </div>
                {session.role === "owner" && project.containerMappingWarning ? (
                  <p className="project-mapping-warning"><AlertTriangle size={13} />{project.containerMappingWarning}</p>
                ) : null}
                <div className="project-route-summary">
                  {primaryRoute ? (
                    <>
                      {primaryDiagnostic ? (
                        <StatusBadge status={primaryDiagnostic.tunnelStatus} label={`Tunnel ${primaryDiagnostic.tunnelStatus}`} />
                      ) : (
                        <StatusBadge status={primaryRoute.enabled ? "checking" : "disabled"} label={primaryRoute.enabled ? "Checking hostname" : "Route disabled"} />
                      )}
                      <a href={`https://${primaryRoute.hostname}`} target="_blank" rel="noopener noreferrer" onClick={(event) => event.stopPropagation()}>
                        https://{primaryRoute.hostname}
                      </a>
                    </>
                  ) : settings.cf?.hasApiToken ? (
                    <span className="muted">No hostname configured</span>
                  ) : (
                    <span className="muted">Cloudflare not configured</span>
                  )}
                </div>
              </div>
              <div className="project-card-side" onClick={(event) => event.stopPropagation()}>
                <div className="project-copy-actions">
                  <Button variant="ghost" onClick={() => void copyText(endpoint(project, settings.appBaseUrl))} icon={<Copy size={14} />}>
                    Deploy URL
                  </Button>
                  <Button variant="ghost" onClick={() => void copyText(githubWebhookEndpoint(project, settings.appBaseUrl))} icon={<Copy size={14} />}>
                    Webhook
                  </Button>
                  {canUseSecrets ? <Button variant="ghost" loading={busy === `token:${project.id}`} onClick={() => void copyDeployToken(project)} icon={<KeyRound size={14} />}>Secret</Button> : null}
                </div>
                <div className="actions" onClick={(event) => event.stopPropagation()}>
                  {canDeploy ? <Button variant="secondary" onClick={() => openRollback(project)} icon={<Undo2 size={15} />}>Rollback</Button> : null}
                  {canDeploy ? <Button disabled={!project.manualDeployEnabled} loading={busy === `deploy:${project.id}`} onClick={() => void deploy(project)} icon={<Play size={15} />}>{busy === `deploy:${project.id}` ? "Deploying" : "Deploy"}</Button> : null}
                  {canUseRuntime ? <Button
                    variant="secondary"
                    disabled={!localRuntimeControl}
                    title={localRuntimeControl ? undefined : "Project runtime controls are not available for worker nodes yet."}
                    onClick={() =>
                      setConfirm({
                        title: "Stop project",
                        body: `Stop containers for ${project.name}?`,
                        label: "Stop",
                        danger: true,
                        loadingMessage: `Stopping ${project.name}...`,
                        successMessage: "Project stopped.",
                        action: async () => {
                          await api.stopProject(project.id);
                          await refreshProjects();
                        },
                      })
                    }
                    icon={<Square size={15} />}
                  >
                    Stop
                  </Button> : null}
                  {canUseRuntime ? <Button
                    variant="secondary"
                    disabled={!localRuntimeControl}
                    title={localRuntimeControl ? undefined : "Project runtime controls are not available for worker nodes yet."}
                    onClick={() =>
                      setConfirm({
                        title: "Restart project",
                        body: `Restart containers for ${project.name}?`,
                        label: "Restart",
                        loadingMessage: `Restarting ${project.name}...`,
                        successMessage: "Project restarted.",
                        action: async () => {
                          await api.restartProject(project.id);
                          await refreshProjects();
                        },
                      })
                    }
                    icon={<RotateCw size={15} />}
                  >
                    Restart
                  </Button> : null}
                  {session.role === "owner" ? <Button
                    variant="danger"
                    onClick={() =>
                      setConfirm({
                        title: "Remove project",
                        body: "This removes the project record, deployment logs, and the project folder from disk.",
                        label: "Remove",
                        danger: true,
                        loadingMessage: `Removing ${project.name}...`,
                        successMessage: "Project removed.",
                        action: async () => {
                          await api.deleteProject(project.id);
                          await refreshProjects();
                        },
                      })
                    }
                  >
                    Remove
                  </Button> : null}
                </div>
              </div>
            </article>
          );
        })}
      </div>
      {!loading && !projects.length ? <p className="muted">No projects registered yet. Add a project to configure and deploy it.</p> : null}
      <Pagination label="Projects" page={projectPage} totalItems={projects.length} onPageChange={setProjectPage} />
    </section>
  );
});
