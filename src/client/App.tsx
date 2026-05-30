import {
  Activity, Archive, Boxes, Container, FileClock, GitBranch,
  KeyRound, LogOut, Moon, RefreshCw, Server, Settings, Sun
} from "lucide-react";
import { DashboardView } from "./views";
import { lazy, Suspense, useCallback, useEffect, useMemo, useState } from "react";
import type { FormEvent } from "react";

const AuditView = lazy(() => import("./views/AuditView").then(m => ({ default: m.AuditView })));
const BackupsView = lazy(() => import("./views/BackupsView").then(m => ({ default: m.BackupsView })));
const ContainersView = lazy(() => import("./views/ContainersView").then(m => ({ default: m.ContainersView })));
const DeploymentsView = lazy(() => import("./views/DeploymentsView").then(m => ({ default: m.DeploymentsView })));
const NodesView = lazy(() => import("./views/NodesView").then(m => ({ default: m.NodesView })));
const ProjectsView = lazy(() => import("./views/ProjectsView").then(m => ({ default: m.ProjectsView })));
const SettingsView = lazy(() => import("./views/SettingsView").then(m => ({ default: m.SettingsView })));

import type { SystemUsage } from "../shared/types";
import { api } from "./lib/api";
import { Button, LoadingInline, TextField } from "./components/ui";
import { YantoBootLoader } from "./components/YantoBootLoader";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { ProjectModal, CreatedProjectSecretModal, RollbackModal, SetupWizardModal } from "./components/ProjectModal";

import { ToastProvider, useToast } from "./contexts/ToastContext";
import { ConfirmProvider, useConfirm } from "./contexts/ConfirmContext";
import { LogModalProvider } from "./contexts/LogModalContext";
import { AuthProvider, useAuth } from "./contexts/AuthContext";

import { useProjects } from "./hooks/useProjects";
import { useDeployments } from "./hooks/useDeployments";
import { useContainers } from "./hooks/useContainers";
import { useBackups } from "./hooks/useBackups";
import { useSettings } from "./hooks/useSettings";
import { useNodes } from "./hooks/useNodes";
import { useAuditLog } from "./hooks/useAuditLog";

import type { ThemeMode, View } from "./types";
import { getInitialTheme, themeStorageKey } from "./types";

const navItems: { id: View; icon: typeof Activity; label: string }[] = [
  { id: "dashboard", icon: Activity, label: "Dashboard" },
  { id: "projects", icon: GitBranch, label: "Projects" },
  { id: "deployments", icon: Boxes, label: "Deployments" },
  { id: "containers", icon: Container, label: "Containers" },
  { id: "nodes", icon: Server, label: "Nodes" },
  { id: "backups", icon: Archive, label: "Backups" },
  { id: "audit", icon: FileClock, label: "Audit" },
  { id: "settings", icon: Settings, label: "Settings" },
];

export function App() {
  return (
    <ToastProvider>
      <ConfirmProvider>
        <LogModalProvider>
          <AuthProvider>
            <AuthGate />
          </AuthProvider>
        </LogModalProvider>
      </ConfirmProvider>
    </ToastProvider>
  );
}

function AuthGate() {
  const { user, loading, login } = useAuth();
  const [loginForm, setLoginForm] = useState({ username: "admin", password: "" });
  const [loginBusy, setLoginBusy] = useState(false);

  if (loading) {
    return <main className="login-shell"><YantoBootLoader /></main>;
  }

  if (!user) {
    const submitLogin = async (event: FormEvent) => {
      event.preventDefault();
      setLoginBusy(true);
      try { await login(loginForm.username, loginForm.password); } catch { /* toast handled by AuthProvider */ } finally { setLoginBusy(false); }
    };
    return (
      <main className="login-shell">
        <form className="login-panel" onSubmit={submitLogin}>
          <h1>Yanto Deploy</h1>
          <p>Sign in to manage projects, containers, deployments, and host cleanup.</p>
          <TextField label="Username" value={loginForm.username} onChange={(username) => setLoginForm((c) => ({ ...c, username }))} required />
          <TextField label="Password" type="password" value={loginForm.password} onChange={(password) => setLoginForm((c) => ({ ...c, password }))} required />
          <Button type="submit" disabled={loginBusy} icon={loginBusy ? <RefreshCw size={16} className="spin" /> : <KeyRound size={16} />}>Sign in</Button>
        </form>
      </main>
    );
  }

  return <AppShell />;
}

function AppShell() {
  const { logout } = useAuth();
  const { showToast } = useToast();
  const { showConfirm } = useConfirm();

  const [view, setView] = useState<View>("dashboard");
  const [theme, setTheme] = useState<ThemeMode>(getInitialTheme);
  const [usage, setUsage] = useState<SystemUsage | null>(null);
  const [refreshBusy, setRefreshBusy] = useState(false);

  // Domain hooks
  const settingsHook = useSettings();
  const nodesHook = useNodes();
  const containersHook = useContainers();
  const deploymentsHook = useDeployments();
  const projectsHook = useProjects({
    nodes: nodesHook.nodes,
    containers: containersHook.containers,
    settings: settingsHook.settings,
  });
  const backupsHook = useBackups();
  const auditHook = useAuditLog();

  // Sync deployments: projects hook refreshes both projects+deployments
  useEffect(() => {
    if (projectsHook.deployments.length) {
      deploymentsHook.setDeployments(projectsHook.deployments);
    }
  }, [projectsHook.deployments]);

  // Theme
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    document.documentElement.style.colorScheme = theme;
    window.localStorage.setItem(themeStorageKey, theme);
  }, [theme]);

  // Blur settings fields on view change
  useEffect(() => {
    if (view !== "settings") return;
    window.requestAnimationFrame(() => {
      const el = document.activeElement;
      if (el instanceof HTMLElement && el.closest(".settings-grid")) el.blur();
    });
  }, [view]);

  // Data loading per view
  const loadView = useCallback(async (targetView: View) => {
    if (targetView === "dashboard") {
      await Promise.all([
        projectsHook.refreshProjects(),
        containersHook.refreshContainers(),
        nodesHook.refreshNodes(),
        api.systemUsage().catch(() => null).then(setUsage),
        settingsHook.refreshSettings(),
      ]);
      return;
    }
    if (targetView === "projects") {
      await Promise.all([
        projectsHook.refreshProjects(),
        containersHook.refreshContainers(),
        nodesHook.refreshNodes(),
        settingsHook.refreshSettings(),
      ]);
      return;
    }
    if (targetView === "deployments") {
      await deploymentsHook.refreshDeployments();
      return;
    }
    if (targetView === "containers") {
      await containersHook.refreshContainers();
      return;
    }
    if (targetView === "nodes") {
      await nodesHook.refreshNodes();
      return;
    }
    if (targetView === "backups") {
      await backupsHook.refreshBackups();
      return;
    }
    if (targetView === "audit") {
      await auditHook.refreshAuditLog();
      return;
    }
    // settings
    await Promise.all([settingsHook.refreshSettings(), settingsHook.refreshSystemLogs()]);
  }, [
    projectsHook.refreshProjects, containersHook.refreshContainers,
    nodesHook.refreshNodes, settingsHook.refreshSettings,
    settingsHook.refreshSystemLogs, deploymentsHook.refreshDeployments,
    backupsHook.refreshBackups, auditHook.refreshAuditLog,
  ]);

  // Initial load + view changes
  useEffect(() => {
    void loadView(view).catch(() => undefined);
  }, [loadView, view]);

  // 30s polling
  useEffect(() => {
    const timer = window.setInterval(() => {
      void loadView(view).catch(() => undefined);
    }, 30000);
    return () => window.clearInterval(timer);
  }, [loadView, view]);

  async function refreshCurrentView() {
    setRefreshBusy(true);
    showToast(`Refreshing ${view}...`, "loading");
    try {
      await loadView(view);
      showToast("View refreshed.");
    } catch (error) {
      showToast(error instanceof Error ? error.message : "Unable to refresh view.", "error");
    } finally {
      setRefreshBusy(false);
    }
  }

  async function copyText(value: string) {
    try {
      await navigator.clipboard.writeText(value);
      showToast("Copied to clipboard.");
    } catch {
      showToast("Unable to copy.", "error");
    }
  }

  // Computed values
  const failingProjects = useMemo(
    () => projectsHook.projects.filter((p) => projectsHook.latestDeploymentByProject.get(p.id)?.status === "failed"),
    [projectsHook.projects, projectsHook.latestDeploymentByProject]
  );
  const warningDisks = useMemo(() => usage?.storage.filter((d) => d.usedPercent >= 80) ?? [], [usage]);

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <div>
            <strong>Yanto</strong>
            <span>{settingsHook.settings.hostProjectsRoot}</span>
          </div>
        </div>
        <nav>
          {navItems.map(({ id, icon: Icon, label }) => (
            <button key={id} className={view === id ? "active" : ""} type="button" onClick={() => setView(id)}>
              <Icon size={17} />
              <span>{label}</span>
            </button>
          ))}
        </nav>
        <div className="sidebar-footer">
          <button className="theme-toggle" type="button" role="switch" aria-checked={theme === "dark"} aria-label="Toggle dark mode" title="Toggle dark mode" onClick={() => setTheme((c) => (c === "dark" ? "light" : "dark"))}>
            {theme === "dark" ? <Moon size={17} /> : <Sun size={17} />}
            <span className="theme-toggle-text">Dark mode</span>
            <span className={`toggle-switch theme-toggle-switch ${theme === "dark" ? "on" : ""}`} aria-hidden="true"><span /></span>
          </button>
          <button className="logout" type="button" onClick={() => void logout()}>
            <LogOut size={17} />
            <span>Sign out</span>
          </button>
        </div>
      </aside>

      <main className="main">
        <header className="topbar">
          <div>
            <h1>{view[0].toUpperCase() + view.slice(1)}</h1>
          </div>
          <Button variant="secondary" disabled={refreshBusy} onClick={() => void refreshCurrentView()} icon={<RefreshCw size={16} className={refreshBusy ? "spin" : ""} />}>
            Refresh
          </Button>
        </header>

        <ErrorBoundary>
          {view === "dashboard" ? (
            <DashboardView
              projects={projectsHook.projects}
              nodes={nodesHook.nodes}
              containers={containersHook.containers}
              deployments={projectsHook.deployments}
              runningDeployments={deploymentsHook.runningDeployments}
              usage={usage}
              settings={settingsHook.settings}
              setupCanReopen={settingsHook.setupCanReopen}
              failingProjects={failingProjects}
              unhealthyContainers={containersHook.unhealthyContainers}
              warningDisks={warningDisks}
              openSetupWizard={settingsHook.openSetupWizard}
              openDeploymentLogs={deploymentsHook.openDeploymentLogs}
            />
          ) : null}

          <Suspense fallback={<LoadingInline label="Loading..." />}>
            {view === "projects" ? (
              <ProjectsView
                visibleProjects={projectsHook.visibleProjects}
                projects={projectsHook.projects}
                containersByProjectFolder={projectsHook.containersByProjectFolder}
                cfRoutesByProject={projectsHook.cfRoutesByProject}
                latestDeploymentByProject={projectsHook.latestDeploymentByProject}
                settings={settingsHook.settings}
                busy={projectsHook.busy}
                projectPage={projectsHook.projectPage}
                openProject={projectsHook.openProject}
                openRollback={projectsHook.openRollback}
                deploy={projectsHook.deploy}
                copyText={copyText}
                setConfirm={showConfirm}
                refreshProjects={projectsHook.refreshProjects}
                setProjectPage={projectsHook.setProjectPage}
              />
            ) : null}

            {view === "deployments" ? (
              <DeploymentsView
                deployments={deploymentsHook.deployments}
                visibleDeployments={deploymentsHook.visibleDeployments}
                deploymentPage={deploymentsHook.deploymentPage}
                openDeploymentLogs={deploymentsHook.openDeploymentLogs}
                setDeploymentPage={deploymentsHook.setDeploymentPage}
              />
            ) : null}

            {view === "backups" ? (
              <BackupsView
                postgresTargets={backupsHook.postgresTargets}
                visibleBackups={backupsHook.visibleBackups}
                backups={backupsHook.backups}
                busy={backupsHook.busy}
                r2Ready={settingsHook.r2Ready}
                backupPage={backupsHook.backupPage}
                dumpPostgresTarget={backupsHook.dumpPostgresTarget}
                restorePostgresTarget={backupsHook.restorePostgresTarget}
                uploadBackupR2={backupsHook.uploadBackupR2}
                setConfirm={showConfirm}
                refreshBackups={backupsHook.refreshBackups}
                setBackupPage={backupsHook.setBackupPage}
              />
            ) : null}

            {view === "audit" ? (
              <AuditView
                auditEntries={auditHook.auditEntries}
                visibleAuditEntries={auditHook.visibleAuditEntries}
                auditPage={auditHook.auditPage}
                setAuditPage={auditHook.setAuditPage}
              />
            ) : null}

            {view === "containers" ? (
              <ContainersView
                containers={containersHook.containers}
                openContainerLogs={containersHook.openContainerLogs}
                setConfirm={showConfirm}
                refreshContainers={containersHook.refreshContainers}
              />
            ) : null}

            {view === "nodes" ? (
              <NodesView nodes={nodesHook.nodes} />
            ) : null}

            {view === "settings" ? (
              <SettingsView
                settings={settingsHook.settings}
                r2Form={settingsHook.r2Form}
                cfForm={settingsHook.cfForm}
                busy={settingsHook.busy}
                sshPrivateKey={settingsHook.sshPrivateKey}
                systemLogs={settingsHook.systemLogs}
                cleanupLogs={settingsHook.cleanupLogs}
                cleanupLogTitle={settingsHook.cleanupLogTitle}
                cleanupPreviewed={settingsHook.cleanupPreviewed}
                updateR2Form={settingsHook.updateR2Form}
                updateCfForm={settingsHook.updateCfForm}
                saveR2Settings={settingsHook.saveR2Settings}
                saveCfSettings={settingsHook.saveCfSettings}
                validateCfSettings={settingsHook.validateCfSettings}
                saveSshPrivateKey={settingsHook.saveSshPrivateKey}
                setSshPrivateKey={settingsHook.setSshPrivateKey}
                copyText={copyText}
                copyWorkerInstallCommand={settingsHook.copyWorkerInstallCommand}
                openSetupWizard={settingsHook.openSetupWizard}
                previewCleanup={settingsHook.previewCleanup}
                refreshSystemLogs={settingsHook.refreshSystemLogs}
                refreshContainers={containersHook.refreshContainers}
                setConfirm={showConfirm}
                setBusy={settingsHook.setBusy}
                setCleanupLogTitle={settingsHook.setCleanupLogTitle}
                setCleanupLogs={settingsHook.setCleanupLogs}
                setCleanupPreviewed={settingsHook.setCleanupPreviewed}
              />
            ) : null}
          </Suspense>
        </ErrorBoundary>
      </main>

      {projectsHook.projectModal ? (
        <ProjectModal
          projectModal={projectsHook.projectModal}
          projectForm={projectsHook.projectForm}
          setProjectForm={projectsHook.setProjectForm}
          projectEnv={projectsHook.projectEnv}
          setProjectEnv={projectsHook.setProjectEnv}
          projectCompose={projectsHook.projectCompose}
          projectEditorModal={projectsHook.projectEditorModal}
          setProjectEditorModal={projectsHook.setProjectEditorModal}
          cfRoutes={projectsHook.cfRoutes}
          cfRouteForm={projectsHook.cfRouteForm}
          setCfRouteForm={projectsHook.setCfRouteForm}
          cfSettingsReady={settingsHook.cfSettingsReady}
          nodeOptions={projectsHook.nodeOptions}
          busy={projectsHook.busy}
          onSave={(e) => void projectsHook.persistProjectDetails(e)}
          onSaveAndDeploy={() => void projectsHook.persistProjectDetails(undefined, "deploy")}
          onClose={() => projectsHook.setProjectModal(null)}
          openComposeEditor={projectsHook.openComposeEditor}
          openEnvEditor={projectsHook.openEnvEditor}
          publishCfRoute={projectsHook.publishCfRoute}
          toggleCfRoute={projectsHook.toggleCfRoute}
          removeCfRoute={projectsHook.removeCfRoute}
          copyText={copyText}
        />
      ) : null}

      {projectsHook.rollbackModal ? (
        <RollbackModal
          rollbackModal={projectsHook.rollbackModal}
          onClose={() => projectsHook.setRollbackModal(null)}
          onRollback={projectsHook.executeRollback}
        />
      ) : null}

      {projectsHook.createdProjectSecret ? (
        <CreatedProjectSecretModal
          secret={projectsHook.createdProjectSecret}
          onClose={() => projectsHook.setCreatedProjectSecret(null)}
          copyText={copyText}
        />
      ) : null}

      {settingsHook.setupModalOpen ? (
        <SetupWizardModal
          settings={settingsHook.settings}
          sshReady={settingsHook.sshReady}
          cfSettingsReady={settingsHook.cfSettingsReady}
          r2Ready={settingsHook.r2Ready}
          setupStep={settingsHook.setupStep}
          setupCanGoBack={settingsHook.setupCanGoBack}
          setupCanGoNext={settingsHook.setupCanGoNext}
          busy={settingsHook.busy}
          sshPrivateKey={settingsHook.sshPrivateKey}
          setSshPrivateKey={settingsHook.setSshPrivateKey}
          r2Form={settingsHook.r2Form}
          cfForm={settingsHook.cfForm}
          updateR2Form={settingsHook.updateR2Form}
          updateCfForm={settingsHook.updateCfForm}
          saveSshPrivateKey={settingsHook.saveSshPrivateKey}
          saveCfSettings={settingsHook.saveCfSettings}
          saveR2Settings={settingsHook.saveR2Settings}
          validateCfSettings={settingsHook.validateCfSettings}
          goToNextSetupStep={settingsHook.goToNextSetupStep}
          goToPreviousSetupStep={settingsHook.goToPreviousSetupStep}
          saveSetupWizard={settingsHook.saveSetupWizard}
          onClose={settingsHook.closeSetupWizard}
          setSetupStep={settingsHook.setSetupStep}
        />
      ) : null}
    </div>
  );
}
