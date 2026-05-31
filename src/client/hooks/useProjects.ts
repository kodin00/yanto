import { useCallback, useEffect, useMemo, useState } from "react";
import type { ContainerInfo, CloudflareRoute, Deployment, DeploymentNode, Project, ProjectWithDeployToken } from "../../shared/types";
import { cloudflareServiceUrl, endpoint, githubWebhookEndpoint, normalizeEnvRows, pageItems, totalPages } from "../app-utils";
import { api } from "../lib/api";
import { useToast } from "../contexts/ToastContext";
import { useLogModal } from "../contexts/LogModalContext";
import {
  buildCfRouteForm, cfRouteServiceTarget, emptyProject, emptyProjectComposeState, parseCfServiceTarget,
  type CfRouteForm, type CreatedProjectSecret, type ProjectComposeState, type ProjectFormState, type RollbackModalState
} from "../types";
import type { ProjectEnvState } from "../components/EnvEditor";

const emptyProjectEnvState: ProjectEnvState = {
  rows: [], baseline: [], draftKey: "", draftValue: "", content: "", mode: "pairs", loading: false, available: true, opened: false
};

export function useProjects(deps: {
  nodes: DeploymentNode[];
  containers: ContainerInfo[];
  settings: { appBaseUrl: string; cf?: { hasApiToken?: boolean; accountId?: string; zoneId?: string } };
}) {
  const { nodes, containers, settings } = deps;
  const [projects, setProjects] = useState<Project[]>([]);
  const [deployments, setDeployments] = useState<Deployment[]>([]);
  const [busy, setBusy] = useState<string | null>(null);
  const [projectPage, setProjectPage] = useState(1);
  const [projectModal, setProjectModal] = useState<Project | "new" | null>(null);
  const [projectForm, setProjectForm] = useState<ProjectFormState>(emptyProject);
  const [projectEnv, setProjectEnv] = useState<ProjectEnvState>(emptyProjectEnvState);
  const [projectCompose, setProjectCompose] = useState<ProjectComposeState>(emptyProjectComposeState);
  const [projectEditorModal, setProjectEditorModal] = useState<"compose" | "env" | null>(null);
  const [cfRoutesByProject, setCfRoutesByProject] = useState<Record<string, CloudflareRoute[]>>({});
  const [cfRoutes, setCfRoutes] = useState<CloudflareRoute[]>([]);
  const [cfRouteForm, setCfRouteForm] = useState<CfRouteForm>(buildCfRouteForm());
  const [rollbackModal, setRollbackModal] = useState<RollbackModalState | null>(null);
  const [createdProjectSecret, setCreatedProjectSecret] = useState<CreatedProjectSecret | null>(null);
  const { showToast } = useToast();
  const { openStreamingLog } = useLogModal();

  const containersByProjectFolder = useMemo(() => {
    const map = new Map<string, ContainerInfo[]>();
    for (const container of containers) {
      if (!container.composeProject) continue;
      map.set(container.composeProject, [...(map.get(container.composeProject) ?? []), container]);
    }
    return map;
  }, [containers]);

  const latestDeploymentByProject = useMemo(() => {
    const map = new Map<string, Deployment>();
    for (const deployment of deployments) {
      if (!map.has(deployment.projectId)) map.set(deployment.projectId, deployment);
    }
    return map;
  }, [deployments]);

  const visibleProjects = useMemo(() => pageItems(projects, projectPage), [projectPage, projects]);

  const nodeOptions = useMemo(() =>
    (nodes.length ? nodes : [{ id: "node_master_local", name: "Master", role: "master", status: "online" } as DeploymentNode])
      .map((node) => ({ label: `${node.name} (${node.role})`, value: node.id })),
    [nodes]
  );

  useEffect(() => {
    const routeEntries = projects.map((project) => [project.id, project.cloudflareRoutes ?? []] as const);
    setCfRoutesByProject((current) => ({ ...current, ...Object.fromEntries(routeEntries) }));
  }, [projects]);

  useEffect(() => {
    setProjectPage((page) => Math.min(page, totalPages(projects)));
  }, [projects]);

  const refreshProjects = useCallback(async () => {
    const [projectRows, deploymentRows] = await Promise.all([api.projects(), api.deployments()]);
    setProjects(projectRows);
    setDeployments(deploymentRows);
  }, []);

  const refreshDeployments = useCallback(async () => {
    setDeployments(await api.deployments());
  }, []);

  function projectEnvPayload() {
    const rows = normalizeEnvRows(projectEnv.rows.filter((row) => row.key.trim()));
    return rows.map((row) => {
      const original = projectEnv.baseline.find((item) => item.key === row.key);
      if (original?.masked && original.value === row.value) return { key: row.key, masked: row.masked };
      return row;
    });
  }

  async function persistProjectDetails(event?: React.FormEvent, after?: "deploy" | "restart") {
    event?.preventDefault();
    if (!projectModal) return;
    setBusy("project");
    showToast(after === "deploy" ? "Saving project and starting deployment..." : after === "restart" ? "Saving project and restarting..." : "Saving project...", "loading");
    try {
      const creatingProject = projectModal === "new";
      let savedProject: Project | ProjectWithDeployToken;
      if (projectModal === "new") {
        savedProject = await api.createProject(projectForm);
      } else {
        savedProject = await api.updateProject(projectModal.id, projectForm);
      }

      const envRows = projectEnvPayload();
      if (projectEnv.opened && projectEnv.available && !projectEnv.loading && (projectModal !== "new" || envRows.length || projectEnv.content.trim())) {
        if (projectEnv.mode === "text") {
          await api.updateProjectEnvContent(savedProject.id, projectEnv.content);
        } else {
          await api.updateProjectEnv(savedProject.id, envRows);
        }
      }
      if (after === "restart") await api.restartProject(savedProject.id);
      if (after === "deploy") await api.deployProject(savedProject.id);
      setProjectModal(null);
      setProjectEditorModal(null);
      await refreshProjects();
      if (creatingProject && "deployToken" in savedProject) {
        setCreatedProjectSecret({
          projectName: savedProject.name,
          deployUrl: endpoint(savedProject, settings.appBaseUrl),
          webhookUrl: githubWebhookEndpoint(savedProject, settings.appBaseUrl),
          deployToken: savedProject.deployToken
        });
      }
      showToast(
        after === "restart" ? "Project saved and restart started."
          : after === "deploy" ? "Project saved and deployment started."
            : creatingProject ? "Project registered. Save the one-time deploy token."
              : "Project updated."
      );
    } catch (error) {
      showToast(error instanceof Error ? error.message : "Unable to save project.", "error");
    } finally {
      setBusy(null);
    }
  }

  async function deploy(project: Project) {
    setBusy(`deploy:${project.id}`);
    showToast(`Starting deployment for ${project.name}...`, "loading");
    try {
      const result = await api.deployProject(project.id);
      showToast(result.reused ? "Deployment is already running." : "Deployment started.");
      openStreamingLog(`${project.name} deployment`, api.deploymentLogStream(result.deployment.id), result.deployment.status);
      await refreshDeployments();
    } catch (error) {
      showToast(error instanceof Error ? error.message : "Unable to deploy project.", "error");
    } finally {
      setBusy(null);
    }
  }

  function openProject(project?: Project) {
    setProjectEditorModal(null);
    if (project) {
      setProjectForm({
        name: project.name, gitUrl: project.gitUrl ?? "", branch: project.branch, folderName: project.folderName,
        composeFile: project.composeFile, composeContent: project.composeContent ?? "", autoStart: project.autoStart,
        manualDeployEnabled: project.manualDeployEnabled, githubWebhookEnabled: project.githubWebhookEnabled,
        targetNodeId: project.targetNodeId || "node_master_local"
      });
      setProjectEnv(emptyProjectEnvState);
      setProjectCompose(emptyProjectComposeState);
      const projectRoutes = project.cloudflareRoutes ?? [];
      const detectedServiceTarget = cloudflareServiceUrl(project, containersByProjectFolder.get(project.folderName) ?? []);
      setCfRouteForm(buildCfRouteForm("", projectRoutes[0]?.serviceTarget || detectedServiceTarget, projectRoutes[0]?.noTlsVerify ?? false));
      setCfRoutes(projectRoutes);
      setProjectModal(project);
      void api.projectCfRoutes(project.id).catch(() => [])
        .then((routes) => {
          setCfRoutes(routes);
          setCfRoutesByProject((current) => ({ ...current, [project.id]: routes }));
          if (routes.length) setCfRouteForm(buildCfRouteForm("", routes[0].serviceTarget, routes[0].noTlsVerify));
        })
        .catch((error) => showToast(error instanceof Error ? error.message : "Unable to load Cloudflare routes.", "error"));
      return;
    }
    setProjectForm({ ...emptyProject, targetNodeId: nodes[0]?.id ?? "node_master_local" });
    setProjectEnv(emptyProjectEnvState);
    setProjectCompose(emptyProjectComposeState);
    setCfRoutes([]);
    setCfRouteForm(buildCfRouteForm());
    setProjectModal("new");
  }

  async function openComposeEditor() {
    if (!projectModal || projectCompose.loading) return;
    if (projectCompose.open) { setProjectEditorModal("compose"); return; }
    if (projectModal === "new") {
      setProjectCompose({ open: true, loading: false, available: true, source: "empty", message: "Custom compose" });
      setProjectEditorModal("compose");
      return;
    }
    if (projectForm.composeContent.trim()) {
      setProjectCompose({ open: true, loading: false, available: true, source: "saved", message: "Saved override" });
      setProjectEditorModal("compose");
      return;
    }
    setProjectCompose({ ...emptyProjectComposeState, loading: true });
    showToast("Loading compose file...", "loading");
    try {
      const compose = await api.projectComposeContent(projectModal.id);
      setProjectForm((current) => ({ ...current, composeContent: compose.content }));
      setProjectCompose({ open: true, loading: false, available: true, source: compose.exists ? "file" : "empty", message: compose.exists ? `Loaded ${compose.composeFile}` : `${compose.composeFile} not found` });
      setProjectEditorModal("compose");
      showToast("", "ok");
    } catch (error) {
      setProjectCompose({ open: false, loading: false, available: false, source: null, message: "Compose could not be loaded" });
      showToast(error instanceof Error ? error.message : "Unable to load compose file.", "error");
    }
  }

  async function openEnvEditor() {
    if (!projectModal || projectEnv.loading) return;
    if (projectEnv.opened) { setProjectEditorModal("env"); return; }
    if (projectModal === "new") {
      setProjectEnv({ ...emptyProjectEnvState, opened: true });
      setProjectEditorModal("env");
      return;
    }
    setProjectEnv({ ...emptyProjectEnvState, opened: true, loading: true });
    showToast("Loading environment variables...", "loading");
    try {
      const [rows, envContent] = await Promise.all([api.projectEnv(projectModal.id), api.projectEnvContent(projectModal.id)]);
      const normalizedRows = normalizeEnvRows(rows);
      setProjectEnv({ rows: normalizedRows, baseline: normalizedRows, draftKey: "", draftValue: "", content: envContent.content, mode: "pairs", loading: false, available: true, opened: true });
      setProjectEditorModal("env");
      showToast("", "ok");
    } catch (error) {
      setProjectEnv({ ...emptyProjectEnvState, loading: false, available: false, opened: true });
      showToast(error instanceof Error ? error.message : "Unable to load environment.", "error");
    }
  }

  function openRollback(project: Project) {
    const projectDeployments = deployments.filter((d) => d.projectId === project.id && d.status === "success");
    setRollbackModal({ project, deployments: projectDeployments });
  }

  async function executeRollback(project: Project, deploymentId: string) {
    setBusy(`rollback:${project.id}`);
    showToast(`Starting rollback for ${project.name}...`, "loading");
    try {
      const result = await api.rollbackProject(project.id, deploymentId);
      setRollbackModal(null);
      showToast("Rollback started.");
      openStreamingLog(`${project.name} rollback`, api.deploymentLogStream(result.deployment.id), result.deployment.status);
      await refreshDeployments();
    } catch (error) {
      showToast(error instanceof Error ? error.message : "Unable to start rollback.", "error");
    } finally {
      setBusy(null);
    }
  }

  async function publishCfRoute(projectId: string) {
    setBusy("cf-route-publish");
    showToast("Publishing Cloudflare route...", "loading");
    try {
      const serviceTarget = cfRouteServiceTarget(cfRouteForm);
      const route = await api.publishCfRoute(projectId, {
        hostname: cfRouteForm.hostname, serviceTarget,
        noTlsVerify: serviceTarget.startsWith("https://") ? cfRouteForm.noTlsVerify : false
      });
      setCfRoutes((current) => [...current, route]);
      setCfRoutesByProject((current) => ({ ...current, [projectId]: [...(current[projectId] ?? []), route] }));
      setProjects((current) => current.map((p) => (p.id === projectId ? { ...p, cloudflareRoutes: [...(p.cloudflareRoutes ?? []), route] } : p)));
      const project = projects.find((p) => p.id === projectId);
      setCfRouteForm(buildCfRouteForm("", project ? cloudflareServiceUrl(project, containersByProjectFolder.get(project.folderName) ?? []) : ""));
      showToast(`Route published: https://${route.hostname}`);
    } catch (error) {
      showToast(error instanceof Error ? error.message : "Unable to publish route.", "error");
    } finally {
      setBusy(null);
    }
  }

  async function toggleCfRoute(route: CloudflareRoute) {
    setBusy(`cf-route-toggle:${route.id}`);
    showToast(route.enabled ? "Disabling Cloudflare route..." : "Enabling Cloudflare route...", "loading");
    try {
      const updated = route.enabled ? await api.disableCfRoute(route.id) : await api.enableCfRoute(route.id);
      setCfRoutes((current) => current.map((r) => (r.id === updated.id ? updated : r)));
      setCfRoutesByProject((current) => ({ ...current, [updated.projectId]: (current[updated.projectId] ?? []).map((r) => (r.id === updated.id ? updated : r)) }));
      setProjects((current) => current.map((p) => (p.id === updated.projectId ? { ...p, cloudflareRoutes: (p.cloudflareRoutes ?? []).map((r) => (r.id === updated.id ? updated : r)) } : p)));
      showToast(route.enabled ? "Route disabled." : "Route enabled.");
    } catch (error) {
      showToast(error instanceof Error ? error.message : "Unable to update route.", "error");
    } finally {
      setBusy(null);
    }
  }

  async function removeCfRoute(routeId: string) {
    setBusy(`cf-route-delete:${routeId}`);
    showToast("Deleting Cloudflare route...", "loading");
    try {
      await api.deleteCfRoute(routeId);
      const deletedRoute = cfRoutes.find((r) => r.id === routeId);
      setCfRoutes((current) => current.filter((r) => r.id !== routeId));
      if (deletedRoute) {
        setCfRoutesByProject((current) => ({ ...current, [deletedRoute.projectId]: (current[deletedRoute.projectId] ?? []).filter((r) => r.id !== routeId) }));
        setProjects((current) => current.map((p) => (p.id === deletedRoute.projectId ? { ...p, cloudflareRoutes: (p.cloudflareRoutes ?? []).filter((r) => r.id !== routeId) } : p)));
      }
      showToast("Route deleted.");
    } catch (error) {
      showToast(error instanceof Error ? error.message : "Unable to delete route.", "error");
    } finally {
      setBusy(null);
    }
  }

  return {
    projects, setProjects, deployments, setDeployments, busy, projectPage, setProjectPage,
    projectModal, setProjectModal, projectForm, setProjectForm, projectEnv, setProjectEnv,
    projectCompose, projectEditorModal, setProjectEditorModal,
    cfRoutesByProject, cfRoutes, cfRouteForm, setCfRouteForm,
    rollbackModal, setRollbackModal, createdProjectSecret, setCreatedProjectSecret,
    containersByProjectFolder, latestDeploymentByProject, visibleProjects, nodeOptions,
    refreshProjects, refreshDeployments, persistProjectDetails, deploy, openProject,
    openComposeEditor, openEnvEditor, openRollback, executeRollback,
    publishCfRoute, toggleCfRoute, removeCfRoute, parseCfServiceTarget
  };
}
