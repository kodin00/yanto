import {
  Activity,
  Archive,
  Boxes,
  ChevronLeft,
  ChevronRight,
  Container,
  Cloud,
  Copy,
  FileClock,
  FileText,
  GitBranch,
  GitPullRequest,
  Globe2,
  KeyRound,
  List,
  LogOut,
  Moon,
  Network,
  Play,
  Plus,
  RefreshCw,
  Server,
  Settings,
  ShieldCheck,
  Sun,
  Trash2
} from "lucide-react";
import { DashboardView } from "./views";
import { FormEvent, lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";

const AuditView = lazy(() => import("./views/AuditView").then(m => ({ default: m.AuditView })));
const BackupsView = lazy(() => import("./views/BackupsView").then(m => ({ default: m.BackupsView })));
const ContainersView = lazy(() => import("./views/ContainersView").then(m => ({ default: m.ContainersView })));
const DeploymentsView = lazy(() => import("./views/DeploymentsView").then(m => ({ default: m.DeploymentsView })));
const DnsView = lazy(() => import("./views/DnsView").then(m => ({ default: m.DnsView })));
const HostnamesView = lazy(() => import("./views/HostnamesView").then(m => ({ default: m.HostnamesView })));
const FrpView = lazy(() => import("./views/FrpView").then(m => ({ default: m.FrpView })));
const NodesView = lazy(() => import("./views/NodesView").then(m => ({ default: m.NodesView })));
const ProjectsView = lazy(() => import("./views/ProjectsView").then(m => ({ default: m.ProjectsView })));
const SettingsView = lazy(() => import("./views/SettingsView").then(m => ({ default: m.SettingsView })));
import type { CloudflareClient, CloudflareDnsRecord, CloudflarePublicSettings, CloudflareRoute, CloudflareRouteDiagnostic, ContainerInfo, Deployment, DeploymentNode, MultiNodePublicSettings, Project, ProjectWithDeployToken, R2PublicSettings, RollbackPreview, SetupWizardStatus, SystemUsage } from "../shared/types";
import {
  cloudflareServiceUrl,
  endpoint,
  githubRepoNameFromUrl,
  githubWebhookEndpoint,
  normalizeEnvRows,
  pageItems,
  slugifyFolderName,
  totalPages
} from "./app-utils";
import { Button, ConfirmDialog, CustomSelect, IconButton, LoadingInline, LogViewer, Modal, StatusBadge, TextAreaField, TextField, Toast, ToggleField } from "./components/ui";
import { EnvEditor, type ProjectEnvState } from "./components/EnvEditor";
import { YantoBootLoader } from "./components/YantoBootLoader";
import { api, type AuditLogEntry, type BackupRecord, type CloudflareDnsRecordPayload, type CloudflareRoutePayload, type PostgresTarget } from "./lib/api";
import packageJson from "../../package.json";

type View = "dashboard" | "projects" | "deployments" | "containers" | "nodes" | "backups" | "hostnames" | "frp" | "dns" | "audit" | "settings";
type ToastState = { message: string; kind?: "ok" | "error" | "loading" } | null;
type LogModalState = { title: string; logs: string; streamPath?: string; live?: boolean; status?: string };
type LogStreamPayload = { logs?: string; chunk?: string; status?: string; error?: string; done?: boolean };
type CfRouteProtocol = "http" | "https";
type CfRouteForm = { hostname: string; protocol: CfRouteProtocol; localTarget: string; noTlsVerify: boolean };
type ProjectComposeState = { open: boolean; loading: boolean; available: boolean; source: "saved" | "file" | "empty" | null; message: string };
type RollbackModalState = { project: Project; targetRef: string; preview: RollbackPreview | null; previewError: string | null; previewLoading: boolean };
type ConfirmState = { title: string; body: string; label: string; danger?: boolean; loadingMessage?: string; successMessage?: string; action: () => Promise<void> };
type CreatedProjectSecret = { projectName: string; deployUrl: string; webhookUrl: string; deployToken: string };
type ThemeMode = "light" | "dark";
type SetupStep = "intro" | "ssh" | "cloudflare" | "r2";

const themeStorageKey = "yanto-theme";
const appVersion = packageJson.version;

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

const emptyR2Settings: R2PublicSettings = {
  enabled: false,
  accountId: "",
  bucket: "",
  maskedAccessKeyId: "",
  hasAccessKeyId: false,
  hasSecretAccessKey: false,
  prefix: "postgres-dumps"
};

const emptyCfSettings: CloudflarePublicSettings = {
  accountId: "",
  zoneId: "",
  hasApiToken: false
};

const emptySetupWizardStatus: SetupWizardStatus = {
  completedAt: null,
  dismissedAt: null,
  updatedAt: null
};

const emptyMultiNodeSettings: MultiNodePublicSettings = {
  enabled: false,
  releaseStage: "beta"
};

const setupSteps: SetupStep[] = ["intro", "ssh", "cloudflare", "r2"];

const emptyProjectEnvState: ProjectEnvState = {
  rows: [],
  baseline: [],
  draftKey: "",
  draftValue: "",
  content: "",
  mode: "text",
  loading: false,
  available: true,
  opened: false
};

const emptyProjectComposeState: ProjectComposeState = {
  open: false,
  loading: false,
  available: true,
  source: null,
  message: ""
};

function getInitialTheme(): ThemeMode {
  if (typeof window === "undefined") return "light";
  const storedTheme = window.localStorage.getItem(themeStorageKey);
  if (storedTheme === "light" || storedTheme === "dark") return storedTheme;
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

function parseCfServiceTarget(serviceTarget: string): Pick<CfRouteForm, "protocol" | "localTarget"> {
  const match = serviceTarget.trim().match(/^(https?):\/\/(.+)$/);
  if (!match) return { protocol: "http", localTarget: serviceTarget.trim() };
  return { protocol: match[1] as CfRouteProtocol, localTarget: match[2] };
}

function buildCfRouteForm(hostname = "", serviceTarget = "", noTlsVerify = false): CfRouteForm {
  const parsed = parseCfServiceTarget(serviceTarget);
  return {
    hostname,
    protocol: parsed.protocol,
    localTarget: parsed.localTarget,
    noTlsVerify: parsed.protocol === "https" ? noTlsVerify : false
  };
}

function cfRouteServiceTarget(form: CfRouteForm) {
  const parsed = parseCfServiceTarget(form.localTarget);
  return `${parsed.protocol === "https" ? "https" : form.protocol}://${parsed.localTarget}`;
}

function projectWithoutSecret(project: Project | ProjectWithDeployToken): Project {
  const copy: Partial<ProjectWithDeployToken> = { ...project };
  delete copy.deployToken;
  return copy as Project;
}

function viewTitle(view: View) {
  return view === "dns" ? "DNS" : view[0].toUpperCase() + view.slice(1);
}

function shortSha(sha: string) {
  return sha.slice(0, 12);
}

export function App() {
  const [user, setUser] = useState<string | null>(null);
  const [login, setLogin] = useState({ username: "admin", password: "" });
  const [view, setView] = useState<View>("dashboard");
  const [theme, setTheme] = useState<ThemeMode>(getInitialTheme);
  const [projects, setProjects] = useState<Project[]>([]);
  const [deployments, setDeployments] = useState<Deployment[]>([]);
  const [containers, setContainers] = useState<ContainerInfo[]>([]);
  const [nodes, setNodes] = useState<DeploymentNode[]>([]);
  const [backups, setBackups] = useState<BackupRecord[]>([]);
  const [cloudflareClients, setCloudflareClients] = useState<CloudflareClient[]>([]);
  const [dnsClientId, setDnsClientId] = useState("");
  const [dnsRecords, setDnsRecords] = useState<CloudflareDnsRecord[]>([]);
  const [dnsLoaded, setDnsLoaded] = useState(false);
  const dnsClientIdRef = useRef("");
  const dnsLoadRequestRef = useRef(0);
  const [routeDiagnostics, setRouteDiagnostics] = useState<CloudflareRouteDiagnostic[]>([]);
  const [postgresTargets, setPostgresTargets] = useState<PostgresTarget[]>([]);
  const [auditEntries, setAuditEntries] = useState<AuditLogEntry[]>([]);
  const [usage, setUsage] = useState<SystemUsage | null>(null);
  const [settings, setSettings] = useState({ projectsRoot: "/projects", hostProjectsRoot: "~/projects", sshKeysDir: "", appBaseUrl: "", sshKey: emptySshKeySettings, r2: emptyR2Settings, cf: emptyCfSettings, setupWizard: emptySetupWizardStatus, multiNode: emptyMultiNodeSettings });
  const [settingsLoaded, setSettingsLoaded] = useState(false);
  const [r2Form, setR2Form] = useState({ enabled: false, accountId: "", bucket: "", accessKeyId: "", secretAccessKey: "", prefix: "postgres-dumps" });
  const [r2FormDirty, setR2FormDirty] = useState(false);
  const [cfForm, setCfForm] = useState({ accountId: "", zoneId: "", apiToken: "" });
  const [cfFormDirty, setCfFormDirty] = useState(false);
  const [cfRoutes, setCfRoutes] = useState<CloudflareRoute[]>([]);
  const [systemLogs, setSystemLogs] = useState("");
  const [cleanupLogs, setCleanupLogs] = useState("");
  const [cleanupLogTitle, setCleanupLogTitle] = useState("Cleanup preview");
  const [cleanupPreviewed, setCleanupPreviewed] = useState(false);
  const [loading, setLoading] = useState(true);
  const [viewLoading, setViewLoading] = useState<Partial<Record<View, boolean>>>({});
  const [toast, setToast] = useState<ToastState>(null);
  const [projectModal, setProjectModal] = useState<Project | "new" | null>(null);
  const [projectForm, setProjectForm] = useState(emptyProject);
  const [projectEnv, setProjectEnv] = useState<ProjectEnvState>(emptyProjectEnvState);
  const [projectCompose, setProjectCompose] = useState<ProjectComposeState>(emptyProjectComposeState);
  const [projectEditorModal, setProjectEditorModal] = useState<"compose" | "env" | null>(null);
  const [cfRoutesByProject, setCfRoutesByProject] = useState<Record<string, CloudflareRoute[]>>({});
  const [cfRouteEditorOpen, setCfRouteEditorOpen] = useState(false);
  const [rollbackModal, setRollbackModal] = useState<RollbackModalState | null>(null);
  const [logModal, setLogModal] = useState<LogModalState | null>(null);
  const [confirm, setConfirm] = useState<ConfirmState | null>(null);
  const [createdProjectSecret, setCreatedProjectSecret] = useState<CreatedProjectSecret | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [sshPrivateKey, setSshPrivateKey] = useState("");
  const [setupModalOpen, setSetupModalOpen] = useState(false);
  const [setupStep, setSetupStep] = useState<SetupStep>("intro");
  const [setupAutoPrompted, setSetupAutoPrompted] = useState(false);
  const [projectPage, setProjectPage] = useState(1);
  const [deploymentPage, setDeploymentPage] = useState(1);
  const [backupPage, setBackupPage] = useState(1);
  const [dnsPage, setDnsPage] = useState(1);
  const [auditPage, setAuditPage] = useState(1);
  const [frpRefreshKey, setFrpRefreshKey] = useState(0);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    document.documentElement.style.colorScheme = theme;
    window.localStorage.setItem(themeStorageKey, theme);
  }, [theme]);

  const fetchContainerRows = useCallback(async () => {
    try {
      return await api.containers();
    } catch {
      return null;
    }
  }, []);

  const refreshProjects = useCallback(async () => {
    setViewLoading((current) => ({ ...current, projects: true }));
    try {
      const [projectRows, deploymentRows, containerRows, diagnostics] = await Promise.all([
        api.projects(),
        api.deployments(),
        fetchContainerRows(),
        api.cloudflareRouteDiagnostics().catch(() => [])
      ]);
      setProjects(projectRows);
      setDeployments(deploymentRows);
      if (containerRows) setContainers(containerRows);
      setRouteDiagnostics(diagnostics);
    } finally {
      setViewLoading((current) => ({ ...current, projects: false }));
    }
  }, [fetchContainerRows]);

  const refreshDeployments = useCallback(async () => {
    setViewLoading((current) => ({ ...current, deployments: true }));
    try {
      setDeployments(await api.deployments());
    } finally {
      setViewLoading((current) => ({ ...current, deployments: false }));
    }
  }, []);

  const refreshBackups = useCallback(async () => {
    setViewLoading((current) => ({ ...current, backups: true }));
    try {
      const [backupRows, postgresRows] = await Promise.all([
        api.backups().catch(() => []),
        api.postgresBackupTargets().catch(() => [])
      ]);
      setBackups(backupRows);
      setPostgresTargets(postgresRows);
    } finally {
      setViewLoading((current) => ({ ...current, backups: false }));
    }
  }, []);

  const refreshSettings = useCallback(async () => {
    const settingRows = await api.settings().catch(() => null);
    if (settingRows) setSettings(settingRows);
  }, []);

  const refreshRouteDiagnostics = useCallback(async () => {
    setRouteDiagnostics(await api.cloudflareRouteDiagnostics().catch(() => []));
  }, []);

  const refreshContainers = useCallback(async () => {
    setViewLoading((current) => ({ ...current, containers: true }));
    try {
      const containerRows = await fetchContainerRows();
      if (containerRows) setContainers(containerRows);
    } finally {
      setViewLoading((current) => ({ ...current, containers: false }));
    }
  }, [fetchContainerRows]);

  const loadView = useCallback(async (targetView: View) => {
    if (targetView === "dashboard") {
      const [projectRows, deploymentRows, containerRows, nodeRows, systemRows, settingRows] = await Promise.all([
        api.projects(),
        api.deployments(),
        fetchContainerRows(),
        api.nodes().catch(() => []),
        api.systemUsage().catch(() => null),
        api.settings()
      ]);
      setProjects(projectRows);
      setDeployments(deploymentRows);
      if (containerRows) setContainers(containerRows);
      setNodes(nodeRows);
      setUsage(systemRows);
      setSettings(settingRows);
      setSettingsLoaded(true);
      return;
    }

    if (targetView === "projects") {
      const [projectRows, deploymentRows, containerRows, nodeRows, settingRows, diagnostics] = await Promise.all([
        api.projects(),
        api.deployments(),
        fetchContainerRows(),
        api.nodes().catch(() => []),
        api.settings(),
        api.cloudflareRouteDiagnostics().catch(() => [])
      ]);
      setProjects(projectRows);
      setDeployments(deploymentRows);
      if (containerRows) setContainers(containerRows);
      setNodes(nodeRows);
      setSettings(settingRows);
      setRouteDiagnostics(diagnostics);
      setSettingsLoaded(true);
      return;
    }

    if (targetView === "deployments") {
      setDeployments(await api.deployments());
      return;
    }

    if (targetView === "containers") {
      const containerRows = await fetchContainerRows();
      if (containerRows) setContainers(containerRows);
      return;
    }

    if (targetView === "hostnames") {
      const [projectRows, containerRows] = await Promise.all([api.projects(), fetchContainerRows()]);
      setProjects(projectRows);
      if (containerRows) setContainers(containerRows);
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

    if (targetView === "dns") {
      const requestId = ++dnsLoadRequestRef.current;
      const [settingRows, clients, diagnostics] = await Promise.all([
        api.settings(),
        api.cloudflareClients().catch(() => []),
        api.cloudflareRouteDiagnostics().catch(() => [])
      ]);
      const currentClientId = dnsClientIdRef.current;
      const selectedClientId = currentClientId && clients.some((client) => client.id === currentClientId) ? currentClientId : clients[0]?.id ?? "";
      const records = selectedClientId ? await api.cloudflareClientDnsRecords(selectedClientId).catch(() => []) : [];
      if (requestId !== dnsLoadRequestRef.current) return;
      dnsClientIdRef.current = selectedClientId;
      setSettings(settingRows);
      setSettingsLoaded(true);
      setCloudflareClients(clients);
      setDnsClientId(selectedClientId);
      setDnsRecords(records);
      setRouteDiagnostics(diagnostics);
      setDnsLoaded(true);
      return;
    }

    if (targetView === "audit") {
      setAuditEntries(await api.auditLog().catch(() => []));
      return;
    }

    if (targetView === "frp") {
      return;
    }

    const [settingRows, logRows] = await Promise.all([api.settings(), api.systemLogs().catch(() => "")]);
    setSettings(settingRows);
    setSettingsLoaded(true);
    setSystemLogs(logRows);
  }, [fetchContainerRows]);

  const loadViewWithState = useCallback(async (targetView: View, showLoading = true) => {
    if (showLoading) {
      setViewLoading((current) => ({ ...current, [targetView]: true }));
    }
    try {
      await loadView(targetView);
    } finally {
      if (showLoading) {
        setViewLoading((current) => ({ ...current, [targetView]: false }));
      }
    }
  }, [loadView]);

  useEffect(() => {
    api
      .me()
      .then((result) => {
        setUser(result.username);
        return loadViewWithState("dashboard", false);
      })
      .catch(() => setUser(null))
      .finally(() => setLoading(false));
  }, [loadViewWithState]);

  useEffect(() => {
    if (!user) return;
    const timer = window.setInterval(() => {
      void loadViewWithState(view, false).catch(() => undefined);
    }, 30000);
    return () => window.clearInterval(timer);
  }, [loadViewWithState, user, view]);

  useEffect(() => {
    if (!user) return;
    void loadViewWithState(view).catch(() => undefined);
  }, [loadViewWithState, user, view]);

  useEffect(() => {
    if (r2FormDirty) return;
    setR2Form({
      enabled: settings.r2?.enabled ?? false,
      accountId: settings.r2?.accountId ?? "",
      bucket: settings.r2?.bucket ?? "",
      accessKeyId: "",
      secretAccessKey: "",
      prefix: settings.r2?.prefix ?? "postgres-dumps"
    });
  }, [r2FormDirty, settings.r2]);

  useEffect(() => {
    if (cfFormDirty) return;
    setCfForm({
      accountId: settings.cf?.accountId ?? "",
      zoneId: settings.cf?.zoneId ?? "",
      apiToken: ""
    });
  }, [cfFormDirty, settings.cf]);

  useEffect(() => {
    if (!user || !settingsLoaded || setupAutoPrompted) return;
    if (settings.setupWizard.completedAt || settings.setupWizard.dismissedAt) return;
    setSetupStep("intro");
    setSetupModalOpen(true);
    setSetupAutoPrompted(true);
  }, [settings.setupWizard.completedAt, settings.setupWizard.dismissedAt, settingsLoaded, setupAutoPrompted, user]);

  useEffect(() => {
    if (view !== "settings") return;
    window.requestAnimationFrame(() => {
      const activeElement = document.activeElement;
      if (activeElement instanceof HTMLElement && activeElement.closest(".settings-grid")) {
        activeElement.blur();
      }
    });
  }, [view]);

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
  const multiNodeEnabled = settings.multiNode?.enabled ?? false;
  const nodeOptions = useMemo(() => (nodes.length ? nodes : [{ id: "node_master_local", name: "Master", role: "master", status: "online" } as DeploymentNode]).map((node) => ({
    label: `${node.name} (${node.role})`,
    value: node.id
  })), [nodes]);
  const visibleProjects = useMemo(() => pageItems(projects, projectPage), [projectPage, projects]);
  const visibleDeployments = useMemo(() => pageItems(deployments, deploymentPage), [deploymentPage, deployments]);
  const visibleBackups = useMemo(() => pageItems(backups, backupPage), [backupPage, backups]);
  const visibleDnsRecords = useMemo(() => pageItems(dnsRecords, dnsPage), [dnsPage, dnsRecords]);
  const visibleAuditEntries = useMemo(() => pageItems(auditEntries, auditPage), [auditEntries, auditPage]);
  const routeDiagnosticsByRouteId = useMemo(() => Object.fromEntries(routeDiagnostics.map((diagnostic) => [diagnostic.routeId, diagnostic])), [routeDiagnostics]);
  const sshReady = Boolean(settings.sshKey?.activePrivateKeyPath);
  const r2Ready = Boolean(settings.r2?.enabled && settings.r2.accountId && settings.r2.bucket && settings.r2.hasAccessKeyId && settings.r2.hasSecretAccessKey);
  const cfSettingsReady = Boolean(settings.cf?.accountId && settings.cf.zoneId && settings.cf.hasApiToken);
  const setupStepIndex = setupSteps.indexOf(setupStep);
  const setupCanGoBack = setupStepIndex > 0;
  const setupCanGoNext = setupStepIndex < setupSteps.length - 1;
  const setupCanReopen = settingsLoaded && !setupModalOpen && !settings.setupWizard.completedAt && Boolean(settings.setupWizard.dismissedAt);

  useEffect(() => {
    const routeEntries = projects.map((project) => [project.id, project.cloudflareRoutes ?? []] as const);
    setCfRoutesByProject((current) => ({ ...current, ...Object.fromEntries(routeEntries) }));
  }, [projects]);

  useEffect(() => {
    setProjectPage((page) => Math.min(page, totalPages(projects)));
  }, [projects]);

  useEffect(() => {
    setDeploymentPage((page) => Math.min(page, totalPages(deployments)));
  }, [deployments]);

  useEffect(() => {
    if (!multiNodeEnabled && view === "nodes") {
      setView("settings");
    }
  }, [multiNodeEnabled, view]);

  useEffect(() => {
    setBackupPage((page) => Math.min(page, totalPages(backups)));
  }, [backups]);

  useEffect(() => {
    setDnsPage((page) => Math.min(page, totalPages(dnsRecords)));
  }, [dnsRecords]);

  useEffect(() => {
    setAuditPage((page) => Math.min(page, totalPages(auditEntries)));
  }, [auditEntries]);

  async function submitLogin(event: FormEvent) {
    event.preventDefault();
    setBusy("login");
    setToast({ message: "Signing in...", kind: "loading" });
    try {
      const result = await api.login(login.username, login.password);
      setUser(result.username);
      await loadViewWithState("dashboard", false);
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

  function updateR2Form(patch: Partial<typeof r2Form>) {
    setR2FormDirty(true);
    setR2Form((current) => ({ ...current, ...patch }));
  }

  function updateCfForm(patch: Partial<typeof cfForm>) {
    setCfFormDirty(true);
    setCfForm((current) => ({ ...current, ...patch }));
  }

  function updateSavedProjectLocally(savedProject: Project | ProjectWithDeployToken) {
    const publicSavedProject = projectWithoutSecret(savedProject);
    setProjects((current) => {
      const existing = current.find((project) => project.id === publicSavedProject.id);
      const nextProject = {
        ...existing,
        ...publicSavedProject,
        containerCount: publicSavedProject.containerCount ?? existing?.containerCount ?? 0,
        cloudflareRoutes: publicSavedProject.cloudflareRoutes ?? existing?.cloudflareRoutes ?? []
      };
      return existing ? current.map((project) => (project.id === nextProject.id ? nextProject : project)) : [nextProject, ...current];
    });
  }

  async function persistProjectDetails(event?: FormEvent, after?: "deploy" | "restart") {
    event?.preventDefault();
    if (!projectModal) return;
    setBusy("project");
    setToast({ message: after === "deploy" ? "Saving project and starting deployment..." : after === "restart" ? "Saving project and restarting..." : "Saving project...", kind: "loading" });
    try {
      const creatingProject = projectModal === "new";
      let savedProject: Project | ProjectWithDeployToken;
      if (projectModal === "new") {
        savedProject = await api.createProject(projectForm);
      } else {
        savedProject = await api.updateProject(projectModal.id, projectForm);
      }

      const envRows = projectEnvPayload();
      const shouldSaveEnv = projectEnv.opened && projectEnv.available && !projectEnv.loading && (!creatingProject || envRows.length || projectEnv.content.trim());
      const pendingDeployEnv =
        shouldSaveEnv && creatingProject && after === "deploy"
          ? projectEnv.mode === "text"
            ? { envContent: projectEnv.content }
            : { envVariables: envRows }
          : undefined;
      if (shouldSaveEnv && !pendingDeployEnv) {
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
        const result = await api.deployProject(savedProject.id, pendingDeployEnv);
        setDeployments((current) => [
          { ...result.deployment, projectName: savedProject.name },
          ...current.filter((deployment) => deployment.id !== result.deployment.id)
        ]);
      }
      setProjectModal(null);
      setProjectEditorModal(null);
      updateSavedProjectLocally(savedProject);
      void refreshProjects().catch(() => undefined);
      if (creatingProject && "deployToken" in savedProject) {
        setCreatedProjectSecret({
          projectName: savedProject.name,
          deployUrl: endpoint(savedProject, settings.appBaseUrl),
          webhookUrl: githubWebhookEndpoint(savedProject, settings.appBaseUrl),
          deployToken: savedProject.deployToken
        });
      }
      setToast({
        message:
          after === "restart"
            ? "Project saved and restart started."
            : after === "deploy"
              ? "Project saved and deployment started."
              : creatingProject
                ? "Project registered. Save the one-time deploy token."
                : "Project updated."
      });
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to save project.", kind: "error" });
    } finally {
      setBusy(null);
    }
  }

  function updateProjectGitUrl(gitUrl: string) {
    setProjectForm((current) => {
      const previousRepoName = githubRepoNameFromUrl(current.gitUrl);
      const nextRepoName = githubRepoNameFromUrl(gitUrl);
      const shouldAutofillName = Boolean(nextRepoName) && (!current.name.trim() || (Boolean(previousRepoName) && current.name === previousRepoName));
      return {
        ...current,
        gitUrl,
        name: shouldAutofillName ? nextRepoName : current.name
      };
    });
  }

  async function saveProject(event: FormEvent) {
    await persistProjectDetails(event);
  }

  async function deploy(project: Project) {
    setBusy(`deploy:${project.id}`);
    setToast({ message: `Starting deployment for ${project.name}...`, kind: "loading" });
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
      await refreshDeployments();
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to deploy project.", kind: "error" });
    } finally {
      setBusy(null);
    }
  }

  async function retryDeployment(deployment: Deployment) {
    setBusy(`deploy:${deployment.projectId}`);
    setToast({ message: `Retrying deployment for ${deployment.projectName ?? deployment.projectId}...`, kind: "loading" });
    try {
      const result = await api.deployProject(deployment.projectId);
      setToast({ message: result.reused ? "Deployment is already running." : "Deployment retry started." });
      setLogModal({
        title: `${deployment.projectName ?? deployment.projectId} deployment`,
        logs: "",
        streamPath: api.deploymentLogStream(result.deployment.id),
        live: true,
        status: result.deployment.status
      });
      await refreshDeployments();
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to retry deployment.", kind: "error" });
    } finally {
      setBusy(null);
    }
  }

  async function copyDeployToken(project: Project) {
    setBusy(`token:${project.id}`);
    setToast({ message: `Copying deploy secret for ${project.name}...`, kind: "loading" });
    try {
      const { deployToken } = await api.projectDeployToken(project.id);
      await copyText(deployToken);
      setToast({ message: "Project deploy secret copied." });
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to copy deploy secret.", kind: "error" });
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
    setProjectEditorModal(null);
    setCfRouteEditorOpen(false);
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
      setProjectEnv(emptyProjectEnvState);
      setProjectCompose(emptyProjectComposeState);
      const projectRoutes = project.cloudflareRoutes ?? [];
      const detectedServiceTarget = cloudflareServiceUrl(project, containersByProjectFolder.get(project.folderName) ?? []);
      setCfRouteForm(buildCfRouteForm(projectRoutes[0]?.hostname ?? "", projectRoutes[0]?.serviceTarget || detectedServiceTarget, projectRoutes[0]?.noTlsVerify ?? false));
      setCfRoutes(projectRoutes);
      setProjectModal(project);
      void api.projectCfRoutes(project.id).catch(() => [])
        .then((routes) => {
          setCfRoutes(routes);
          setCfRoutesByProject((current) => ({ ...current, [project.id]: routes }));
          if (routes.length) {
            setCfRouteForm(buildCfRouteForm(routes[0].hostname, routes[0].serviceTarget, routes[0].noTlsVerify));
          }
        })
        .catch((error) => {
          setToast({ message: error instanceof Error ? error.message : "Unable to load Cloudflare routes.", kind: "error" });
        });
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
    if (projectCompose.open) {
      setProjectEditorModal("compose");
      return;
    }
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
    setToast({ message: "Loading compose file...", kind: "loading" });
    try {
      const compose = await api.projectComposeContent(projectModal.id);
      setProjectForm((current) => ({ ...current, composeContent: compose.content }));
      setProjectCompose({
        open: true,
        loading: false,
        available: true,
        source: compose.exists ? "file" : "empty",
        message: compose.exists ? `Loaded ${compose.composeFile}` : `${compose.composeFile} not found`
      });
      setProjectEditorModal("compose");
      setToast(null);
    } catch (error) {
      setProjectCompose({ open: false, loading: false, available: false, source: null, message: "Compose could not be loaded" });
      setToast({ message: error instanceof Error ? error.message : "Unable to load compose file.", kind: "error" });
    }
  }

  async function openEnvEditor() {
    if (!projectModal || projectEnv.loading) return;
    if (projectEnv.opened) {
      setProjectEditorModal("env");
      return;
    }
    if (projectModal === "new") {
      setProjectEnv({ ...emptyProjectEnvState, opened: true });
      setProjectEditorModal("env");
      return;
    }

    setProjectEnv({ ...emptyProjectEnvState, opened: true, loading: true });
    setToast({ message: "Loading environment variables...", kind: "loading" });
    try {
      const [rows, envContent] = await Promise.all([api.projectEnv(projectModal.id), api.projectEnvContent(projectModal.id)]);
      const normalizedRows = normalizeEnvRows(rows);
      setProjectEnv({ rows: normalizedRows, baseline: normalizedRows, draftKey: "", draftValue: "", content: envContent.content, mode: "text", loading: false, available: true, opened: true });
      setProjectEditorModal("env");
      setToast(null);
    } catch (error) {
      setProjectEnv({ ...emptyProjectEnvState, loading: false, available: false, opened: true });
      setToast({ message: error instanceof Error ? error.message : "Unable to load environment.", kind: "error" });
    }
  }

  function openRollback(project: Project) {
    setRollbackModal({ project, targetRef: "", preview: null, previewError: null, previewLoading: false });
  }

  function updateRollbackTarget(targetRef: string) {
    setRollbackModal((current) => current ? { ...current, targetRef, preview: null, previewError: null } : current);
  }

  async function previewRollback() {
    if (!rollbackModal) return;
    const targetRef = rollbackModal.targetRef.trim();
    setRollbackModal((current) => current ? { ...current, previewLoading: true, previewError: null, preview: null } : current);
    try {
      const preview = await api.rollbackPreview(rollbackModal.project.id, targetRef);
      setRollbackModal((current) => current ? { ...current, targetRef, preview, previewLoading: false, previewError: null } : current);
    } catch (error) {
      setRollbackModal((current) => current ? { ...current, preview: null, previewLoading: false, previewError: error instanceof Error ? error.message : "Unable to preview rollback." } : current);
    }
  }

  async function startRollback() {
    if (!rollbackModal) return;
    const targetRef = rollbackModal.targetRef.trim();
    setBusy(`rollback:${rollbackModal.project.id}`);
    setToast({ message: `Starting rollback for ${rollbackModal.project.name}...`, kind: "loading" });
    try {
      const result = await api.rollbackProject(rollbackModal.project.id, targetRef);
      setRollbackModal(null);
      setToast({ message: "Rollback started." });
      setLogModal({
        title: `${rollbackModal.project.name} rollback`,
        logs: "",
        streamPath: api.deploymentLogStream(result.deployment.id),
        live: true,
        status: result.deployment.status
      });
      await refreshDeployments();
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to start rollback.", kind: "error" });
    } finally {
      setBusy(null);
    }
  }

  async function copyText(value: string) {
    await navigator.clipboard.writeText(value);
    setToast({ message: "Copied." });
  }

  async function copyWorkerInstallCommand() {
    setToast({ message: "Creating worker install command...", kind: "loading" });
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
    setToast({ message: "Creating Postgres backup...", kind: "loading" });
    try {
      await api.createBackup(containerId);
      await refreshBackups();
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
          await refreshBackups();
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
    setToast({ message: "Saving R2 settings...", kind: "loading" });
    try {
      const result = await api.saveR2Settings(r2Form);
      setR2Form({
        enabled: result.r2.enabled,
        accountId: result.r2.accountId,
        bucket: result.r2.bucket,
        accessKeyId: "",
        secretAccessKey: "",
        prefix: result.r2.prefix
      });
      setR2FormDirty(false);
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
    setToast({ message: "Saving Cloudflare settings...", kind: "loading" });
    try {
      const result = await api.saveCloudflareSettings(cfForm);
      setCfForm({ accountId: result.cf.accountId, zoneId: result.cf.zoneId, apiToken: "" });
      setCfFormDirty(false);
      setSettings((current) => ({ ...current, cf: result.cf }));
      setToast({ message: "Cloudflare settings saved." });
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to save Cloudflare settings.", kind: "error" });
    } finally {
      setBusy(null);
    }
  }

  async function saveMultiNodeSettings(enabled: boolean) {
    setBusy("multi-node-settings");
    setToast({ message: "Saving multi-node setting...", kind: "loading" });
    try {
      const result = await api.saveMultiNodeSettings({ enabled });
      setSettings((current) => ({ ...current, multiNode: result.multiNode }));
      setToast({ message: result.multiNode.enabled ? "Multi-node beta enabled." : "Multi-node beta disabled." });
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to save multi-node setting.", kind: "error" });
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

  const [cfRouteForm, setCfRouteForm] = useState<CfRouteForm>(buildCfRouteForm());

  async function publishCfRoute(projectId: string) {
    setBusy("cf-route-publish");
    setToast({ message: "Publishing Cloudflare route...", kind: "loading" });
    try {
      const serviceTarget = cfRouteServiceTarget(cfRouteForm);
      const payload: CloudflareRoutePayload = {
        hostname: cfRouteForm.hostname,
        serviceTarget,
        noTlsVerify: serviceTarget.startsWith("https://") ? cfRouteForm.noTlsVerify : false
      };
      const route = await api.publishCfRoute(projectId, payload);
      setCfRoutes([route]);
      setCfRoutesByProject((current) => ({ ...current, [projectId]: [route] }));
      setProjects((current) => current.map((project) => (project.id === projectId ? { ...project, cloudflareRoutes: [route] } : project)));
      const project = projects.find((item) => item.id === projectId);
      setCfRouteForm(buildCfRouteForm(route.hostname, route.serviceTarget || (project ? cloudflareServiceUrl(project, containersByProjectFolder.get(project.folderName) ?? []) : ""), route.noTlsVerify));
      setCfRouteEditorOpen(false);
      void refreshRouteDiagnostics();
      setToast({ message: `Hostname saved: https://${route.hostname}` });
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to publish route.", kind: "error" });
    } finally {
      setBusy(null);
    }
  }

  async function toggleCfRoute(route: CloudflareRoute) {
    setBusy(`cf-route-toggle:${route.id}`);
    setToast({ message: route.enabled ? "Disabling Cloudflare route..." : "Enabling Cloudflare route...", kind: "loading" });
    try {
      const updated = route.enabled ? await api.disableCfRoute(route.id) : await api.enableCfRoute(route.id);
      setCfRoutes((current) => current.map((r) => (r.id === updated.id ? updated : r)));
      if (updated.projectId) setCfRoutesByProject((current) => ({
        ...current,
        [updated.projectId!]: (current[updated.projectId!] ?? []).map((r) => (r.id === updated.id ? updated : r))
      }));
      setProjects((current) => current.map((project) => (project.id === updated.projectId ? { ...project, cloudflareRoutes: (project.cloudflareRoutes ?? []).map((r) => (r.id === updated.id ? updated : r)) } : project)));
      void refreshRouteDiagnostics();
      setToast({ message: route.enabled ? "Route disabled." : "Route enabled." });
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to update route.", kind: "error" });
    } finally {
      setBusy(null);
    }
  }

  async function removeCfRoute(routeId: string) {
    setBusy(`cf-route-delete:${routeId}`);
    setToast({ message: "Deleting Cloudflare route...", kind: "loading" });
    try {
      await api.deleteCfRoute(routeId);
      const deletedRoute = cfRoutes.find((route) => route.id === routeId);
      setCfRoutes((current) => current.filter((r) => r.id !== routeId));
      if (deletedRoute) {
        if (deletedRoute.projectId) setCfRoutesByProject((current) => ({ ...current, [deletedRoute.projectId!]: (current[deletedRoute.projectId!] ?? []).filter((r) => r.id !== routeId) }));
        setProjects((current) => current.map((project) => (project.id === deletedRoute.projectId ? { ...project, cloudflareRoutes: (project.cloudflareRoutes ?? []).filter((r) => r.id !== routeId) } : project)));
      }
      void refreshRouteDiagnostics();
      setToast({ message: "Route deleted." });
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to delete route.", kind: "error" });
    } finally {
      setBusy(null);
    }
  }

  async function selectDnsClient(clientId: string) {
    if (clientId === dnsClientIdRef.current) return;
    const requestId = ++dnsLoadRequestRef.current;
    dnsClientIdRef.current = clientId;
    setDnsClientId(clientId);
    setDnsPage(1);
    setViewLoading((current) => ({ ...current, dns: true }));
    try {
      const records = clientId ? await api.cloudflareClientDnsRecords(clientId) : [];
      if (requestId !== dnsLoadRequestRef.current) return;
      setDnsRecords(records);
    } catch (error) {
      if (requestId !== dnsLoadRequestRef.current) return;
      setToast({ message: error instanceof Error ? error.message : "Unable to load DNS records.", kind: "error" });
    } finally {
      if (requestId === dnsLoadRequestRef.current) {
        setViewLoading((current) => ({ ...current, dns: false }));
      }
    }
  }

  async function createDnsRecord(payload: CloudflareDnsRecordPayload) {
    if (!dnsClientId) throw new Error("Choose a Cloudflare client first.");
    setBusy("dns-save");
    setToast({ message: "Creating DNS record...", kind: "loading" });
    try {
      const record = await api.createCloudflareClientDnsRecord(dnsClientId, payload);
      setDnsRecords((current) => [record, ...current.filter((item) => item.id !== record.id)]);
      void refreshRouteDiagnostics();
      setToast({ message: "DNS record created." });
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to create DNS record.", kind: "error" });
      throw error;
    } finally {
      setBusy(null);
    }
  }

  async function updateDnsRecord(recordId: string, payload: CloudflareDnsRecordPayload) {
    if (!dnsClientId) throw new Error("Choose a Cloudflare client first.");
    setBusy("dns-save");
    setToast({ message: "Updating DNS record...", kind: "loading" });
    try {
      const record = await api.updateCloudflareClientDnsRecord(dnsClientId, recordId, payload);
      setDnsRecords((current) => current.map((item) => (item.id === record.id ? record : item)));
      void refreshRouteDiagnostics();
      setToast({ message: "DNS record updated." });
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to update DNS record.", kind: "error" });
      throw error;
    } finally {
      setBusy(null);
    }
  }

  async function deleteDnsRecord(record: CloudflareDnsRecord) {
    if (!dnsClientId) throw new Error("Choose a Cloudflare client first.");
    setBusy(`dns-delete:${record.id}`);
    try {
      await api.deleteCloudflareClientDnsRecord(dnsClientId, record.id);
      setDnsRecords((current) => current.filter((item) => item.id !== record.id));
      void refreshRouteDiagnostics();
    } finally {
      setBusy(null);
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
    setToast({ message: "Saving SSH key...", kind: "loading" });
    try {
      await api.saveSshKey(sshPrivateKey);
      setSshPrivateKey("");
      await refreshSettings();
      setToast({ message: "SSH key saved." });
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to save SSH key.", kind: "error" });
    } finally {
      setBusy(null);
    }
  }

  async function generateSshPrivateKey() {
    setBusy("ssh-key-generate");
    setToast({ message: "Generating SSH key...", kind: "loading" });
    try {
      const result = await api.generateSshKey();
      setSettings((current) => ({ ...current, sshKey: { ...current.sshKey, ...result.sshKey } }));
      setToast({ message: "SSH key generated. Copy the public key to GitHub." });
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to generate SSH key.", kind: "error" });
    } finally {
      setBusy(null);
    }
  }

  function openSetupWizard(step: SetupStep = "intro") {
    setSetupStep(step);
    setSetupModalOpen(true);
  }

  function goToNextSetupStep() {
    setSetupStep(setupSteps[Math.min(setupStepIndex + 1, setupSteps.length - 1)]);
  }

  function goToPreviousSetupStep() {
    setSetupStep(setupSteps[Math.max(setupStepIndex - 1, 0)]);
  }

  async function saveSetupWizard(action: "completed" | "dismissed") {
    setBusy(`setup-${action}`);
    try {
      const result = await api.saveSetupWizard(action);
      setSettings((current) => ({ ...current, setupWizard: result.setupWizard }));
      setSetupModalOpen(false);
      setToast({ message: action === "completed" ? "Setup finished." : "Setup skipped. You can reopen it from Settings." });
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to update setup status.", kind: "error" });
    } finally {
      setBusy(null);
    }
  }

  function closeSetupWizard() {
    if (settings.setupWizard.completedAt || settings.setupWizard.dismissedAt) {
      setSetupModalOpen(false);
      return;
    }
    if (busy?.startsWith("setup-")) {
      setSetupModalOpen(false);
      return;
    }
    setSetupModalOpen(false);
    void saveSetupWizard("dismissed");
  }

  async function refreshCurrentView() {
    setBusy("refresh-view");
    setToast({ message: `Refreshing ${view}...`, kind: "loading" });
    try {
      if (view === "frp") setFrpRefreshKey((current) => current + 1);
      await loadViewWithState(view);
      setToast({ message: "View refreshed." });
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to refresh view.", kind: "error" });
    } finally {
      setBusy(null);
    }
  }

  async function refreshSystemLogs() {
    setBusy("system-logs");
    setToast({ message: "Refreshing system log...", kind: "loading" });
    try {
      setSystemLogs(await api.systemLogs());
      setToast({ message: "System log refreshed." });
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to refresh system log.", kind: "error" });
    } finally {
      setBusy(null);
    }
  }

  if (loading) {
    return (
      <main className="login-shell">
        <YantoBootLoader />
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

  const rollbackTarget = rollbackModal?.targetRef.trim() ?? "";
  const rollbackPreview = rollbackModal?.preview?.requestedRef === rollbackTarget ? rollbackModal.preview : null;
  const rollbackBusy = rollbackModal ? busy === `rollback:${rollbackModal.project.id}` : false;

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
            ["hostnames", Globe2, "Hostnames"],
            ["frp", Network, "FRP"],
            ...(multiNodeEnabled ? [["nodes", Server, "Nodes"] as const] : []),
            ["backups", Archive, "Backups"],
            ["dns", Globe2, "DNS"],
            ["audit", FileClock, "Audit"],
            ["settings", Settings, "Settings"]
          ].map(([id, Icon, label]) => (
            <button key={id as string} className={view === id ? "active" : ""} type="button" onClick={() => setView(id as View)}>
              <Icon size={17} />
              <span>{label as string}</span>
            </button>
          ))}
        </nav>
        <div className="sidebar-footer">
          <div className="app-version" title={`Yanto version ${appVersion}`}>v{appVersion}</div>
          <button className="theme-toggle" type="button" role="switch" aria-checked={theme === "dark"} aria-label="Toggle dark mode" title="Toggle dark mode" onClick={() => setTheme((current) => (current === "dark" ? "light" : "dark"))}>
            {theme === "dark" ? <Moon size={17} /> : <Sun size={17} />}
            <span className="theme-toggle-text">Dark mode</span>
            <span className={`toggle-switch theme-toggle-switch ${theme === "dark" ? "on" : ""}`} aria-hidden="true">
              <span />
            </span>
          </button>
          <button
            className="logout"
            type="button"
            onClick={async () => {
              setToast({ message: "Signing out...", kind: "loading" });
              try {
                await api.logout();
                setUser(null);
                setToast(null);
              } catch (error) {
                setToast({ message: error instanceof Error ? error.message : "Unable to sign out.", kind: "error" });
              }
            }}
          >
            <LogOut size={17} />
            <span>Sign out</span>
          </button>
        </div>
      </aside>

      <main className="main">
        <header className="topbar">
          <div>
            <h1>{viewTitle(view)}</h1>
          </div>
          <Button variant="secondary" disabled={busy === "refresh-view"} onClick={() => void refreshCurrentView()} icon={<RefreshCw size={16} className={busy === "refresh-view" ? "spin" : ""} />}>
            {busy === "refresh-view" ? "Refreshing" : "Refresh"}
          </Button>
        </header>

        {view === "dashboard" ? (
          <DashboardView
            projects={projects}
            nodes={nodes}
            containers={containers}
            deployments={deployments}
            runningDeployments={runningDeployments}
            usage={usage}
            settings={settings}
            setupCanReopen={setupCanReopen}
            failingProjects={failingProjects}
            unhealthyContainers={unhealthyContainers}
            warningDisks={warningDisks}
            openSetupWizard={openSetupWizard}
            openDeploymentLogs={openDeploymentLogs}
          />
        ) : null}

        <Suspense fallback={<LoadingInline label="Loading..." />}>
        {view === "projects" ? (
          <ProjectsView
            visibleProjects={visibleProjects}
            projects={projects}
            containersByProjectFolder={containersByProjectFolder}
            cfRoutesByProject={cfRoutesByProject}
            routeDiagnosticsByRouteId={routeDiagnosticsByRouteId}
            latestDeploymentByProject={latestDeploymentByProject}
            settings={settings}
            busy={busy}
            loading={viewLoading.projects}
            projectPage={projectPage}
            openProject={openProject}
            openRollback={openRollback}
            deploy={deploy}
            copyText={copyText}
            copyDeployToken={copyDeployToken}
            setConfirm={setConfirm}
            refreshProjects={refreshProjects}
            setProjectPage={setProjectPage}
          />
        ) : null}

        {view === "deployments" ? (
          <DeploymentsView
            deployments={deployments}
            visibleDeployments={visibleDeployments}
            deploymentPage={deploymentPage}
            busy={busy}
            loading={viewLoading.deployments}
            openDeploymentLogs={openDeploymentLogs}
            retryDeployment={retryDeployment}
            setDeploymentPage={setDeploymentPage}
          />
        ) : null}

        {view === "backups" ? (
          <BackupsView
            postgresTargets={postgresTargets}
            visibleBackups={visibleBackups}
            backups={backups}
            busy={busy}
            loading={viewLoading.backups}
            r2Ready={r2Ready}
            backupPage={backupPage}
            dumpPostgresTarget={dumpPostgresTarget}
            restorePostgresTarget={restorePostgresTarget}
            uploadBackupR2={uploadBackupR2}
            setConfirm={setConfirm}
            refreshBackups={refreshBackups}
            setBackupPage={setBackupPage}
          />
        ) : null}

        {view === "dns" ? (
          <DnsView
            clients={cloudflareClients}
            selectedClientId={dnsClientId}
            records={dnsRecords}
            visibleRecords={visibleDnsRecords}
            diagnostics={routeDiagnostics}
            busy={busy}
            loading={viewLoading.dns}
            loaded={dnsLoaded}
            page={dnsPage}
            selectClient={(clientId) => void selectDnsClient(clientId)}
            createRecord={createDnsRecord}
            updateRecord={updateDnsRecord}
            deleteRecord={deleteDnsRecord}
            copyText={copyText}
            setConfirm={setConfirm}
            setPage={setDnsPage}
            openClients={() => setView("hostnames")}
          />
        ) : null}

        {view === "hostnames" ? <HostnamesView projects={projects} containers={containers} toast={(message, kind) => setToast({ message, kind })} /> : null}

        {view === "frp" ? (
          <FrpView
            refreshKey={frpRefreshKey}
            copyText={copyText}
            toast={(message, kind) => setToast({ message, kind })}
            setConfirm={setConfirm}
          />
        ) : null}

        {view === "audit" ? (
          <AuditView
            auditEntries={auditEntries}
            visibleAuditEntries={visibleAuditEntries}
            auditPage={auditPage}
            setAuditPage={setAuditPage}
          />
        ) : null}

        {view === "containers" ? (
          <ContainersView
            containers={containers}
            loading={viewLoading.containers}
            openContainerLogs={openContainerLogs}
            setConfirm={setConfirm}
            refreshContainers={refreshContainers}
          />
        ) : null}

        {view === "nodes" && multiNodeEnabled ? (
          <NodesView nodes={nodes} />
        ) : null}

        {view === "settings" ? (
          <SettingsView
            settings={settings}
            r2Form={r2Form}
            cfForm={cfForm}
            busy={busy}
            sshPrivateKey={sshPrivateKey}
            systemLogs={systemLogs}
            cleanupLogs={cleanupLogs}
            cleanupLogTitle={cleanupLogTitle}
            cleanupPreviewed={cleanupPreviewed}
            updateR2Form={updateR2Form}
            updateCfForm={updateCfForm}
            saveR2Settings={saveR2Settings}
            saveCfSettings={saveCfSettings}
            saveMultiNodeSettings={saveMultiNodeSettings}
            validateCfSettings={validateCfSettings}
            saveSshPrivateKey={saveSshPrivateKey}
            generateSshPrivateKey={generateSshPrivateKey}
            setSshPrivateKey={setSshPrivateKey}
            copyText={copyText}
            copyWorkerInstallCommand={copyWorkerInstallCommand}
            openSetupWizard={openSetupWizard}
            previewCleanup={previewCleanup}
            refreshSystemLogs={refreshSystemLogs}
            refreshContainers={refreshContainers}
            setConfirm={setConfirm}
            setBusy={setBusy}
            setCleanupLogTitle={setCleanupLogTitle}
            setCleanupLogs={setCleanupLogs}
            setCleanupPreviewed={setCleanupPreviewed}
          />
        ) : null}
        </Suspense>
      </main>

      {setupModalOpen ? (
        <Modal title="Quick setup" onClose={closeSetupWizard}>
          <div className="setup-wizard">
            <div className="setup-progress" aria-label="Setup progress">
              {setupSteps.map((step, index) => (
                <button key={step} type="button" className={setupStep === step ? "active" : index < setupStepIndex ? "done" : ""} onClick={() => setSetupStep(step)} aria-label={`Go to ${step}`}>
                  <span>{index + 1}</span>
                </button>
              ))}
            </div>

            {setupStep === "intro" ? (
              <div className="setup-step">
                <div className="setup-intro">
                  <strong>Bring the basics in now, or skip and keep moving.</strong>
                  <p className="muted">These settings are optional. Yanto will remember if you skip this modal and keep the setup entry available in Settings.</p>
                </div>
                <div className="setup-checklist">
                  <div>
                    <KeyRound size={16} />
                    <span>Git SSH key</span>
                    <StatusBadge status={sshReady ? "ready" : "optional"} />
                  </div>
                  <div>
                    <ShieldCheck size={16} />
                    <span>Cloudflare Tunnel</span>
                    <StatusBadge status={cfSettingsReady ? "ready" : "optional"} />
                  </div>
                  <div>
                    <Cloud size={16} />
                    <span>Cloudflare R2</span>
                    <StatusBadge status={r2Ready ? "ready" : "optional"} />
                  </div>
                </div>
              </div>
            ) : null}

            {setupStep === "ssh" ? (
              <form className="setup-step form-grid ssh-key-form" onSubmit={saveSshPrivateKey}>
                <div className="setup-status-row">
                  <span>Managed key</span>
                  <StatusBadge status={settings.sshKey?.hasManagedKey ? "ready" : "optional"} />
                </div>
                {settings.sshKey?.publicKey ? (
                  <div className="token-box ssh-public-key-box">
                    <span>{settings.sshKey.publicKey}</span>
                    <button type="button" onClick={() => void copyText(settings.sshKey?.publicKey ?? "")} title="Copy public key" aria-label="Copy public key">
                      <Copy size={15} />
                    </button>
                  </div>
                ) : null}
                <TextAreaField
                  label="Private key"
                  value={sshPrivateKey}
                  onChange={setSshPrivateKey}
                  placeholder={"Paste the full private key, starting with -----BEGIN OPENSSH PRIVATE KEY-----"}
                />
                <div className="actions">
                  <Button type="button" variant="secondary" disabled={busy === "ssh-key-generate" || settings.sshKey?.hasManagedKey} onClick={() => void generateSshPrivateKey()} icon={<GitPullRequest size={16} />}>
                    Generate key
                  </Button>
                  <Button type="submit" disabled={busy === "ssh-key" || !sshPrivateKey.trim()} icon={<KeyRound size={16} />}>
                    Save SSH key
                  </Button>
                </div>
              </form>
            ) : null}

            {setupStep === "cloudflare" ? (
              <form className="setup-step form-grid compact-form" onSubmit={saveCfSettings} autoComplete="off">
                <div className="setup-status-row">
                  <span>Tunnel settings</span>
                  <StatusBadge status={cfSettingsReady ? "ready" : "optional"} />
                </div>
                <div className="cf-token-requirements">
                  <span>Token permissions</span>
                  <ul>
                    <li>Account / Cloudflare Tunnel / Edit</li>
                    <li>Account / Account Settings / Read</li>
                    <li>Zone / Zone / Read</li>
                    <li>Zone / DNS / Edit</li>
                  </ul>
                </div>
                <div className="settings-form-pair">
                  <TextField label="Account ID" value={cfForm.accountId} onChange={(accountId) => updateCfForm({ accountId })} />
                  <TextField label="Zone ID" value={cfForm.zoneId} onChange={(zoneId) => updateCfForm({ zoneId })} />
                </div>
                <TextField
                  label="API Token"
                  type="password"
                  value={cfForm.apiToken}
                  onChange={(apiToken) => updateCfForm({ apiToken })}
                  placeholder={settings.cf?.hasApiToken ? "Saved; leave blank to keep" : ""}
                  autoComplete="new-password"
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
            ) : null}

            {setupStep === "r2" ? (
              <form className="setup-step form-grid compact-form" onSubmit={saveR2Settings} autoComplete="off">
                <div className="setup-status-row">
                  <span>R2 uploads</span>
                  <StatusBadge status={r2Ready ? "ready" : "optional"} />
                </div>
                <ToggleField
                  label="Upload enabled"
                  value={r2Form.enabled}
                  onChange={(enabled) => updateR2Form({ enabled })}
                  description={settings.r2?.hasSecretAccessKey ? "Secret key saved" : "Add an R2 secret key before uploading"}
                />
                <div className="settings-form-pair">
                  <TextField label="Account ID" value={r2Form.accountId} onChange={(accountId) => updateR2Form({ accountId })} autoComplete="off" />
                  <TextField label="Bucket" value={r2Form.bucket} onChange={(bucket) => updateR2Form({ bucket })} autoComplete="off" />
                </div>
                <div className="settings-form-pair">
                  <TextField
                    label="Access key ID"
                    value={r2Form.accessKeyId}
                    onChange={(accessKeyId) => updateR2Form({ accessKeyId })}
                    placeholder={settings.r2?.maskedAccessKeyId ? `${settings.r2.maskedAccessKeyId}; leave blank to keep` : ""}
                    autoComplete="off"
                  />
                  <TextField
                    label="Secret access key"
                    type="password"
                    value={r2Form.secretAccessKey}
                    onChange={(secretAccessKey) => updateR2Form({ secretAccessKey })}
                    placeholder={settings.r2?.hasSecretAccessKey ? "Saved; leave blank to keep" : ""}
                    autoComplete="new-password"
                  />
                </div>
                <TextField label="Object prefix" value={r2Form.prefix} onChange={(prefix) => updateR2Form({ prefix })} autoComplete="off" />
                <div className="actions">
                  <Button type="submit" disabled={busy === "r2-settings"} icon={<Cloud size={16} />}>
                    Save R2
                  </Button>
                </div>
              </form>
            ) : null}

            <div className="setup-actions">
              <Button variant="ghost" disabled={!!busy?.startsWith("setup-")} onClick={() => void saveSetupWizard("dismissed")}>
                Skip
              </Button>
              <div className="actions">
                <Button variant="secondary" disabled={!setupCanGoBack} onClick={goToPreviousSetupStep} icon={<ChevronLeft size={16} />}>
                  Back
                </Button>
                {setupCanGoNext ? (
                  <Button onClick={goToNextSetupStep} icon={<ChevronRight size={16} />}>
                    Next
                  </Button>
                ) : (
                  <Button disabled={!!busy?.startsWith("setup-")} onClick={() => void saveSetupWizard("completed")} icon={<ShieldCheck size={16} />}>
                    Finish
                  </Button>
                )}
              </div>
            </div>
          </div>
        </Modal>
      ) : null}

      {projectModal ? (
        <Modal title={projectModal === "new" ? "Add project" : "Edit project"} size="wide" closeOnEscape={!projectEditorModal} onClose={() => {
          setProjectModal(null);
          setProjectEditorModal(null);
        }}>
          <form className="project-edit-form" onSubmit={saveProject}>
            <div className="project-edit-layout">
              <section className="project-edit-section">
                <div className="section-kicker">Project</div>
                <TextField label="Name" value={projectForm.name} onChange={(name) => setProjectForm((current) => ({ ...current, name }))} required />
                <TextField label="Git SSH URL" value={projectForm.gitUrl} onChange={updateProjectGitUrl} placeholder="Optional: git@github.com:user/repo.git" />
                <div className="project-edit-pair">
                  <TextField label="Branch" value={projectForm.branch} onChange={(branch) => setProjectForm((current) => ({ ...current, branch }))} required />
                  <TextField label="Compose file" value={projectForm.composeFile} onChange={(composeFile) => setProjectForm((current) => ({ ...current, composeFile }))} placeholder="docker-compose.yml" required />
                </div>
                <TextField label="Folder name" value={projectForm.folderName} onChange={(folderName) => setProjectForm((current) => ({ ...current, folderName }))} placeholder={slugifyFolderName(projectForm.name) || "Auto from project name"} />
                {multiNodeEnabled ? (
                  <CustomSelect label="Deployment node" value={projectForm.targetNodeId} options={nodeOptions} onChange={(targetNodeId) => setProjectForm((current) => ({ ...current, targetNodeId }))} />
                ) : null}
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

              <div className="project-edit-side">
                <section className="project-edit-section project-edit-tools">
                  <div className="section-kicker">Editors</div>
                  <button className="editor-launch-row" type="button" onClick={() => void openComposeEditor()} disabled={projectCompose.loading}>
                    <FileText size={16} />
                    <span>
                      <strong>Compose</strong>
                      <small>{projectCompose.loading ? "Opening..." : "Open editor"}</small>
                    </span>
                    <span className="editor-launch-action" aria-hidden="true">
                      {projectCompose.loading ? <RefreshCw size={15} className="spin" /> : <FileText size={15} />}
                    </span>
                  </button>
                  <button className="editor-launch-row" type="button" onClick={() => void openEnvEditor()} disabled={projectEnv.loading}>
                    <List size={16} />
                    <span>
                      <strong>Environment</strong>
                      <small>{projectEnv.loading ? "Opening..." : "Open editor"}</small>
                    </span>
                    <span className="editor-launch-action" aria-hidden="true">
                      {projectEnv.loading ? <RefreshCw size={15} className="spin" /> : <List size={15} />}
                    </span>
                  </button>
                </section>

                {projectModal !== "new" ? (
                  <section className="project-edit-section cf-routes-section">
                  <div className="cf-route-head">
                    <div>
                      <div className="section-kicker">Cloudflare Tunnel</div>
                      <p className="muted">Manage multiple public hostnames and isolated tunnel networks from the Hostnames screen.</p>
                    </div>
                    <StatusBadge status={cfSettingsReady ? (cfRoutes.some((route) => route.enabled) ? "connected" : "idle") : "disabled"} />
                  </div>
                  <div className="cf-route-list compact">
                    {cfRoutes.map((route) => (
                      <div key={route.id} className="cf-route-row">
                        <div className="cf-route-info">
                          <a className="cf-route-hostname" href={`https://${route.hostname}`} target="_blank" rel="noopener noreferrer">
                            https://{route.hostname}
                          </a>
                          <span className="cf-route-service">{route.serviceTarget}{route.noTlsVerify ? " · no TLS verify" : ""}</span>
                          <StatusBadge status={route.enabled ? "enabled" : "disabled"} />
                          {routeDiagnosticsByRouteId[route.id] ? (
                            <>
                              <StatusBadge status={routeDiagnosticsByRouteId[route.id].dnsStatus} label={`DNS ${routeDiagnosticsByRouteId[route.id].dnsStatus}`} />
                              <StatusBadge status={routeDiagnosticsByRouteId[route.id].tunnelStatus} label={`Tunnel ${routeDiagnosticsByRouteId[route.id].tunnelStatus}`} />
                              <StatusBadge status={routeDiagnosticsByRouteId[route.id].reachabilityStatus} label={`HTTPS ${routeDiagnosticsByRouteId[route.id].reachabilityStatus}`} />
                            </>
                          ) : null}
                        </div>
                        <div className="cf-route-actions">
                          <IconButton label="Copy URL" onClick={() => void copyText(`https://${route.hostname}`)}><Copy size={14} /></IconButton>
                          <Button variant="ghost" disabled={!cfSettingsReady || busy === `cf-route-toggle:${route.id}`} onClick={() => void toggleCfRoute(route)}>
                            {route.enabled ? "Disable" : "Enable"}
                          </Button>
                          <IconButton label="Delete route" disabled={!cfSettingsReady || busy === `cf-route-delete:${route.id}`} onClick={() => void removeCfRoute(route.id)}><Trash2 size={14} /></IconButton>
                        </div>
                        {routeDiagnosticsByRouteId[route.id]?.messages.length ? (
                          <p className="cf-route-diagnostic-message">{routeDiagnosticsByRouteId[route.id].messages[0]}</p>
                        ) : null}
                      </div>
                    ))}
                    {!cfRoutes.length ? <p className="muted">No public hostnames configured.</p> : null}
                  </div>
                  {!cfRouteEditorOpen ? (
                    <Button
                      type="button"
                      variant="secondary"
                      onClick={() => { setProjectModal(null); setView("hostnames"); }}
                      icon={<Plus size={15} />}
                    >
                      Manage hostnames
                    </Button>
                  ) : null}
                  {cfRouteEditorOpen ? (
                  <div className="cf-route-add-form compact">
                    <div className="cf-route-add-row">
                      <TextField label="Hostname" value={cfRouteForm.hostname} onChange={(hostname) => setCfRouteForm((current) => ({ ...current, hostname }))} placeholder="app.example.com" disabled={!cfSettingsReady} />
                      <TextField
                        label="Local service target"
                        value={cfRouteForm.localTarget}
                        onChange={(localTarget) => {
                          const parsed = parseCfServiceTarget(localTarget);
                          setCfRouteForm((current) => ({
                            ...current,
                            protocol: parsed.protocol,
                            localTarget: parsed.localTarget,
                            noTlsVerify: parsed.protocol === "https" ? current.noTlsVerify : false
                          }));
                        }}
                        placeholder="container-name:3000"
                        disabled={!cfSettingsReady}
                      />
                      <CustomSelect<CfRouteProtocol>
                        label="Protocol"
                        value={cfRouteForm.protocol}
                        options={[
                          { label: "HTTP", value: "http" },
                          { label: "HTTPS", value: "https" }
                        ]}
                        onChange={(protocol) => setCfRouteForm((current) => ({ ...current, protocol, noTlsVerify: protocol === "https" ? current.noTlsVerify : false }))}
                        disabled={!cfSettingsReady}
                      />
                      {cfRouteForm.protocol === "https" ? (
                        <ToggleField label="No TLS verify" value={cfRouteForm.noTlsVerify} onChange={(noTlsVerify) => setCfRouteForm((current) => ({ ...current, noTlsVerify }))} disabled={!cfSettingsReady} />
                      ) : null}
                      <Button disabled={!cfSettingsReady || !cfRouteForm.hostname || !cfRouteForm.localTarget} loading={busy === "cf-route-publish"} variant="secondary" onClick={() => void publishCfRoute(projectModal.id)} icon={<Plus size={15} />}>
                        {busy === "cf-route-publish" ? "Saving" : cfRoutes.length ? "Save hostname" : "Publish"}
                      </Button>
                      <Button type="button" variant="ghost" onClick={() => setCfRouteEditorOpen(false)}>
                        Cancel
                      </Button>
                    </div>
                  </div>
                  ) : null}
                  </section>
                ) : null}
              </div>
            </div>
            <div className="actions project-edit-actions">
              <Button type="button" variant="secondary" disabled={projectEnv.loading || !projectForm.manualDeployEnabled} loading={busy === "project"} onClick={() => void persistProjectDetails(undefined, "deploy")} icon={<Play size={15} />}>
                {busy === "project" ? "Saving" : "Save & Deploy"}
              </Button>
              <Button type="submit" disabled={projectEnv.loading} loading={busy === "project"}>
                {busy === "project" ? "Saving" : "Save project"}
              </Button>
            </div>
          </form>
        </Modal>
      ) : null}

      {projectModal && projectEditorModal === "compose" ? (
        <Modal title="Compose editor" size="wide" onClose={() => setProjectEditorModal(null)}>
          <div className="editor-modal-body compose-section">
            <div className="editor-status-line">
              <StatusBadge status={projectCompose.source ?? "open"} />
              <span>{projectCompose.message || "Compose override"}</span>
            </div>
            <TextAreaField
              label="Compose content"
              value={projectForm.composeContent}
              onChange={(composeContent) => setProjectForm((current) => ({ ...current, composeContent }))}
              placeholder={"Optional. Paste docker-compose.yml content here for compose-only projects or to override the file during deploy."}
            />
          </div>
        </Modal>
      ) : null}

      {projectModal && projectEditorModal === "env" ? (
        <Modal title="Environment editor" size="wide" onClose={() => setProjectEditorModal(null)}>
          <div className="editor-modal-body env-section">
            {projectEnv.loading ? (
              <LoadingInline label="Loading environment" />
            ) : projectEnv.available ? (
              <EnvEditor modal={projectEnv} onChange={setProjectEnv} />
            ) : (
              <p className="muted">Environment could not be loaded. Project fields can still be saved.</p>
            )}
          </div>
        </Modal>
      ) : null}

      {rollbackModal ? (
        <Modal title={`Rollback ${rollbackModal.project.name}`} size="wide" onClose={() => setRollbackModal(null)}>
          <div className="rollback-panel">
            <form
              className="rollback-target-row"
              onSubmit={(event) => {
                event.preventDefault();
                void previewRollback();
              }}
            >
              <TextField
                label="Commit or tag"
                value={rollbackModal.targetRef}
                onChange={updateRollbackTarget}
                placeholder="v1.2.3 or 9f3a1c2"
                disabled={rollbackModal.previewLoading || rollbackBusy}
              />
              <Button type="submit" variant="secondary" disabled={!rollbackTarget || rollbackModal.previewLoading || rollbackBusy} loading={rollbackModal.previewLoading} icon={<GitBranch size={15} />}>
                Preview diff
              </Button>
            </form>

            {rollbackModal.previewError ? <p className="error-text">{rollbackModal.previewError}</p> : null}
            {rollbackModal.previewLoading ? <LoadingInline label="Loading rollback diff" /> : null}

            {rollbackPreview ? (
              <div className="rollback-preview">
                <div className="rollback-ref-grid">
                  <div>
                    <span>Current</span>
                    <strong>{shortSha(rollbackPreview.current.sha)}</strong>
                    <p>{rollbackPreview.current.message || "No commit message"}</p>
                  </div>
                  <div>
                    <span>Target</span>
                    <strong>{shortSha(rollbackPreview.target.sha)}</strong>
                    <p>{rollbackPreview.target.message || rollbackPreview.requestedRef}</p>
                  </div>
                </div>
                <div className="rollback-stat-grid">
                  <div>
                    <span>Leaving behind</span>
                    <strong>{rollbackPreview.commitsToLeaveBehind}</strong>
                  </div>
                  <div>
                    <span>Applying</span>
                    <strong>{rollbackPreview.commitsToApply}</strong>
                  </div>
                  <div>
                    <span>Files changed</span>
                    <strong>{rollbackPreview.filesChanged}</strong>
                  </div>
                  <div>
                    <span>Line diff</span>
                    <strong>+{rollbackPreview.additions} / -{rollbackPreview.deletions}</strong>
                  </div>
                </div>
                <div className="rollback-files">
                  {rollbackPreview.files.length ? (
                    rollbackPreview.files.map((file) => (
                      <div className="rollback-file-row" key={file.path}>
                        <span>{file.path}</span>
                        <strong>{file.binary ? "binary" : `+${file.additions ?? 0} / -${file.deletions ?? 0}`}</strong>
                      </div>
                    ))
                  ) : (
                    <p className="muted">No file changes between the current checkout and this target.</p>
                  )}
                  {rollbackPreview.filesChanged > rollbackPreview.files.length ? <p className="muted">Showing first {rollbackPreview.files.length} files.</p> : null}
                </div>
              </div>
            ) : null}

            <div className="actions">
              <Button type="button" variant="ghost" onClick={() => setRollbackModal(null)}>
                Cancel
              </Button>
              <Button type="button" disabled={!rollbackPreview || rollbackBusy} loading={rollbackBusy} onClick={() => void startRollback()} icon={<GitBranch size={15} />}>
                Rollback to ref
              </Button>
            </div>
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

      {createdProjectSecret ? (
        <Modal title={`${createdProjectSecret.projectName} deploy token`} onClose={() => setCreatedProjectSecret(null)}>
          <div className="form-grid compact-form">
            <label className="field">
              <span>Deploy URL</span>
              <input value={createdProjectSecret.deployUrl} readOnly onFocus={(event) => event.currentTarget.select()} />
            </label>
            <label className="field">
              <span>GitHub webhook URL</span>
              <input value={createdProjectSecret.webhookUrl} readOnly onFocus={(event) => event.currentTarget.select()} />
            </label>
            <label className="field">
              <span>Deploy token</span>
              <input value={createdProjectSecret.deployToken} readOnly onFocus={(event) => event.currentTarget.select()} />
            </label>
            <div className="actions">
              <Button variant="secondary" onClick={() => void copyText(createdProjectSecret.deployUrl)} icon={<Copy size={15} />}>
                Deploy URL
              </Button>
              <Button variant="secondary" onClick={() => void copyText(createdProjectSecret.webhookUrl)} icon={<Copy size={15} />}>
                Webhook URL
              </Button>
              <Button onClick={() => void copyText(createdProjectSecret.deployToken)} icon={<Copy size={15} />}>
                Token
              </Button>
            </div>
          </div>
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
            const loadingMessage = confirm.loadingMessage ?? `${confirm.label} in progress...`;
            const successMessage = confirm.successMessage ?? "Action completed.";
            setConfirm(null);
            setToast({ message: loadingMessage, kind: "loading" });
            action()
              .then(() => {
                setToast({ message: successMessage });
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
