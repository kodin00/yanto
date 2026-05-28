import {
  Activity,
  Archive,
  AlertTriangle,
  Boxes,
  ChevronLeft,
  ChevronRight,
  Container,
  Copy,
  DatabaseZap,
  FileClock,
  FileText,
  GitBranch,
  Cloud,
  HardDrive,
  KeyRound,
  List,
  LogOut,
  MemoryStick,
  Play,
  Plus,
  RefreshCw,
  RotateCw,
  Server,
  Settings,
  ShieldCheck,
  Square,
  Undo2,
  Trash2
} from "lucide-react";
import { FormEvent, type ReactNode, useCallback, useEffect, useMemo, useState } from "react";
import type { CloudflarePublicSettings, CloudflareRoute, ContainerInfo, Deployment, DeploymentNode, Project, SystemUsage } from "../shared/types";
import {
  bytes,
  dateTime,
  durationBetween,
  endpoint,
  githubWebhookEndpoint,
  normalizeEnvRows,
  pageItems,
  pageSize,
  slugifyFolderName,
  totalPages
} from "./app-utils";
import { Button, ConfirmDialog, CustomSelect, IconButton, LoadingInline, LogViewer, Modal, StatusBadge, TextAreaField, TextField, Toast, ToggleField } from "./components/ui";
import { AuditTable, BackupTable, ContainerGroups, DeploymentTable, PostgresTargetTable } from "./data-tables";
import { api, type AuditLogEntry, type BackupRecord, type CloudflareRoutePayload, type PostgresTarget, type ProjectEnvVariable } from "./lib/api";

type View = "dashboard" | "projects" | "deployments" | "containers" | "nodes" | "backups" | "audit" | "settings";
type ToastState = { message: string; kind?: "ok" | "error" | "loading" } | null;
type LogModalState = { title: string; logs: string; streamPath?: string; live?: boolean; status?: string };
type LogStreamPayload = { logs?: string; chunk?: string; status?: string; error?: string; done?: boolean };
type EnvEditMode = "pairs" | "text";
type ProjectEnvState = { rows: ProjectEnvVariable[]; baseline: ProjectEnvVariable[]; draftKey: string; draftValue: string; content: string; mode: EnvEditMode; loading: boolean; available: boolean };
type RollbackModalState = { project: Project; deployments: Deployment[] };

const emptyProject = {
  name: "",
  gitUrl: "",
  branch: "master",
  folderName: "",
  composeFile: "docker-compose.yml",
  composeContent: "",
  autoStart: true,
  manualDeployEnabled: true,
  githubWebhookEnabled: true,
  targetNodeId: "node_master_local"
};

const emptySshKeySettings = {
  hasManagedKey: false,
  hasMountedKey: false,
  managedPrivateKeyPath: "/data/ssh/id_ed25519",
  mountedPrivateKeyPath: "/root/.ssh/id_ed25519",
  activePrivateKeyPath: null as string | null,
  publicKey: null as string | null
};

const emptyR2Settings = {
  enabled: false,
  accountId: "",
  bucket: "",
  accessKeyId: "",
  hasSecretAccessKey: false,
  prefix: "postgres-dumps"
};

const emptyCfSettings: CloudflarePublicSettings = {
  accountId: "",
  zoneId: "",
  hasApiToken: false
};

const emptyProjectEnvState: ProjectEnvState = {
  rows: [],
  baseline: [],
  draftKey: "",
  draftValue: "",
  content: "",
  mode: "pairs",
  loading: false,
  available: true
};

function serializeEnvRows(rows: ProjectEnvVariable[]) {
  const content = normalizeEnvRows(rows)
    .map((row) => `${row.key}=${row.value ?? ""}`)
    .join("\n");
  return content ? `${content}\n` : "";
}

function parseEnvContentRows(content: string) {
  const rows: ProjectEnvVariable[] = [];
  for (const line of content.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const separator = line.indexOf("=");
    if (separator <= 0) continue;
    const value = line.slice(separator + 1);
    rows.push({ key: line.slice(0, separator).trim(), value, masked: value === "********" });
  }
  return normalizeEnvRows(rows);
}

export function App() {
  const [user, setUser] = useState<string | null>(null);
  const [login, setLogin] = useState({ username: "admin", password: "" });
  const [view, setView] = useState<View>("dashboard");
  const [projects, setProjects] = useState<Project[]>([]);
  const [deployments, setDeployments] = useState<Deployment[]>([]);
  const [containers, setContainers] = useState<ContainerInfo[]>([]);
  const [nodes, setNodes] = useState<DeploymentNode[]>([]);
  const [backups, setBackups] = useState<BackupRecord[]>([]);
  const [postgresTargets, setPostgresTargets] = useState<PostgresTarget[]>([]);
  const [auditEntries, setAuditEntries] = useState<AuditLogEntry[]>([]);
  const [usage, setUsage] = useState<SystemUsage | null>(null);
  const [settings, setSettings] = useState({ projectsRoot: "/projects", hostProjectsRoot: "~/projects", sshKeysDir: "", appBaseUrl: "", sshKey: emptySshKeySettings, r2: emptyR2Settings, cf: emptyCfSettings });
  const [r2Form, setR2Form] = useState({ enabled: false, accountId: "", bucket: "", accessKeyId: "", secretAccessKey: "", prefix: "postgres-dumps" });
  const [cfForm, setCfForm] = useState({ accountId: "", zoneId: "", apiToken: "" });
  const [cfRoutes, setCfRoutes] = useState<CloudflareRoute[]>([]);
  const [systemLogs, setSystemLogs] = useState("");
  const [cleanupLogs, setCleanupLogs] = useState("");
  const [cleanupLogTitle, setCleanupLogTitle] = useState("Cleanup preview");
  const [cleanupPreviewed, setCleanupPreviewed] = useState(false);
  const [loading, setLoading] = useState(true);
  const [toast, setToast] = useState<ToastState>(null);
  const [projectModal, setProjectModal] = useState<Project | "new" | null>(null);
  const [projectForm, setProjectForm] = useState(emptyProject);
  const [projectEnv, setProjectEnv] = useState<ProjectEnvState>(emptyProjectEnvState);
  const [rollbackModal, setRollbackModal] = useState<RollbackModalState | null>(null);
  const [logModal, setLogModal] = useState<LogModalState | null>(null);
  const [confirm, setConfirm] = useState<{ title: string; body: string; label: string; danger?: boolean; action: () => Promise<void> } | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [sshPrivateKey, setSshPrivateKey] = useState("");
  const [projectPage, setProjectPage] = useState(1);
  const [deploymentPage, setDeploymentPage] = useState(1);
  const [backupPage, setBackupPage] = useState(1);
  const [auditPage, setAuditPage] = useState(1);

  const loadAll = useCallback(async () => {
    const [projectRows, deploymentRows, containerRows, nodeRows, backupRows, postgresRows, auditRows, systemRows, settingRows, logRows] = await Promise.all([
      api.projects(),
      api.deployments(),
      api.containers().catch(() => []),
      api.nodes().catch(() => []),
      api.backups().catch(() => []),
      api.postgresBackupTargets().catch(() => []),
      api.auditLog().catch(() => []),
      api.systemUsage().catch(() => null),
      api.settings(),
      api.systemLogs().catch(() => "")
    ]);
    setProjects(projectRows);
    setDeployments(deploymentRows);
    setContainers(containerRows);
    setNodes(nodeRows);
    setBackups(backupRows);
    setPostgresTargets(postgresRows);
    setAuditEntries(auditRows);
    setUsage(systemRows);
    setSettings(settingRows);
    setSystemLogs(logRows);
  }, []);

  const loadView = useCallback(async (targetView: View) => {
    if (targetView === "dashboard") {
      const [projectRows, deploymentRows, containerRows, nodeRows, systemRows, settingRows] = await Promise.all([
        api.projects(),
        api.deployments(),
        api.containers().catch(() => []),
        api.nodes().catch(() => []),
        api.systemUsage().catch(() => null),
        api.settings()
      ]);
      setProjects(projectRows);
      setDeployments(deploymentRows);
      setContainers(containerRows);
      setNodes(nodeRows);
      setUsage(systemRows);
      setSettings(settingRows);
      return;
    }

    if (targetView === "projects") {
      const [projectRows, deploymentRows, containerRows, nodeRows, settingRows] = await Promise.all([api.projects(), api.deployments(), api.containers().catch(() => []), api.nodes().catch(() => []), api.settings()]);
      setProjects(projectRows);
      setDeployments(deploymentRows);
      setContainers(containerRows);
      setNodes(nodeRows);
      setSettings(settingRows);
      return;
    }

    if (targetView === "deployments") {
      setDeployments(await api.deployments());
      return;
    }

    if (targetView === "containers") {
      setContainers(await api.containers().catch(() => []));
      return;
    }

    if (targetView === "nodes") {
      setNodes(await api.nodes().catch(() => []));
      return;
    }

    if (targetView === "backups") {
      const [backupRows, postgresRows] = await Promise.all([api.backups().catch(() => []), api.postgresBackupTargets().catch(() => [])]);
      setBackups(backupRows);
      setPostgresTargets(postgresRows);
      return;
    }

    if (targetView === "audit") {
      setAuditEntries(await api.auditLog().catch(() => []));
      return;
    }

    const [settingRows, logRows] = await Promise.all([api.settings(), api.systemLogs().catch(() => "")]);
    setSettings(settingRows);
    setSystemLogs(logRows);
  }, []);

  useEffect(() => {
    api
      .me()
      .then((result) => {
        setUser(result.username);
        return loadView("dashboard");
      })
      .catch(() => setUser(null))
      .finally(() => setLoading(false));
  }, [loadView]);

  useEffect(() => {
    if (!user) return;
    const timer = window.setInterval(() => {
      void loadView(view).catch(() => undefined);
    }, 30000);
    return () => window.clearInterval(timer);
  }, [loadView, user, view]);

  useEffect(() => {
    if (!user) return;
    void loadView(view).catch(() => undefined);
  }, [loadView, user, view]);

  useEffect(() => {
    setR2Form({
      enabled: settings.r2?.enabled ?? false,
      accountId: settings.r2?.accountId ?? "",
      bucket: settings.r2?.bucket ?? "",
      accessKeyId: settings.r2?.accessKeyId ?? "",
      secretAccessKey: "",
      prefix: settings.r2?.prefix ?? "postgres-dumps"
    });
  }, [settings.r2]);

  useEffect(() => {
    setCfForm({
      accountId: settings.cf?.accountId ?? "",
      zoneId: settings.cf?.zoneId ?? "",
      apiToken: ""
    });
  }, [settings.cf]);

  useEffect(() => {
    if (!logModal?.streamPath) return;
    const streamPath = logModal.streamPath;
    const source = new EventSource(streamPath);

    source.onmessage = (event) => {
      let payload: LogStreamPayload;
      try {
        payload = JSON.parse(event.data) as LogStreamPayload;
      } catch {
        payload = { chunk: event.data };
      }
      setLogModal((current) => {
        if (!current || current.streamPath !== streamPath) return current;
        const nextLogs = payload.logs ?? `${current.logs}${payload.chunk ?? ""}${payload.error ? `\n${payload.error}\n` : ""}`;
        return {
          ...current,
          logs: nextLogs,
          status: payload.status ?? current.status,
          live: !payload.done
        };
      });
      if (payload.done) {
        source.close();
      }
    };

    source.onerror = () => {
      source.close();
      setLogModal((current) => (current?.streamPath === streamPath ? { ...current, live: false } : current));
    };

    return () => source.close();
  }, [logModal?.streamPath]);

  const runningDeployments = useMemo(() => deployments.filter((deployment) => deployment.status === "running"), [deployments]);
  const latestDeploymentByProject = useMemo(() => {
    const map = new Map<string, Deployment>();
    for (const deployment of deployments) {
      if (!map.has(deployment.projectId)) {
        map.set(deployment.projectId, deployment);
      }
    }
    return map;
  }, [deployments]);
  const failingProjects = useMemo(
    () => projects.filter((project) => latestDeploymentByProject.get(project.id)?.status === "failed"),
    [latestDeploymentByProject, projects]
  );
  const unhealthyContainers = useMemo(() => containers.filter((container) => !["running", "created"].includes(container.state)), [containers]);
  const warningDisks = useMemo(() => usage?.storage.filter((disk) => disk.usedPercent >= 80) ?? [], [usage]);
  const containersByProjectFolder = useMemo(() => {
    const map = new Map<string, ContainerInfo[]>();
    for (const container of containers) {
      if (!container.composeProject) continue;
      map.set(container.composeProject, [...(map.get(container.composeProject) ?? []), container]);
    }
    return map;
  }, [containers]);
  const nodeById = useMemo(() => new Map(nodes.map((node) => [node.id, node])), [nodes]);
  const nodeOptions = useMemo(() => (nodes.length ? nodes : [{ id: "node_master_local", name: "Master", role: "master", status: "online" } as DeploymentNode]).map((node) => ({
    label: `${node.name} (${node.role})`,
    value: node.id
  })), [nodes]);
  const visibleProjects = useMemo(() => pageItems(projects, projectPage), [projectPage, projects]);
  const visibleDeployments = useMemo(() => pageItems(deployments, deploymentPage), [deploymentPage, deployments]);
  const visibleBackups = useMemo(() => pageItems(backups, backupPage), [backupPage, backups]);
  const visibleAuditEntries = useMemo(() => pageItems(auditEntries, auditPage), [auditEntries, auditPage]);
  const r2Ready = Boolean(settings.r2?.enabled && settings.r2.accountId && settings.r2.bucket && settings.r2.accessKeyId && settings.r2.hasSecretAccessKey);

  useEffect(() => {
    setProjectPage((page) => Math.min(page, totalPages(projects)));
  }, [projects]);

  useEffect(() => {
    setDeploymentPage((page) => Math.min(page, totalPages(deployments)));
  }, [deployments]);

  useEffect(() => {
    setBackupPage((page) => Math.min(page, totalPages(backups)));
  }, [backups]);

  useEffect(() => {
    setAuditPage((page) => Math.min(page, totalPages(auditEntries)));
  }, [auditEntries]);

  async function submitLogin(event: FormEvent) {
    event.preventDefault();
    setBusy("login");
    try {
      const result = await api.login(login.username, login.password);
      setUser(result.username);
      await loadView("dashboard");
      setToast({ message: "Signed in." });
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to sign in.", kind: "error" });
    } finally {
      setBusy(null);
    }
  }

  function projectEnvPayload() {
    const rows = normalizeEnvRows(projectEnv.rows.filter((row) => row.key.trim()));
    return rows.map((row) => {
      const original = projectEnv.baseline.find((item) => item.key === row.key);
      if (original?.masked && original.value === row.value) {
        return { key: row.key, masked: row.masked };
      }
      return row;
    });
  }

  async function persistProjectDetails(event?: FormEvent, after?: "deploy" | "restart") {
    event?.preventDefault();
    if (!projectModal) return;
    setBusy("project");
    try {
      let savedProject: Project;
      if (projectModal === "new") {
        savedProject = await api.createProject(projectForm);
      } else {
        savedProject = await api.updateProject(projectModal.id, projectForm);
      }

      const envRows = projectEnvPayload();
      if (projectEnv.available && !projectEnv.loading && (projectModal !== "new" || envRows.length || projectEnv.content.trim())) {
        if (projectEnv.mode === "text") {
          await api.updateProjectEnvContent(savedProject.id, projectEnv.content);
        } else {
          await api.updateProjectEnv(savedProject.id, envRows);
        }
      }
      if (after === "restart") {
        await api.restartProject(savedProject.id);
      }
      if (after === "deploy") {
        await api.deployProject(savedProject.id);
      }
      setProjectModal(null);
      await loadAll();
      setToast({
        message:
          after === "restart"
            ? "Project saved and restart started."
            : after === "deploy"
              ? "Project saved and deployment started."
              : projectModal === "new"
                ? "Project registered."
                : "Project updated."
      });
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to save project.", kind: "error" });
    } finally {
      setBusy(null);
    }
  }

  async function saveProject(event: FormEvent) {
    await persistProjectDetails(event);
  }

  async function deploy(project: Project) {
    setBusy(`deploy:${project.id}`);
    try {
      const result = await api.deployProject(project.id);
      setToast({ message: result.reused ? "Deployment is already running." : "Deployment started." });
      setLogModal({
        title: `${project.name} deployment`,
        logs: "",
        streamPath: api.deploymentLogStream(result.deployment.id),
        live: true,
        status: result.deployment.status
      });
      await loadAll();
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to deploy project.", kind: "error" });
    } finally {
      setBusy(null);
    }
  }

  function openDeploymentLogs(deployment: Deployment) {
    setLogModal({
      title: `${deployment.projectName ?? deployment.projectId} deployment`,
      logs: "",
      streamPath: api.deploymentLogStream(deployment.id),
      live: true,
      status: deployment.status
    });
  }

  function openContainerLogs(container: ContainerInfo) {
    setLogModal({
      title: `${container.name} logs`,
      logs: "",
      streamPath: api.containerLogStream(container.id),
      live: true,
      status: container.state
    });
  }

  function openProject(project?: Project) {
    if (project) {
      setProjectForm({
        name: project.name,
        gitUrl: project.gitUrl ?? "",
        branch: project.branch,
        folderName: project.folderName,
        composeFile: project.composeFile,
        composeContent: project.composeContent ?? "",
        autoStart: project.autoStart,
        manualDeployEnabled: project.manualDeployEnabled,
        githubWebhookEnabled: project.githubWebhookEnabled,
        targetNodeId: project.targetNodeId || "node_master_local"
      });
      setProjectEnv({ ...emptyProjectEnvState, loading: true });
      setProjectModal(project);
      void Promise.all([api.projectEnv(project.id), api.projectEnvContent(project.id), api.projectCfRoutes(project.id).catch(() => [])])
        .then(([rows, envContent, routes]) => {
          const normalizedRows = normalizeEnvRows(rows);
          setProjectEnv({ rows: normalizedRows, baseline: normalizedRows, draftKey: "", draftValue: "", content: envContent.content, mode: "pairs", loading: false, available: true });
          setCfRoutes(routes);
        })
        .catch((error) => {
          setProjectEnv({ ...emptyProjectEnvState, loading: false, available: false });
          setToast({ message: error instanceof Error ? error.message : "Unable to load environment.", kind: "error" });
        });
      return;
    }
    setProjectForm({ ...emptyProject, targetNodeId: nodes[0]?.id ?? "node_master_local" });
    setProjectEnv(emptyProjectEnvState);
    setCfRoutes([]);
    setProjectModal("new");
  }

  function openRollback(project: Project) {
    const projectDeployments = deployments.filter((deployment) => deployment.projectId === project.id && deployment.status === "success");
    setRollbackModal({ project, deployments: projectDeployments });
  }

  async function copyText(value: string) {
    await navigator.clipboard.writeText(value);
    setToast({ message: "Copied." });
  }

  async function copyWorkerInstallCommand() {
    try {
      const result = await api.workerJoinToken();
      await copyText(result.command);
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to load worker command.", kind: "error" });
    }
  }

  async function dumpPostgresTarget(containerId?: string) {
    const busyKey = containerId ? `backup:${containerId}` : "backup:yanto";
    setBusy(busyKey);
    try {
      await api.createBackup(containerId);
      await loadAll();
      setToast({ message: "Postgres backup created." });
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to create backup.", kind: "error" });
    } finally {
      setBusy(null);
    }
  }

  async function restorePostgresTarget(target: PostgresTarget, file: File) {
    setConfirm({
      title: "Restore Postgres dump",
      body: `Replace ${target.databaseName} on ${target.containerName} with ${file.name}? The current public schema will be dropped before importing the dump.`,
      label: "Restore",
      danger: true,
      action: async () => {
        setBusy(`restore:${target.containerId}`);
        setToast({ message: "Restoring Postgres dump...", kind: "loading" });
        try {
          await api.restorePostgresTarget(target.containerId, file);
          await loadAll();
          setToast({ message: "Postgres dump restored." });
        } finally {
          setBusy(null);
        }
      }
    });
  }

  async function uploadBackupR2(backup: BackupRecord) {
    setBusy(`r2:${backup.id}`);
    setToast({ message: "Uploading dump to Cloudflare R2...", kind: "loading" });
    try {
      const result = await api.uploadBackupToR2(backup.id);
      setToast({ message: `Uploaded to R2: ${result.key}` });
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to upload to R2.", kind: "error" });
    } finally {
      setBusy(null);
    }
  }

  async function saveR2Settings(event: FormEvent) {
    event.preventDefault();
    setBusy("r2-settings");
    try {
      const result = await api.saveR2Settings(r2Form);
      setSettings((current) => ({ ...current, r2: result.r2 }));
      setToast({ message: "R2 settings saved." });
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to save R2 settings.", kind: "error" });
    } finally {
      setBusy(null);
    }
  }

  async function saveCfSettings(event: FormEvent) {
    event.preventDefault();
    setBusy("cf-settings");
    try {
      const result = await api.saveCloudflareSettings(cfForm);
      setSettings((current) => ({ ...current, cf: result.cf }));
      setToast({ message: "Cloudflare settings saved." });
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to save Cloudflare settings.", kind: "error" });
    } finally {
      setBusy(null);
    }
  }

  async function validateCfSettings() {
    setBusy("cf-validate");
    setToast({ message: "Validating Cloudflare credentials...", kind: "loading" });
    try {
      const result = await api.validateCloudflareSettings(cfForm);
      setToast({ message: `Validated. Account: ${result.accountName}${result.zoneName ? `, Zone: ${result.zoneName}` : ""}` });
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Validation failed.", kind: "error" });
    } finally {
      setBusy(null);
    }
  }

  const [cfRouteForm, setCfRouteForm] = useState({ hostname: "", serviceTarget: "" });

  async function publishCfRoute(projectId: string) {
    setBusy("cf-route-publish");
    try {
      const payload: CloudflareRoutePayload = { hostname: cfRouteForm.hostname, serviceTarget: cfRouteForm.serviceTarget };
      const route = await api.publishCfRoute(projectId, payload);
      setCfRoutes((current) => [...current, route]);
      setCfRouteForm({ hostname: "", serviceTarget: "" });
      setToast({ message: `Route published: https://${route.hostname}` });
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to publish route.", kind: "error" });
    } finally {
      setBusy(null);
    }
  }

  async function toggleCfRoute(route: CloudflareRoute) {
    try {
      const updated = route.enabled ? await api.disableCfRoute(route.id) : await api.enableCfRoute(route.id);
      setCfRoutes((current) => current.map((r) => (r.id === updated.id ? updated : r)));
      setToast({ message: route.enabled ? "Route disabled." : "Route enabled." });
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to update route.", kind: "error" });
    }
  }

  async function removeCfRoute(routeId: string) {
    try {
      await api.deleteCfRoute(routeId);
      setCfRoutes((current) => current.filter((r) => r.id !== routeId));
      setToast({ message: "Route deleted." });
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to delete route.", kind: "error" });
    }
  }

  async function previewCleanup() {
    setBusy("cleanup-preview");
    setToast({ message: "Checking cleanup preview...", kind: "loading" });
    setCleanupLogTitle("Cleanup preview");
    setCleanupLogs("Checking reclaimable Docker space...");
    try {
      const result = await api.cleanupPreview();
      setCleanupPreviewed(true);
      setCleanupLogs(result.logs);
      setToast({ message: "Cleanup preview ready." });
    } catch (error) {
      setCleanupPreviewed(false);
      setCleanupLogs(error instanceof Error ? error.message : "Unable to preview cleanup.");
      setToast({ message: error instanceof Error ? error.message : "Unable to preview cleanup.", kind: "error" });
    } finally {
      setBusy(null);
    }
  }

  async function saveSshPrivateKey(event: FormEvent) {
    event.preventDefault();
    setBusy("ssh-key");
    try {
      await api.saveSshKey(sshPrivateKey);
      setSshPrivateKey("");
      await loadAll();
      setToast({ message: "SSH key saved." });
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to save SSH key.", kind: "error" });
    } finally {
      setBusy(null);
    }
  }

  if (loading) {
    return (
      <main className="login-shell">
        <LoadingInline label="Starting Yanto" />
      </main>
    );
  }

  if (!user) {
    return (
      <main className="login-shell">
        <form className="login-panel" onSubmit={submitLogin}>
          <h1>Yanto Deploy</h1>
          <p>Sign in to manage projects, containers, deployments, and host cleanup.</p>
          <TextField label="Username" value={login.username} onChange={(username) => setLogin((current) => ({ ...current, username }))} required />
          <TextField label="Password" type="password" value={login.password} onChange={(password) => setLogin((current) => ({ ...current, password }))} required />
          <Button type="submit" disabled={busy === "login"} icon={busy === "login" ? <RefreshCw size={16} className="spin" /> : <KeyRound size={16} />}>
            Sign in
          </Button>
        </form>
        {toast ? <Toast {...toast} onClose={() => setToast(null)} /> : null}
      </main>
    );
  }

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <div>
            <strong>Yanto</strong>
            <span>{settings.hostProjectsRoot}</span>
          </div>
        </div>
        <nav>
          {[
            ["dashboard", Activity, "Dashboard"],
            ["projects", GitBranch, "Projects"],
            ["deployments", Boxes, "Deployments"],
            ["containers", Container, "Containers"],
            ["nodes", Server, "Nodes"],
            ["backups", Archive, "Backups"],
            ["audit", FileClock, "Audit"],
            ["settings", Settings, "Settings"]
          ].map(([id, Icon, label]) => (
            <button key={id as string} className={view === id ? "active" : ""} type="button" onClick={() => setView(id as View)}>
              <Icon size={17} />
              <span>{label as string}</span>
            </button>
          ))}
        </nav>
        <button
          className="logout"
          type="button"
          onClick={async () => {
            await api.logout();
            setUser(null);
          }}
        >
          <LogOut size={17} />
          <span>Sign out</span>
        </button>
      </aside>

      <main className="main">
        <header className="topbar">
          <div>
            <h1>{view[0].toUpperCase() + view.slice(1)}</h1>
          </div>
          <Button variant="secondary" onClick={() => void loadView(view)} icon={<RefreshCw size={16} />}>
            Refresh
          </Button>
        </header>

        {view === "dashboard" ? (
          <section className="dashboard">
            <section className="stat-grid">
              <StatTile label="Projects" value={projects.length} detail={`${settings.hostProjectsRoot} root`} />
              <StatTile label="Nodes" value={nodes.length || 1} detail={`${nodes.filter((node) => node.status === "online").length || 1} online`} />
              {runningDeployments.length ? <StatTile label="Active deploys" value={runningDeployments.length} detail="Deployment in progress" /> : null}
              <StatTile label="Running containers" value={containers.filter((container) => container.state === "running").length} detail={`${containers.length} total containers`} />
              <StatTile label="RAM used" value={usage ? `${usage.memory.usedPercent}%` : "-"} detail={usage ? `${bytes(usage.memory.used)} of ${bytes(usage.memory.total)}` : "Unavailable"} />
            </section>

            <div className="dashboard-main-grid">
              <div className="dashboard-left-column">
                <WarningsPanel failingProjects={failingProjects} unhealthyContainers={unhealthyContainers} warningDisks={warningDisks} />
                <UsagePanel usage={usage} />
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
        ) : null}

        {view === "projects" ? (
          <section className="panel">
            <div className="panel-head">
              <h2>Registered projects</h2>
              <Button onClick={() => openProject()} icon={<Plus size={16} />}>
                Add project
              </Button>
            </div>
            <div className="project-grid">
              {visibleProjects.map((project) => {
                const projectContainers = containersByProjectFolder.get(project.folderName) ?? [];
                return (
                <article
                  className="project-card"
                  key={project.id}
                  role="button"
                  tabIndex={0}
                  onClick={() => openProject(project)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter" || event.key === " ") {
                      event.preventDefault();
                      openProject(project);
                    }
                  }}
                >
                  <div className="project-card-main">
                    <div className="project-card-head">
                      <div>
                        <h3>{project.name}</h3>
                        <p>{project.gitUrl || "Compose file project"}</p>
                      </div>
                      <StatusBadge status={deployments.find((deployment) => deployment.projectId === project.id)?.status ?? "ready"} />
                    </div>
                    <dl>
                      <div>
                        <dt>Branch</dt>
                        <dd>{project.gitUrl ? project.branch : "-"}</dd>
                      </div>
                      <div>
                        <dt>Folder</dt>
                        <dd>{project.localPath}</dd>
                      </div>
                      <div>
                        <dt>Compose</dt>
                        <dd>{project.composeFile}</dd>
                      </div>
                      <div>
                        <dt>Node</dt>
                        <dd>{nodeById.get(project.targetNodeId)?.name ?? project.targetNodeId}</dd>
                      </div>
                      <div>
                        <dt>Auto start</dt>
                        <dd>{project.autoStart ? "On restart" : "Off"}</dd>
                      </div>
                      <div>
                        <dt>Manual API</dt>
                        <dd>{project.manualDeployEnabled ? "On" : "Off"}</dd>
                      </div>
                      <div>
                        <dt>GitHub hook</dt>
                        <dd>{project.githubWebhookEnabled ? "On" : "Off"}</dd>
                      </div>
                    </dl>
                  </div>
                  <div className="project-card-side">
                    <div className="endpoint-box" onClick={(event) => event.stopPropagation()}>
                      <span>{endpoint(project, settings.appBaseUrl)}</span>
                      <button type="button" onClick={() => void copyText(endpoint(project, settings.appBaseUrl))} title="Copy endpoint" aria-label="Copy endpoint">
                        <Copy size={15} />
                      </button>
                    </div>
                    <div className="endpoint-box" onClick={(event) => event.stopPropagation()}>
                      <span>{githubWebhookEndpoint(project, settings.appBaseUrl)}</span>
                      <button type="button" onClick={() => void copyText(githubWebhookEndpoint(project, settings.appBaseUrl))} title="Copy GitHub webhook URL" aria-label="Copy GitHub webhook URL">
                        <Copy size={15} />
                      </button>
                    </div>
                    <div className="token-box" onClick={(event) => event.stopPropagation()}>
                      <span>Secret {project.deployToken}</span>
                      <button type="button" onClick={() => void copyText(project.deployToken)} title="Copy secret" aria-label="Copy secret">
                        <Copy size={15} />
                      </button>
                    </div>
                    <div className="project-container-list">
                      <span>Containers</span>
                      {projectContainers.length ? (
                        <div>
                          {projectContainers.slice(0, 4).map((container) => (
                            <span className="project-container-row" key={container.id}>
                              <strong>{container.name}</strong>
                              <StatusBadge status={container.state} />
                            </span>
                          ))}
                          {projectContainers.length > 4 ? <small>+{projectContainers.length - 4} more</small> : null}
                        </div>
                      ) : (
                        <small>No containers</small>
                      )}
                    </div>
                    <div className="actions" onClick={(event) => event.stopPropagation()}>
                      <Button variant="secondary" onClick={() => openRollback(project)} icon={<Undo2 size={15} />}>
                        Rollback
                      </Button>
                      <Button disabled={busy === `deploy:${project.id}` || !project.manualDeployEnabled} onClick={() => void deploy(project)} icon={<Play size={15} />}>
                        Deploy
                      </Button>
                      <Button
                        variant="secondary"
                        onClick={() =>
                          setConfirm({
                            title: "Stop project",
                            body: `Stop containers for ${project.name}?`,
                            label: "Stop",
                            danger: true,
                            action: async () => {
                              await api.stopProject(project.id);
                              await loadAll();
                            }
                          })
                        }
                        icon={<Square size={15} />}
                      >
                        Stop
                      </Button>
                      <Button
                        variant="danger"
                        onClick={() =>
                          setConfirm({
                            title: "Remove project",
                            body: "This removes the project record and deployment logs. The project folder is left untouched.",
                            label: "Remove",
                            danger: true,
                            action: async () => {
                              await api.deleteProject(project.id);
                              await loadAll();
                            }
                          })
                        }
                      >
                        Remove
                      </Button>
                    </div>
                  </div>
                </article>
                );
              })}
            </div>
            <Pagination label="Projects" page={projectPage} totalItems={projects.length} onPageChange={setProjectPage} />
          </section>
        ) : null}

        {view === "deployments" ? (
          <section className="panel">
            <div className="panel-head">
              <h2>Deployment history</h2>
              <span className="count">{deployments.length} records</span>
            </div>
            <DeploymentTable deployments={visibleDeployments} onLogs={openDeploymentLogs} />
            <Pagination label="Deployments" page={deploymentPage} totalItems={deployments.length} onPageChange={setDeploymentPage} />
          </section>
        ) : null}

        {view === "backups" ? (
          <div className="backup-layout">
            <section className="panel">
              <div className="panel-head">
                <h2>Postgres targets</h2>
                <div className="actions">
                  <span className="count">{postgresTargets.length} detected</span>
                  <Button disabled={busy === "backup:yanto"} onClick={() => void dumpPostgresTarget()} icon={<Archive size={16} />}>
                    Dump Yanto DB
                  </Button>
                </div>
              </div>
              <PostgresTargetTable targets={postgresTargets} busy={busy} onDump={dumpPostgresTarget} onRestore={restorePostgresTarget} />
            </section>
            <section className="panel">
              <div className="panel-head">
                <h2>Backup history</h2>
                <span className="count">{backups.length} dumps</span>
              </div>
              <BackupTable
                backups={visibleBackups}
                busy={busy}
                r2Ready={r2Ready}
                onUploadR2={uploadBackupR2}
                onDelete={(backup) =>
                  setConfirm({
                    title: "Remove backup",
                    body: `Remove ${backup.filename || backup.id}? The dump file will be deleted from disk.`,
                    label: "Remove",
                    danger: true,
                    action: async () => {
                      await api.deleteBackup(backup.id);
                      await loadAll();
                    }
                  })
                }
              />
              <Pagination label="Backups" page={backupPage} totalItems={backups.length} onPageChange={setBackupPage} />
            </section>
          </div>
        ) : null}

        {view === "audit" ? (
          <section className="panel">
            <div className="panel-head">
              <h2>Audit log</h2>
              <span className="count">{auditEntries.length} events</span>
            </div>
            <AuditTable entries={visibleAuditEntries} />
            <Pagination label="Audit events" page={auditPage} totalItems={auditEntries.length} onPageChange={setAuditPage} />
          </section>
        ) : null}

        {view === "containers" ? (
          <section className="panel">
            <div className="panel-head">
              <h2>Docker containers</h2>
              <span className="count">{containers.length} found</span>
            </div>
            <ContainerGroups containers={containers} onLogs={openContainerLogs} onConfirm={(next) => setConfirm(next)} onReload={loadAll} />
          </section>
        ) : null}

        {view === "nodes" ? (
          <section className="panel">
            <div className="panel-head">
              <h2>Deployment nodes</h2>
              <span className="count">{nodes.length} registered</span>
            </div>
            <div className="table-wrap">
              <table>
                <thead>
                  <tr>
                    <th>Name</th>
                    <th>Role</th>
                    <th>Status</th>
                    <th>Docker</th>
                    <th>Projects</th>
                    <th>Active</th>
                    <th>Last seen</th>
                  </tr>
                </thead>
                <tbody>
                  {nodes.map((node) => (
                    <tr key={node.id}>
                      <td>{node.name}</td>
                      <td>{node.role}</td>
                      <td><StatusBadge status={node.status} /></td>
                      <td>{node.dockerVersion ?? "-"}</td>
                      <td>{node.projectCount ?? 0}</td>
                      <td>{node.runningDeploymentCount ?? 0}</td>
                      <td>{node.lastSeenAt ? dateTime(node.lastSeenAt) : "-"}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            {!nodes.length ? <p className="muted">No nodes registered yet.</p> : null}
          </section>
        ) : null}

        {view === "settings" ? (
          <section className="settings-grid">
            <section className="panel webhook-settings compact-settings-panel">
              <div className="panel-head">
                <h2>Deployment webhook</h2>
                <GitBranch size={19} />
              </div>
              <div className="settings-code-list">
                <div>
                  <dt>Endpoint</dt>
                  <div className="endpoint-box">
                    <span>{`${settings.appBaseUrl.replace(/\/$/, "")}/deploy?id=<project-id>`}</span>
                    <button type="button" onClick={() => void copyText(`${settings.appBaseUrl.replace(/\/$/, "")}/deploy?id=<project-id>`)} title="Copy endpoint" aria-label="Copy endpoint">
                      <Copy size={15} />
                    </button>
                  </div>
                </div>
                <div>
                  <dt>Auth header</dt>
                  <div className="token-box">
                    <span>Authorization: Bearer &lt;project-deploy-token&gt;</span>
                    <button type="button" onClick={() => void copyText("Authorization: Bearer <project-deploy-token>")} title="Copy auth header" aria-label="Copy auth header">
                      <Copy size={15} />
                    </button>
                  </div>
                </div>
              </div>
            </section>
            <section className="panel webhook-settings compact-settings-panel">
              <div className="panel-head">
                <h2>Worker install</h2>
                <Server size={19} />
              </div>
              <Button variant="secondary" onClick={() => void copyWorkerInstallCommand()} icon={<Copy size={16} />}>
                Copy worker command
              </Button>
            </section>
            <section className="panel r2-settings-panel">
              <div className="panel-head">
                <h2>Cloudflare R2</h2>
                <Cloud size={19} />
              </div>
              <form className="form-grid compact-form" onSubmit={saveR2Settings}>
                <ToggleField
                  label="Upload enabled"
                  value={r2Form.enabled}
                  onChange={(enabled) => setR2Form((current) => ({ ...current, enabled }))}
                  description={settings.r2?.hasSecretAccessKey ? "Secret key saved" : "Add an R2 secret key before uploading"}
                />
                <div className="settings-form-pair">
                  <TextField label="Account ID" value={r2Form.accountId} onChange={(accountId) => setR2Form((current) => ({ ...current, accountId }))} />
                  <TextField label="Bucket" value={r2Form.bucket} onChange={(bucket) => setR2Form((current) => ({ ...current, bucket }))} />
                </div>
                <div className="settings-form-pair">
                  <TextField label="Access key ID" value={r2Form.accessKeyId} onChange={(accessKeyId) => setR2Form((current) => ({ ...current, accessKeyId }))} />
                  <TextField
                    label="Secret access key"
                    type="password"
                    value={r2Form.secretAccessKey}
                    onChange={(secretAccessKey) => setR2Form((current) => ({ ...current, secretAccessKey }))}
                    placeholder={settings.r2?.hasSecretAccessKey ? "Saved; leave blank to keep" : ""}
                  />
                </div>
                <TextField label="Object prefix" value={r2Form.prefix} onChange={(prefix) => setR2Form((current) => ({ ...current, prefix }))} />
                <div className="actions">
                  <Button type="submit" disabled={busy === "r2-settings"} icon={<Cloud size={16} />}>
                    Save R2
                  </Button>
                </div>
              </form>
            </section>
            <section className="panel cf-tunnel-settings-panel">
              <div className="panel-head">
                <h2>Cloudflare Tunnel</h2>
                <ShieldCheck size={19} />
              </div>
              <form className="form-grid compact-form" onSubmit={saveCfSettings}>
                <div className="settings-form-pair">
                  <TextField label="Account ID" value={cfForm.accountId} onChange={(accountId) => setCfForm((current) => ({ ...current, accountId }))} />
                  <TextField label="Zone ID" value={cfForm.zoneId} onChange={(zoneId) => setCfForm((current) => ({ ...current, zoneId }))} />
                </div>
                <TextField
                  label="API Token"
                  type="password"
                  value={cfForm.apiToken}
                  onChange={(apiToken) => setCfForm((current) => ({ ...current, apiToken }))}
                  placeholder={settings.cf?.hasApiToken ? "Saved; leave blank to keep" : ""}
                />
                <div className="actions">
                  <Button variant="secondary" disabled={busy === "cf-validate"} onClick={() => void validateCfSettings()}>
                    Validate
                  </Button>
                  <Button type="submit" disabled={busy === "cf-settings"} icon={<ShieldCheck size={16} />}>
                    Save
                  </Button>
                </div>
              </form>
            </section>
            <section className="panel ssh-settings-panel">
              <div className="panel-head">
                <h2>Git SSH key</h2>
                <KeyRound size={19} />
              </div>
              <dl className="settings-list ssh-status-list">
                <div>
                  <dt>Active key path</dt>
                  <dd>{settings.sshKey?.activePrivateKeyPath ?? "No key found"}</dd>
                </div>
                <div>
                  <dt>Managed key</dt>
                  <dd>{settings.sshKey?.hasManagedKey ? "Saved in app volume" : "Not saved"}</dd>
                </div>
                <div>
                  <dt>Mounted VPS key</dt>
                  <dd>{settings.sshKey?.hasMountedKey ? settings.sshKey.mountedPrivateKeyPath : "Not found"}</dd>
                </div>
              </dl>
              {settings.sshKey?.publicKey ? (
                <div className="token-box ssh-public-key-box">
                  <span>{settings.sshKey.publicKey}</span>
                  <button type="button" onClick={() => void copyText(settings.sshKey?.publicKey ?? "")} title="Copy public key" aria-label="Copy public key">
                    <Copy size={15} />
                  </button>
                </div>
              ) : null}
              <form className="form-grid ssh-key-form" onSubmit={saveSshPrivateKey}>
                <TextAreaField
                  label="Private key"
                  value={sshPrivateKey}
                  onChange={setSshPrivateKey}
                  placeholder={"Paste the full private key, starting with -----BEGIN OPENSSH PRIVATE KEY-----"}
                />
                <div className="actions">
                  <Button type="submit" disabled={busy === "ssh-key" || !sshPrivateKey.trim()} icon={<KeyRound size={16} />}>
                    Save SSH key
                  </Button>
                </div>
              </form>
            </section>
            <section className="panel runtime-settings-panel">
              <div className="panel-head">
                <h2>Runtime</h2>
              </div>
              <dl className="settings-list">
                <div>
                  <dt>Container projects root</dt>
                  <dd>{settings.projectsRoot}</dd>
                </div>
                <div>
                  <dt>Host projects root</dt>
                  <dd>{settings.hostProjectsRoot}</dd>
                </div>
                <div>
                  <dt>SSH keys</dt>
                  <dd>{settings.sshKeysDir}</dd>
                </div>
                <div>
                  <dt>Base URL</dt>
                  <dd>{settings.appBaseUrl}</dd>
                </div>
              </dl>
            </section>
            <section className="panel">
              <div className="panel-head">
                <h2>Cleanup</h2>
                <DatabaseZap size={19} />
              </div>
              <p className="muted">Preview reclaimable Docker space first, then clean protected unused cache and resources.</p>
              <div className="actions">
                <Button
                  variant="secondary"
                  disabled={busy === "cleanup-preview" || busy === "cleanup"}
                  onClick={() => void previewCleanup()}
                  icon={busy === "cleanup-preview" ? <RefreshCw size={16} className="spin" /> : <DatabaseZap size={16} />}
                >
                  {busy === "cleanup-preview" ? "Checking" : "Preview cleanup"}
                </Button>
                <Button
                  variant="danger"
                  disabled={busy === "cleanup-preview" || busy === "cleanup"}
                  onClick={() => {
                    if (!cleanupPreviewed) {
                      setToast({ message: "Run preview cleanup first, then clean cache.", kind: "error" });
                      return;
                    }
                    setConfirm({
                      title: "Run cleanup",
                      body: "This removes unused Docker cache and unused Docker resources shown by the preview. Running containers, named volumes, and Yanto containers are protected.",
                      label: "Clean cache",
                      danger: true,
                      action: async () => {
                        setBusy("cleanup");
                        setToast({ message: "Cleaning Docker cache...", kind: "loading" });
                        setCleanupLogTitle("Cleanup logs");
                        setCleanupLogs("Cleaning unused Docker cache and resources...");
                        try {
                          const result = await api.cleanup();
                          setCleanupPreviewed(false);
                          setCleanupLogs(result.logs);
                          await loadAll();
                          setToast({ message: "Cleanup completed." });
                        } finally {
                          setBusy(null);
                        }
                      }
                    });
                  }}
                  icon={busy === "cleanup" ? <RefreshCw size={16} className="spin" /> : <Trash2 size={16} />}
                >
                  Clean cache
                </Button>
              </div>
              <div className="cleanup-result">
                <div className="cleanup-result-head">
                  <strong>{cleanupLogTitle}</strong>
                  <StatusBadge status={cleanupPreviewed ? "ready" : busy === "cleanup-preview" || busy === "cleanup" ? "running" : "idle"} />
                </div>
                <LogViewer logs={cleanupLogs || "No cleanup preview yet."} />
              </div>
            </section>
            <section className="panel system-log-panel">
              <div className="panel-head">
                <h2>System log</h2>
                <Button variant="secondary" onClick={() => void api.systemLogs().then(setSystemLogs)} icon={<RefreshCw size={16} />}>
                  Refresh
                </Button>
              </div>
              <LogViewer logs={systemLogs || "No system log entries recorded yet."} />
            </section>
          </section>
        ) : null}
      </main>

      {projectModal ? (
        <Modal title={projectModal === "new" ? "Add project" : "Edit project"} size="wide" onClose={() => setProjectModal(null)}>
          <form className="project-edit-form" onSubmit={saveProject}>
            <div className="project-edit-layout">
              <section className="project-edit-section">
                <div className="section-kicker">Project</div>
                <TextField label="Name" value={projectForm.name} onChange={(name) => setProjectForm((current) => ({ ...current, name }))} required />
                <TextField label="Git SSH URL" value={projectForm.gitUrl} onChange={(gitUrl) => setProjectForm((current) => ({ ...current, gitUrl }))} placeholder="Optional: git@github.com:user/repo.git" />
                <div className="project-edit-pair">
                  <TextField label="Branch" value={projectForm.branch} onChange={(branch) => setProjectForm((current) => ({ ...current, branch }))} required />
                  <TextField label="Compose file" value={projectForm.composeFile} onChange={(composeFile) => setProjectForm((current) => ({ ...current, composeFile }))} placeholder="docker-compose.yml" required />
                </div>
                <TextField label="Folder name" value={projectForm.folderName} onChange={(folderName) => setProjectForm((current) => ({ ...current, folderName }))} placeholder={slugifyFolderName(projectForm.name) || "Auto from project name"} />
                <CustomSelect label="Deployment node" value={projectForm.targetNodeId} options={nodeOptions} onChange={(targetNodeId) => setProjectForm((current) => ({ ...current, targetNodeId }))} />
                <ToggleField
                  label="Auto start after restart"
                  value={projectForm.autoStart}
                  onChange={(autoStart) => setProjectForm((current) => ({ ...current, autoStart }))}
                  description="Deploy with a Yanto compose override that sets restart: unless-stopped."
                />
                <ToggleField
                  label="Manual API deployments"
                  value={projectForm.manualDeployEnabled}
                  onChange={(manualDeployEnabled) => setProjectForm((current) => ({ ...current, manualDeployEnabled }))}
                  description="Allow deployments from the authenticated deploy action and token endpoint."
                />
                <ToggleField
                  label="GitHub webhook deployments"
                  value={projectForm.githubWebhookEnabled}
                  onChange={(githubWebhookEnabled) => setProjectForm((current) => ({ ...current, githubWebhookEnabled }))}
                  description="Allow signed GitHub push webhooks to deploy this project."
                />
              </section>

              <section className="project-edit-section compose-section">
                <div className="section-kicker">Compose</div>
                <TextAreaField
                  label="Compose editor"
                  value={projectForm.composeContent}
                  onChange={(composeContent) => setProjectForm((current) => ({ ...current, composeContent }))}
                  placeholder={"Optional. Paste docker-compose.yml content here for compose-only projects or to override the file during deploy."}
                />
              </section>

              <section className="project-edit-section env-section">
                <div className="section-kicker">Environment</div>
                {projectEnv.loading ? (
                  <LoadingInline label="Loading environment" />
                ) : projectEnv.available ? (
                  <EnvEditor modal={projectEnv} onChange={setProjectEnv} />
                ) : (
                  <p className="muted">Environment could not be loaded. Project fields can still be saved.</p>
                )}
              </section>

              {projectModal !== "new" && settings.cf?.hasApiToken ? (
                <section className="project-edit-section cf-routes-section">
                  <div className="section-kicker">Cloudflare Tunnel</div>
                  {cfRoutes.length > 0 ? (
                    <div className="cf-route-list">
                      {cfRoutes.map((route) => (
                        <div key={route.id} className="cf-route-row">
                          <div className="cf-route-info">
                            <span className="cf-route-hostname">
                              <a href={`https://${route.hostname}`} target="_blank" rel="noopener noreferrer">
                                https://{route.hostname}
                              </a>
                              <IconButton label="Copy URL" onClick={() => void copyText(`https://${route.hostname}`)}><Copy size={14} /></IconButton>
                            </span>
                            <span className="cf-route-service">{route.serviceTarget}</span>
                            <StatusBadge status={route.enabled ? "enabled" : "disabled"} />
                          </div>
                          <div className="cf-route-actions">
                            <Button variant="ghost" onClick={() => void toggleCfRoute(route)}>
                              {route.enabled ? "Disable" : "Enable"}
                            </Button>
                            <Button variant="ghost" onClick={() => void removeCfRoute(route.id)} icon={<Trash2 size={14} />}>
                              Delete
                            </Button>
                          </div>
                        </div>
                      ))}
                    </div>
                  ) : (
                    <p className="muted">No public hostnames configured for this project.</p>
                  )}
                  <div className="cf-route-add-form">
                    <div className="project-edit-pair">
                      <TextField label="Hostname" value={cfRouteForm.hostname} onChange={(hostname) => setCfRouteForm((current) => ({ ...current, hostname }))} placeholder="app.example.com" />
                      <TextField label="Service URL" value={cfRouteForm.serviceTarget} onChange={(serviceTarget) => setCfRouteForm((current) => ({ ...current, serviceTarget }))} placeholder="http://127.0.0.1:3000" />
                    </div>
                    <Button disabled={busy === "cf-route-publish" || !cfRouteForm.hostname || !cfRouteForm.serviceTarget} variant="secondary" onClick={() => void publishCfRoute(projectModal.id)} icon={<Plus size={15} />}>
                      Publish route
                    </Button>
                  </div>
                </section>
              ) : null}
            </div>
            <div className="actions project-edit-actions">
              <Button variant="secondary" onClick={() => setProjectModal(null)}>
                Cancel
              </Button>
              {projectModal !== "new" ? (
                <Button type="button" variant="secondary" disabled={busy === "project" || projectEnv.loading} onClick={() => void persistProjectDetails(undefined, "restart")} icon={<RotateCw size={15} />}>
                  Save & Restart
                </Button>
              ) : null}
              <Button type="button" variant="secondary" disabled={busy === "project" || projectEnv.loading || !projectForm.manualDeployEnabled} onClick={() => void persistProjectDetails(undefined, "deploy")} icon={<Play size={15} />}>
                Save & Deploy
              </Button>
              <Button type="submit" disabled={busy === "project" || projectEnv.loading}>
                Save project
              </Button>
            </div>
          </form>
        </Modal>
      ) : null}

      {rollbackModal ? (
        <Modal title={`Rollback ${rollbackModal.project.name}`} onClose={() => setRollbackModal(null)}>
          <div className="rollback-list">
            {rollbackModal.deployments.map((deployment) => (
              <button
                type="button"
                key={deployment.id}
                onClick={async () => {
                  setBusy(`rollback:${rollbackModal.project.id}`);
                  try {
                    const result = await api.rollbackProject(rollbackModal.project.id, deployment.id);
                    setRollbackModal(null);
                    setToast({ message: "Rollback started." });
                    setLogModal({
                      title: `${rollbackModal.project.name} rollback`,
                      logs: "",
                      streamPath: api.deploymentLogStream(result.deployment.id),
                      live: true,
                      status: result.deployment.status
                    });
                    await loadAll();
                  } catch (error) {
                    setToast({ message: error instanceof Error ? error.message : "Unable to start rollback.", kind: "error" });
                  } finally {
                    setBusy(null);
                  }
                }}
              >
                <span>{dateTime(deployment.startedAt)}</span>
                <strong>{durationBetween(deployment.startedAt, deployment.finishedAt)}</strong>
                <StatusBadge status={deployment.status} />
              </button>
            ))}
            {!rollbackModal.deployments.length ? <p className="muted">No successful deployments are available for rollback yet.</p> : null}
          </div>
        </Modal>
      ) : null}

      {logModal ? (
        <Modal title={logModal.title} onClose={() => setLogModal(null)}>
          {logModal.streamPath ? (
            <div className="log-status-line">
              <StatusBadge status={logModal.live ? "live" : logModal.status ?? "closed"} />
              <span>{logModal.live ? "Streaming logs" : "Log stream closed"}</span>
            </div>
          ) : null}
          <LogViewer logs={logModal.logs} />
        </Modal>
      ) : null}

      {confirm ? (
        <ConfirmDialog
          title={confirm.title}
          body={confirm.body}
          confirmLabel={confirm.label}
          danger={confirm.danger}
          onClose={() => setConfirm(null)}
          onConfirm={() => {
            const action = confirm.action;
            setConfirm(null);
            action()
              .then(() => {
                setToast({ message: "Action completed." });
              })
              .catch((error) => {
                setToast({ message: error instanceof Error ? error.message : "Action failed.", kind: "error" });
              });
          }}
        />
      ) : null}

      {toast ? <Toast {...toast} onClose={() => setToast(null)} /> : null}
    </div>
  );
}

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
  warningDisks
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

function Pagination({
  label,
  page,
  totalItems,
  onPageChange
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

function EnvEditor({ modal, onChange }: { modal: ProjectEnvState; onChange: (next: ProjectEnvState) => void }) {
  const baselineKeys = new Set(modal.baseline.map((row) => row.key));
  const currentKeys = new Set(modal.rows.map((row) => row.key));
  const changedRows = modal.rows.filter((row) => {
    const original = modal.baseline.find((item) => item.key === row.key);
    return !original || original.value !== row.value || original.masked !== row.masked;
  });
  const removedRows = modal.baseline.filter((row) => !currentKeys.has(row.key));
  const setRows = (rows: ProjectEnvVariable[], patch: Partial<ProjectEnvState> = {}) => onChange({ ...modal, ...patch, rows, content: serializeEnvRows(rows) });
  const setMode = (mode: EnvEditMode) => {
    if (mode === modal.mode) return;
    onChange(mode === "text" ? { ...modal, mode } : { ...modal, mode, rows: parseEnvContentRows(modal.content) });
  };

  return (
    <div className="env-editor">
      <div className="env-mode-toggle" role="group" aria-label="Environment input mode">
        <button type="button" className={modal.mode === "pairs" ? "active" : ""} onClick={() => setMode("pairs")}>
          <List size={15} />
          <span>Key/value</span>
        </button>
        <button type="button" className={modal.mode === "text" ? "active" : ""} onClick={() => setMode("text")}>
          <FileText size={15} />
          <span>Text</span>
        </button>
      </div>
      {modal.mode === "text" ? (
        <TextAreaField label="Environment text" value={modal.content} onChange={(content) => onChange({ ...modal, content })} />
      ) : (
        <>
          <div className="env-rows">
            {modal.rows.map((row, index) => (
              <div className="env-row" key={`${row.key}:${index}`}>
                <TextField label="Key" value={row.key} onChange={(key) => setRows(modal.rows.map((item, rowIndex) => (rowIndex === index ? { ...item, key } : item)))} />
                <TextField
                  label="Value"
                  type={row.masked ? "password" : "text"}
                  value={row.value ?? ""}
                  onChange={(value) => setRows(modal.rows.map((item, rowIndex) => (rowIndex === index ? { ...item, value } : item)))}
                />
                <ToggleField label="Masked" value={Boolean(row.masked)} onChange={(masked) => setRows(modal.rows.map((item, rowIndex) => (rowIndex === index ? { ...item, masked } : item)))} />
                <IconButton label="Remove variable" variant="danger" onClick={() => setRows(modal.rows.filter((_, rowIndex) => rowIndex !== index))}>
                  <Trash2 size={15} />
                </IconButton>
              </div>
            ))}
          </div>
          <div className="env-add-row">
            <TextField label="New key" value={modal.draftKey} onChange={(draftKey) => onChange({ ...modal, draftKey })} />
            <TextField label="New value" type="password" value={modal.draftValue} onChange={(draftValue) => onChange({ ...modal, draftValue })} />
            <Button
              variant="secondary"
              onClick={() => {
                const key = modal.draftKey.trim();
                if (!key) return;
                setRows(normalizeEnvRows([...modal.rows, { key, value: modal.draftValue, masked: true }]), { draftKey: "", draftValue: "" });
              }}
              icon={<Plus size={15} />}
            >
              Add
            </Button>
          </div>
          <div className="env-diff">
            <div className="section-kicker">Masked diff</div>
            {[...changedRows, ...removedRows].map((row) => {
              const removed = !currentKeys.has(row.key);
              const created = !baselineKeys.has(row.key);
              return (
                <div key={`${row.key}:diff`}>
                  <ShieldCheck size={15} />
                  <span>{row.key}</span>
                  <strong>{removed ? "removed" : created ? "added" : "updated"}</strong>
                </div>
              );
            })}
            {!changedRows.length && !removedRows.length ? <p className="muted">No pending environment changes.</p> : null}
          </div>
        </>
      )}
    </div>
  );
}
