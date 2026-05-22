import {
  Activity,
  Boxes,
  ChevronLeft,
  ChevronRight,
  Container,
  Copy,
  DatabaseZap,
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
  Square,
  Trash2
} from "lucide-react";
import { FormEvent, type ReactNode, useCallback, useEffect, useMemo, useState } from "react";
import type { ContainerInfo, Deployment, Project, SystemUsage } from "../shared/types";
import { Button, ConfirmDialog, IconButton, LoadingInline, LogViewer, Modal, StatusBadge, TextAreaField, TextField, Toast } from "./components/ui";
import { api } from "./lib/api";

type View = "dashboard" | "projects" | "deployments" | "containers" | "settings";
type ToastState = { message: string; kind?: "ok" | "error" } | null;
type LogModalState = { title: string; logs: string; streamPath?: string; live?: boolean; status?: string };
type LogStreamPayload = { logs?: string; chunk?: string; status?: string; error?: string; done?: boolean };

const emptyProject = {
  name: "",
  gitUrl: "",
  branch: "master",
  folderName: "",
  composeFile: "docker-compose.yml",
  composeContent: ""
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

export function App() {
  const [user, setUser] = useState<string | null>(null);
  const [login, setLogin] = useState({ username: "admin", password: "" });
  const [view, setView] = useState<View>("dashboard");
  const [projects, setProjects] = useState<Project[]>([]);
  const [deployments, setDeployments] = useState<Deployment[]>([]);
  const [containers, setContainers] = useState<ContainerInfo[]>([]);
  const [usage, setUsage] = useState<SystemUsage | null>(null);
  const [settings, setSettings] = useState({ projectsRoot: "/projects", hostProjectsRoot: "~/projects", sshKeysDir: "", appBaseUrl: "", sshKey: emptySshKeySettings });
  const [loading, setLoading] = useState(true);
  const [toast, setToast] = useState<ToastState>(null);
  const [projectModal, setProjectModal] = useState<Project | "new" | null>(null);
  const [projectForm, setProjectForm] = useState(emptyProject);
  const [logModal, setLogModal] = useState<LogModalState | null>(null);
  const [confirm, setConfirm] = useState<{ title: string; body: string; label: string; danger?: boolean; action: () => Promise<void> } | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [sshPrivateKey, setSshPrivateKey] = useState("");
  const [projectPage, setProjectPage] = useState(1);
  const [deploymentPage, setDeploymentPage] = useState(1);
  const [containerPage, setContainerPage] = useState(1);

  const loadAll = useCallback(async () => {
    const [projectRows, deploymentRows, containerRows, systemRows, settingRows] = await Promise.all([
      api.projects(),
      api.deployments(),
      api.containers().catch(() => []),
      api.systemUsage().catch(() => null),
      api.settings()
    ]);
    setProjects(projectRows);
    setDeployments(deploymentRows);
    setContainers(containerRows);
    setUsage(systemRows);
    setSettings(settingRows);
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
  const visibleProjects = useMemo(() => pageItems(projects, projectPage), [projectPage, projects]);
  const visibleDeployments = useMemo(() => pageItems(deployments, deploymentPage), [deploymentPage, deployments]);
  const visibleContainers = useMemo(() => pageItems(containers, containerPage), [containerPage, containers]);

  useEffect(() => {
    setProjectPage((page) => Math.min(page, totalPages(projects)));
  }, [projects]);

  useEffect(() => {
    setDeploymentPage((page) => Math.min(page, totalPages(deployments)));
  }, [deployments]);

  useEffect(() => {
    setContainerPage((page) => Math.min(page, totalPages(containers)));
  }, [containers]);

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
        composeContent: project.composeContent ?? ""
      });
      setProjectModal(project);
      return;
    }
    setProjectForm(emptyProject);
    setProjectModal("new");
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
            {view !== "dashboard" ? <p>{projects.length} projects, {containers.length} containers, {runningDeployments.length} active deployments</p> : null}
          </div>
          <Button variant="secondary" onClick={() => void loadAll()} icon={<RefreshCw size={16} />}>
            Refresh
          </Button>
        </header>

        {view === "dashboard" ? (
          <section className="dashboard">
            <section className="stat-grid">
              <StatTile label="Projects" value={projects.length} detail={`${settings.hostProjectsRoot} root`} />
              <StatTile label="Active deploys" value={runningDeployments.length} detail={runningDeployments.length ? "Deployment in progress" : "Queue is clear"} />
              <StatTile label="Running containers" value={containers.filter((container) => container.state === "running").length} detail={`${containers.length} total containers`} />
              <StatTile label="RAM used" value={usage ? `${usage.memory.usedPercent}%` : "-"} detail={usage ? `${bytes(usage.memory.used)} of ${bytes(usage.memory.total)}` : "Unavailable"} />
            </section>

            <div className="dashboard-main-grid">
              <div className="dashboard-left-column">
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
                  </dl>
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
                  <div className="actions">
                    <Button variant="secondary" onClick={() => openProject(project)}>
                      Edit
                    </Button>
                    <Button disabled={busy === `deploy:${project.id}`} onClick={() => void deploy(project)} icon={<Play size={15} />}>
                      Deploy
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

        {view === "containers" ? (
          <section className="panel">
            <div className="panel-head">
              <h2>Docker containers</h2>
              <span className="count">{containers.length} found</span>
            </div>
            <div className="table-wrap">
              <table>
                <thead>
                  <tr>
                    <th>Name</th>
                    <th>Image</th>
                    <th>Status</th>
                    <th>Ports</th>
                    <th>CPU</th>
                    <th>Memory</th>
                    <th></th>
                  </tr>
                </thead>
                <tbody>
                  {visibleContainers.map((container) => (
                    <tr key={container.id}>
                      <td>{container.name}</td>
                      <td>{container.image}</td>
                      <td><StatusBadge status={container.state} /></td>
                      <td className="ports-cell">{container.ports || "-"}</td>
                      <td>{container.cpuPercent}</td>
                      <td>{usedMemoryMb(container.memoryUsage)} ({container.memoryPercent})</td>
                      <td className="table-actions icon-actions">
                        <IconButton label="View logs" variant="secondary" onClick={() => void openContainerLogs(container)}>
                          <ScrollText size={15} />
                        </IconButton>
                        <IconButton
                          label="Restart container"
                          variant="secondary"
                          onClick={() =>
                            setConfirm({
                              title: "Restart container",
                              body: `Restart ${container.name}?`,
                              label: "Restart",
                              action: async () => {
                                await api.restartContainer(container.id);
                                await loadAll();
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
                            setConfirm({
                              title: "Stop container",
                              body: `Stop ${container.name}?`,
                              label: "Stop",
                              danger: true,
                              action: async () => {
                                await api.stopContainer(container.id);
                                await loadAll();
                              }
                            })
                          }
                        >
                          <Square size={15} />
                        </IconButton>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            <Pagination label="Containers" page={containerPage} totalItems={containers.length} onPageChange={setContainerPage} />
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
              <p className="muted">Clean Docker builder cache, dangling images, unused containers, unused networks, and supported package caches.</p>
              <Button
                variant="danger"
                onClick={() =>
                  setConfirm({
                    title: "Run cleanup",
                    body: "This removes unused Docker cache and unused Docker resources. Running containers are not removed.",
                    label: "Clean cache",
                    danger: true,
                    action: async () => {
                      const result = await api.cleanup();
                      setLogModal({ title: "Cleanup logs", logs: result.logs });
                    }
                  })
                }
                icon={<Trash2 size={16} />}
              >
                Clean cache
              </Button>
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
          <Meter label="CPU load" value={usage.cpuLoadPercent} icon={<Activity size={15} />} />
          <Meter label="RAM" value={usage.memory.usedPercent} detail={`${bytes(usage.memory.used)} / ${bytes(usage.memory.total)}`} icon={<MemoryStick size={15} />} />
          {storage ? <Meter label={`Storage ${storage.mount}`} value={storage.usedPercent} detail={`${bytes(storage.used)} / ${bytes(storage.size)}`} icon={<HardDrive size={15} />} /> : null}
        </div>
      ) : (
        <p className="muted">System usage is unavailable. Check the container permissions and mounted project path.</p>
      )}
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

function Meter({ label, value, detail, icon }: { label: string; value: number; detail?: string; icon?: ReactNode }) {
  return (
    <div className="meter">
      <div>
        <span>{icon}{label}</span>
        <strong>{value}%</strong>
      </div>
      <div className="meter-track">
        <span style={{ width: `${Math.min(100, Math.max(0, value))}%` }} />
      </div>
      {detail ? <small>{detail}</small> : null}
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
            <th>Status</th>
            <th>Trigger</th>
            <th>Started</th>
            {!compact ? <th>Finished</th> : null}
            <th></th>
          </tr>
        </thead>
        <tbody>
          {deployments.map((deployment) => (
            <tr key={deployment.id}>
              <td>{deployment.projectName ?? deployment.projectId}</td>
              <td><StatusBadge status={deployment.status} /></td>
              <td>{deployment.trigger}</td>
              <td>{dateTime(deployment.startedAt)}</td>
              {!compact ? <td>{dateTime(deployment.finishedAt)}</td> : null}
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
