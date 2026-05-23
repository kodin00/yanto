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
  Download,
  FileClock,
  GitBranch,
  HardDrive,
  KeyRound,
  LogOut,
  MemoryStick,
  Play,
  Plus,
  RefreshCw,
  RotateCw,
  ScrollText,
  Server,
  Settings,
  ShieldCheck,
  Square,
  Undo2,
  Trash2
} from "lucide-react";
import { FormEvent, type ReactNode, useCallback, useEffect, useMemo, useState } from "react";
import type { ContainerInfo, Deployment, Project, SystemUsage } from "../shared/types";
import { Button, ConfirmDialog, IconButton, LoadingInline, LogViewer, Modal, StatusBadge, TextAreaField, TextField, Toast, ToggleField } from "./components/ui";
import { api, type AuditLogEntry, type BackupRecord, type ProjectEnvVariable } from "./lib/api";

type View = "dashboard" | "projects" | "deployments" | "containers" | "backups" | "audit" | "settings";
type ToastState = { message: string; kind?: "ok" | "error" } | null;
type LogModalState = { title: string; logs: string; streamPath?: string; live?: boolean; status?: string };
type LogStreamPayload = { logs?: string; chunk?: string; status?: string; error?: string; done?: boolean };
type EnvModalState = { project: Project; rows: ProjectEnvVariable[]; baseline: ProjectEnvVariable[]; draftKey: string; draftValue: string; loading: boolean };
type RollbackModalState = { project: Project; deployments: Deployment[] };

const emptyProject = {
  name: "",
  gitUrl: "",
  branch: "master",
  folderName: "",
  composeFile: "docker-compose.yml",
  composeContent: "",
  autoStart: false
};

const emptySshKeySettings = {
  hasManagedKey: false,
  hasMountedKey: false,
  managedPrivateKeyPath: "/data/ssh/id_ed25519",
  mountedPrivateKeyPath: "/root/.ssh/id_ed25519",
  activePrivateKeyPath: null as string | null,
  publicKey: null as string | null
};

const pageSize = 10;

function bytes(value: number) {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let current = value;
  let unit = 0;
  while (current >= 1024 && unit < units.length - 1) {
    current /= 1024;
    unit += 1;
  }
  return `${current.toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`;
}

function dateTime(value: string | null) {
  if (!value) return "-";
  return new Date(value).toLocaleString();
}

function durationSince(value: string | null) {
  if (!value) return "-";
  const started = new Date(value).getTime();
  if (Number.isNaN(started)) return "-";
  const seconds = Math.max(0, Math.floor((Date.now() - started) / 1000));
  const units = [
    ["d", 86400],
    ["h", 3600],
    ["m", 60]
  ] as const;
  const parts: string[] = [];
  let remaining = seconds;
  for (const [label, unitSeconds] of units) {
    const amount = Math.floor(remaining / unitSeconds);
    if (amount) {
      parts.push(`${amount}${label}`);
      remaining %= unitSeconds;
    }
    if (parts.length === 2) break;
  }
  if (!parts.length) return `${seconds}s`;
  return parts.join(" ");
}

function durationBetween(startedAt: string | null, finishedAt: string | null) {
  if (!startedAt) return "-";
  const start = new Date(startedAt).getTime();
  const end = finishedAt ? new Date(finishedAt).getTime() : Date.now();
  if (Number.isNaN(start) || Number.isNaN(end)) return "-";
  const seconds = Math.max(0, Math.floor((end - start) / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  const remainingSeconds = seconds % 60;
  if (minutes < 60) return remainingSeconds ? `${minutes}m ${remainingSeconds}s` : `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  const remainingMinutes = minutes % 60;
  return remainingMinutes ? `${hours}h ${remainingMinutes}m` : `${hours}h`;
}

function endpoint(project: Project, baseUrl: string) {
  return `${baseUrl.replace(/\/$/, "")}/deploy?id=${project.id}`;
}

function slugifyFolderName(input: string) {
  return input
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 64);
}

function pageItems<T>(items: T[], page: number) {
  return items.slice((page - 1) * pageSize, page * pageSize);
}

function totalPages(items: unknown[]) {
  return Math.max(1, Math.ceil(items.length / pageSize));
}

function usedMemoryMb(memoryUsage: string) {
  const used = memoryUsage.split("/")[0]?.trim();
  if (!used || used === "-") return "-";
  const match = used.match(/^([\d.]+)\s*([KMGT]?i?B)$/i);
  if (!match) return used;
  const value = Number(match[1] ?? 0);
  const unit = (match[2] ?? "").toLowerCase();
  const multipliers: Record<string, number> = {
    b: 1 / 1024 / 1024,
    kb: 1 / 1024,
    kib: 1 / 1024,
    mb: 1,
    mib: 1,
    gb: 1024,
    gib: 1024,
    tb: 1024 * 1024,
    tib: 1024 * 1024
  };
  const mb = value * (multipliers[unit] ?? 1);
  return `${mb >= 10 ? mb.toFixed(0) : mb.toFixed(1)} MB`;
}

function isProtectedYantoContainer(container: ContainerInfo) {
  return /^yanto-(app|postgres)-\d+$/.test(container.name);
}

function normalizeEnvRows(rows: ProjectEnvVariable[]) {
  return rows.map((row) => ({ key: row.key, value: row.value ?? "", masked: Boolean(row.masked) })).sort((a, b) => a.key.localeCompare(b.key));
}

function deploymentChanges(deployment: Deployment) {
  const extra = deployment as Deployment & { changes?: string | string[] | null; commitSha?: string | null };
  if (Array.isArray(extra.changes)) return extra.changes.join(", ");
  if (extra.changes) return extra.changes;
  if (extra.commitSha) return extra.commitSha.slice(0, 12);
  return deployment.exitCode === null ? "Running" : `Exit ${deployment.exitCode}`;
}

export function App() {
  const [user, setUser] = useState<string | null>(null);
  const [login, setLogin] = useState({ username: "admin", password: "" });
  const [view, setView] = useState<View>("dashboard");
  const [projects, setProjects] = useState<Project[]>([]);
  const [deployments, setDeployments] = useState<Deployment[]>([]);
  const [containers, setContainers] = useState<ContainerInfo[]>([]);
  const [backups, setBackups] = useState<BackupRecord[]>([]);
  const [auditEntries, setAuditEntries] = useState<AuditLogEntry[]>([]);
  const [usage, setUsage] = useState<SystemUsage | null>(null);
  const [settings, setSettings] = useState({ projectsRoot: "/projects", hostProjectsRoot: "~/projects", sshKeysDir: "", appBaseUrl: "", sshKey: emptySshKeySettings });
  const [systemLogs, setSystemLogs] = useState("");
  const [cleanupPreviewed, setCleanupPreviewed] = useState(false);
  const [loading, setLoading] = useState(true);
  const [toast, setToast] = useState<ToastState>(null);
  const [projectModal, setProjectModal] = useState<Project | "new" | null>(null);
  const [projectForm, setProjectForm] = useState(emptyProject);
  const [envModal, setEnvModal] = useState<EnvModalState | null>(null);
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
    const [projectRows, deploymentRows, containerRows, backupRows, auditRows, systemRows, settingRows, logRows] = await Promise.all([
      api.projects(),
      api.deployments(),
      api.containers().catch(() => []),
      api.backups().catch(() => []),
      api.auditLog().catch(() => []),
      api.systemUsage().catch(() => null),
      api.settings(),
      api.systemLogs().catch(() => "")
    ]);
    setProjects(projectRows);
    setDeployments(deploymentRows);
    setContainers(containerRows);
    setBackups(backupRows);
    setAuditEntries(auditRows);
    setUsage(systemRows);
    setSettings(settingRows);
    setSystemLogs(logRows);
  }, []);

  useEffect(() => {
    api
      .me()
      .then((result) => {
        setUser(result.username);
        return loadAll();
      })
      .catch(() => setUser(null))
      .finally(() => setLoading(false));
  }, [loadAll]);

  useEffect(() => {
    if (!user) return;
    const timer = window.setInterval(() => {
      void loadAll().catch(() => undefined);
    }, 8000);
    return () => window.clearInterval(timer);
  }, [loadAll, user]);

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
  const visibleProjects = useMemo(() => pageItems(projects, projectPage), [projectPage, projects]);
  const visibleDeployments = useMemo(() => pageItems(deployments, deploymentPage), [deploymentPage, deployments]);
  const visibleBackups = useMemo(() => pageItems(backups, backupPage), [backupPage, backups]);
  const visibleAuditEntries = useMemo(() => pageItems(auditEntries, auditPage), [auditEntries, auditPage]);

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
      await loadAll();
      setToast({ message: "Signed in." });
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to sign in.", kind: "error" });
    } finally {
      setBusy(null);
    }
  }

  async function saveProject(event: FormEvent) {
    event.preventDefault();
    setBusy("project");
    try {
      if (projectModal === "new") {
        await api.createProject(projectForm);
        setToast({ message: "Project registered." });
      } else if (projectModal) {
        await api.updateProject(projectModal.id, projectForm);
        setToast({ message: "Project updated." });
      }
      setProjectModal(null);
      await loadAll();
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to save project.", kind: "error" });
    } finally {
      setBusy(null);
    }
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
        autoStart: project.autoStart
      });
      setProjectModal(project);
      return;
    }
    setProjectForm(emptyProject);
    setProjectModal("new");
  }

  async function openEnvEditor(project: Project) {
    setEnvModal({ project, rows: [], baseline: [], draftKey: "", draftValue: "", loading: true });
    try {
      const rows = normalizeEnvRows(await api.projectEnv(project.id));
      setEnvModal({ project, rows, baseline: rows, draftKey: "", draftValue: "", loading: false });
    } catch (error) {
      setEnvModal(null);
      setToast({ message: error instanceof Error ? error.message : "Unable to load environment.", kind: "error" });
    }
  }

  async function persistProjectEnv(after?: "deploy" | "restart") {
    if (!envModal) return;
    setBusy(`env:${envModal.project.id}`);
    try {
      const rows = normalizeEnvRows(envModal.rows.filter((row) => row.key.trim()));
      const payload = rows.map((row) => {
        const original = envModal.baseline.find((item) => item.key === row.key);
        if (original?.masked && original.value === row.value) {
          return { key: row.key, masked: row.masked };
        }
        return row;
      });
      await api.updateProjectEnv(envModal.project.id, payload);
      if (after === "restart") {
        await api.restartProject(envModal.project.id);
      }
      if (after === "deploy") {
        await api.deployProject(envModal.project.id);
      }
      setEnvModal(null);
      await loadAll();
      setToast({ message: after === "restart" ? "Environment updated and restart started." : after === "deploy" ? "Environment updated and deployment started." : "Environment updated." });
    } catch (error) {
      setToast({ message: error instanceof Error ? error.message : "Unable to save environment.", kind: "error" });
    } finally {
      setBusy(null);
    }
  }

  async function saveProjectEnv(event: FormEvent) {
    event.preventDefault();
    await persistProjectEnv();
  }

  function openRollback(project: Project) {
    const projectDeployments = deployments.filter((deployment) => deployment.projectId === project.id && deployment.status === "success");
    setRollbackModal({ project, deployments: projectDeployments });
  }

  async function copyText(value: string) {
    await navigator.clipboard.writeText(value);
    setToast({ message: "Copied." });
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
          <Button variant="secondary" onClick={() => void loadAll()} icon={<RefreshCw size={16} />}>
            Refresh
          </Button>
        </header>

        {view === "dashboard" ? (
          <section className="dashboard">
            <section className="stat-grid">
              <StatTile label="Projects" value={projects.length} detail={`${settings.hostProjectsRoot} root`} />
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
              {visibleProjects.map((project) => (
                <article className="project-card" key={project.id}>
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
                        <dt>Auto start</dt>
                        <dd>{project.autoStart ? "On restart" : "Off"}</dd>
                      </div>
                    </dl>
                  </div>
                  <div className="project-card-side">
                    <div className="endpoint-box">
                      <span>{endpoint(project, settings.appBaseUrl)}</span>
                      <button type="button" onClick={() => void copyText(endpoint(project, settings.appBaseUrl))} title="Copy endpoint" aria-label="Copy endpoint">
                        <Copy size={15} />
                      </button>
                    </div>
                    <div className="token-box">
                      <span>Bearer {project.deployToken}</span>
                      <button type="button" onClick={() => void copyText(`Bearer ${project.deployToken}`)} title="Copy token" aria-label="Copy token">
                        <Copy size={15} />
                      </button>
                    </div>
                    <div className="project-side-stat">
                      <span>Containers</span>
                      <strong>{project.containerCount ?? 0} created</strong>
                    </div>
                    <div className="actions">
                      <Button variant="secondary" onClick={() => openProject(project)}>
                        Edit
                      </Button>
                      <Button variant="secondary" onClick={() => void openEnvEditor(project)} icon={<KeyRound size={15} />}>
                        Env
                      </Button>
                      <Button variant="secondary" onClick={() => openRollback(project)} icon={<Undo2 size={15} />}>
                        Rollback
                      </Button>
                      <Button disabled={busy === `deploy:${project.id}`} onClick={() => void deploy(project)} icon={<Play size={15} />}>
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
              ))}
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
          <section className="panel">
            <div className="panel-head">
              <h2>Backup history</h2>
              <div className="actions">
                <span className="count">{backups.length} dumps</span>
                <Button
                  disabled={busy === "backup"}
                  onClick={async () => {
                    setBusy("backup");
                    try {
                      await api.createBackup();
                      await loadAll();
                      setToast({ message: "Postgres backup created." });
                    } catch (error) {
                      setToast({ message: error instanceof Error ? error.message : "Unable to create backup.", kind: "error" });
                    } finally {
                      setBusy(null);
                    }
                  }}
                  icon={<Archive size={16} />}
                >
                  Dump Postgres
                </Button>
              </div>
            </div>
            <BackupTable backups={visibleBackups} />
            <Pagination label="Backups" page={backupPage} totalItems={backups.length} onPageChange={setBackupPage} />
          </section>
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

        {view === "settings" ? (
          <section className="settings-grid">
            <section className="panel webhook-settings">
              <div className="panel-head">
                <h2>Deployment webhook</h2>
                <GitBranch size={19} />
              </div>
              <p className="muted">Use this endpoint from Git webhooks or your own automation. Replace the project id and token with values from the project card.</p>
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
                <p className="muted">Get the token from Projects, then copy the Bearer token shown on that project.</p>
              </div>
            </section>
            <section className="panel webhook-settings">
              <div className="panel-head">
                <h2>Git SSH key</h2>
                <KeyRound size={19} />
              </div>
              <p className="muted">Paste the private key Yanto should use for Git clone and pull. It is stored as a file in the persistent SSH volume, not shown again after saving.</p>
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
            <section className="panel">
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
                  onClick={async () => {
                    const result = await api.cleanupPreview();
                    setCleanupPreviewed(true);
                    setLogModal({ title: "Cleanup preview", logs: result.logs });
                  }}
                  icon={<DatabaseZap size={16} />}
                >
                  Preview cleanup
                </Button>
              <Button
                variant="danger"
                disabled={!cleanupPreviewed}
                onClick={() =>
                  setConfirm({
                    title: "Run cleanup",
                    body: "This removes unused Docker cache and unused Docker resources shown by the preview. Running containers, named volumes, and Yanto containers are protected.",
                    label: "Clean cache",
                    danger: true,
                    action: async () => {
                      const result = await api.cleanup();
                      setCleanupPreviewed(false);
                      setLogModal({ title: "Cleanup logs", logs: result.logs });
                    }
                  })
                }
                icon={<Trash2 size={16} />}
              >
                Clean cache
              </Button>
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
        <Modal title={projectModal === "new" ? "Add project" : "Edit project"} onClose={() => setProjectModal(null)}>
          <form className="form-grid" onSubmit={saveProject}>
            <TextField label="Name" value={projectForm.name} onChange={(name) => setProjectForm((current) => ({ ...current, name }))} required />
            <TextField label="Git SSH URL" value={projectForm.gitUrl} onChange={(gitUrl) => setProjectForm((current) => ({ ...current, gitUrl }))} placeholder="Optional: git@github.com:user/repo.git" />
            <TextField label="Branch" value={projectForm.branch} onChange={(branch) => setProjectForm((current) => ({ ...current, branch }))} required />
            <TextField label="Folder name" value={projectForm.folderName} onChange={(folderName) => setProjectForm((current) => ({ ...current, folderName }))} placeholder={slugifyFolderName(projectForm.name) || "Auto from project name"} />
            <TextField label="Compose file" value={projectForm.composeFile} onChange={(composeFile) => setProjectForm((current) => ({ ...current, composeFile }))} placeholder="docker-compose.yml" required />
            <ToggleField
              label="Auto start after restart"
              value={projectForm.autoStart}
              onChange={(autoStart) => setProjectForm((current) => ({ ...current, autoStart }))}
              description="Deploy with a Yanto compose override that sets restart: unless-stopped."
            />
            <TextAreaField
              label="Compose editor"
              value={projectForm.composeContent}
              onChange={(composeContent) => setProjectForm((current) => ({ ...current, composeContent }))}
              placeholder={"Optional. Paste docker-compose.yml content here for compose-only projects or to override the file during deploy."}
            />
            <div className="actions">
              <Button variant="secondary" onClick={() => setProjectModal(null)}>
                Cancel
              </Button>
              <Button type="submit" disabled={busy === "project"}>
                Save project
              </Button>
            </div>
          </form>
        </Modal>
      ) : null}

      {envModal ? (
        <Modal title={`${envModal.project.name} environment`} onClose={() => setEnvModal(null)}>
          {envModal.loading ? (
            <LoadingInline label="Loading environment" />
          ) : (
            <form className="form-grid" onSubmit={saveProjectEnv}>
              <EnvEditor modal={envModal} onChange={setEnvModal} />
              <div className="actions">
                <Button variant="secondary" onClick={() => setEnvModal(null)}>
                  Cancel
                </Button>
                  <Button type="submit" disabled={busy === `env:${envModal.project.id}`}>
                    Save
                  </Button>
                  <Button type="button" variant="secondary" disabled={busy === `env:${envModal.project.id}`} onClick={() => void persistProjectEnv("restart")} icon={<RotateCw size={15} />}>
                    Save & Restart
                  </Button>
                  <Button type="button" disabled={busy === `env:${envModal.project.id}`} onClick={() => void persistProjectEnv("deploy")} icon={<Play size={15} />}>
                    Save & Deploy
                  </Button>
                </div>
              </form>
          )}
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

function BackupTable({ backups }: { backups: BackupRecord[] }) {
  if (!backups.length) {
    return <p className="muted">No backups recorded yet.</p>;
  }

  return (
    <div className="table-wrap">
      <table>
        <thead>
          <tr>
            <th>Backup</th>
            <th>Status</th>
            <th>Size</th>
            <th>Created</th>
            <th>Duration</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {backups.map((backup) => (
            <tr key={backup.id}>
              <td>{backup.filename || backup.id}</td>
              <td><StatusBadge status={backup.status} /></td>
              <td>{backup.fileSizeBytes ? bytes(backup.fileSizeBytes) : "-"}</td>
              <td>{dateTime(backup.createdAt)}</td>
              <td>{durationBetween(backup.createdAt, backup.finishedAt)}</td>
              <td className="table-actions">
                <a className={`button secondary link-button ${backup.status !== "success" ? "disabled" : ""}`} href={backup.status === "success" ? api.backupDownloadUrl(backup.id) : undefined}>
                  <Download size={15} />
                  <span>Download</span>
                </a>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function AuditTable({ entries }: { entries: AuditLogEntry[] }) {
  if (!entries.length) {
    return <p className="muted">No audit events recorded yet.</p>;
  }

  return (
    <div className="table-wrap">
      <table>
        <thead>
          <tr>
            <th>Time</th>
            <th>Actor</th>
            <th>Action</th>
            <th>Target</th>
            <th>Status</th>
            <th>Message</th>
          </tr>
        </thead>
        <tbody>
          {entries.map((entry) => (
            <tr key={entry.id}>
              <td>{dateTime(entry.createdAt)}</td>
              <td>{entry.actor ?? "system"}</td>
              <td>{entry.action}</td>
              <td>{entry.entityId ? `${entry.entityType}:${entry.entityId}` : entry.entityType}</td>
              <td><StatusBadge status="recorded" /></td>
              <td>{JSON.stringify(entry.metadata ?? {})}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function ContainerGroups({
  containers,
  onLogs,
  onConfirm,
  onReload
}: {
  containers: ContainerInfo[];
  onLogs: (container: ContainerInfo) => void;
  onConfirm: (confirm: { title: string; body: string; label: string; danger?: boolean; action: () => Promise<void> }) => void;
  onReload: () => Promise<void>;
}) {
  const groups = Array.from(
    containers.reduce((map, container) => {
      const key = container.composeProject || "standalone";
      map.set(key, [...(map.get(key) ?? []), container]);
      return map;
    }, new Map<string, ContainerInfo[]>())
  ).sort(([a], [b]) => a.localeCompare(b));

  if (!groups.length) {
    return <p className="muted">No containers found yet.</p>;
  }

  return (
    <div className="container-groups">
      {groups.map(([group, rows]) => (
        <details key={group} open>
          <summary>
            <span>{group}</span>
            <small>{rows.filter((container) => container.state === "running").length} / {rows.length} running</small>
          </summary>
          <div className="table-wrap">
            <table>
              <thead>
                <tr>
                  <th>Name</th>
                  <th>Image</th>
                  <th>Status</th>
                  <th>Uptime</th>
                  <th>Ports</th>
                  <th>CPU</th>
                  <th>Memory</th>
                  <th></th>
                </tr>
              </thead>
              <tbody>
                {rows.map((container) => {
                  const protectedContainer = isProtectedYantoContainer(container);
                  return (
                    <tr key={container.id}>
                      <td>{container.name}</td>
                      <td>{container.image}</td>
                      <td><StatusBadge status={container.state} /></td>
                      <td title={dateTime(container.createdAt)}>{durationSince(container.createdAt)}</td>
                      <td className="ports-cell">{container.ports || "-"}</td>
                      <td>{container.cpuPercent}</td>
                      <td>{usedMemoryMb(container.memoryUsage)} ({container.memoryPercent})</td>
                      <td className="action-cell">
                        {protectedContainer ? (
                          <span className="protected-label">Protected</span>
                        ) : (
                          <div className="table-actions icon-actions">
                            <IconButton label="View logs" variant="secondary" onClick={() => void onLogs(container)}>
                              <ScrollText size={15} />
                            </IconButton>
                            <IconButton
                              label="Restart container"
                              variant="secondary"
                              onClick={() =>
                                onConfirm({
                                  title: "Restart container",
                                  body: `Restart ${container.name}?`,
                                  label: "Restart",
                                  action: async () => {
                                    await api.restartContainer(container.id);
                                    await onReload();
                                  }
                                })
                              }
                            >
                              <RotateCw size={15} />
                            </IconButton>
                            <IconButton
                              label="Stop container"
                              variant="danger"
                              onClick={() =>
                                onConfirm({
                                  title: "Stop container",
                                  body: `Stop ${container.name}?`,
                                  label: "Stop",
                                  danger: true,
                                  action: async () => {
                                    await api.stopContainer(container.id);
                                    await onReload();
                                  }
                                })
                              }
                            >
                              <Square size={15} />
                            </IconButton>
                          </div>
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </details>
      ))}
    </div>
  );
}

function EnvEditor({ modal, onChange }: { modal: EnvModalState; onChange: (next: EnvModalState) => void }) {
  const baselineKeys = new Set(modal.baseline.map((row) => row.key));
  const currentKeys = new Set(modal.rows.map((row) => row.key));
  const changedRows = modal.rows.filter((row) => {
    const original = modal.baseline.find((item) => item.key === row.key);
    return !original || original.value !== row.value || original.masked !== row.masked;
  });
  const removedRows = modal.baseline.filter((row) => !currentKeys.has(row.key));

  return (
    <div className="env-editor">
      <div className="env-rows">
        {modal.rows.map((row, index) => (
          <div className="env-row" key={`${row.key}:${index}`}>
            <TextField label="Key" value={row.key} onChange={(key) => onChange({ ...modal, rows: modal.rows.map((item, rowIndex) => (rowIndex === index ? { ...item, key } : item)) })} />
            <TextField
              label="Value"
              type={row.masked ? "password" : "text"}
              value={row.value ?? ""}
              onChange={(value) => onChange({ ...modal, rows: modal.rows.map((item, rowIndex) => (rowIndex === index ? { ...item, value } : item)) })}
            />
            <ToggleField
              label="Masked"
              value={Boolean(row.masked)}
              onChange={(masked) => onChange({ ...modal, rows: modal.rows.map((item, rowIndex) => (rowIndex === index ? { ...item, masked } : item)) })}
            />
            <IconButton label="Remove variable" variant="danger" onClick={() => onChange({ ...modal, rows: modal.rows.filter((_, rowIndex) => rowIndex !== index) })}>
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
            onChange({ ...modal, draftKey: "", draftValue: "", rows: normalizeEnvRows([...modal.rows, { key, value: modal.draftValue, masked: true }]) });
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
    </div>
  );
}

function DeploymentTable({ deployments, onLogs, compact }: { deployments: Deployment[]; onLogs: (deployment: Deployment) => void; compact?: boolean }) {
  if (!deployments.length) {
    return <p className="muted">No deployments recorded yet.</p>;
  }

  return (
    <div className="table-wrap">
      <table>
        <thead>
          <tr>
            <th>Project</th>
            <th>Trigger</th>
            <th>Started</th>
            <th>Status</th>
            <th>Duration</th>
            {!compact ? <th>Changes</th> : null}
            <th></th>
          </tr>
        </thead>
        <tbody>
          {deployments.map((deployment) => (
            <tr key={deployment.id}>
              <td>{deployment.projectName ?? deployment.projectId}</td>
              <td>{deployment.trigger}</td>
              <td>{dateTime(deployment.startedAt)}</td>
              <td><StatusBadge status={deployment.status} /></td>
              <td>{durationBetween(deployment.startedAt, deployment.finishedAt)}</td>
              {!compact ? <td>{deploymentChanges(deployment)}</td> : null}
              <td className="table-actions">
                <Button variant="secondary" onClick={() => onLogs(deployment)}>
                  Logs
                </Button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
