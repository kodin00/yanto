import {
  ChevronLeft, ChevronRight, Cloud, Copy, FileText, KeyRound,
  List, Play, Plus, ShieldCheck, Trash2
} from "lucide-react";
import type { CloudflareRoute, Project } from "../../shared/types";
import { dateTime, durationBetween, slugifyFolderName } from "../app-utils";
import { EnvEditor, type ProjectEnvState } from "./EnvEditor";
import {
  Button, CustomSelect, IconButton, LoadingInline, Modal,
  StatusBadge, TextAreaField, TextField, ToggleField
} from "./ui";
import type {
  CfRouteForm, CfRouteProtocol, CreatedProjectSecret,
  ProjectComposeState, ProjectFormState
} from "../types";
import { parseCfServiceTarget } from "../types";

type ProjectModalProps = {
  projectModal: Project | "new";
  projectForm: ProjectFormState;
  setProjectForm: (fn: (current: ProjectFormState) => ProjectFormState) => void;
  projectEnv: ProjectEnvState;
  setProjectEnv: (state: ProjectEnvState) => void;
  projectCompose: ProjectComposeState;
  projectEditorModal: "compose" | "env" | null;
  setProjectEditorModal: (modal: "compose" | "env" | null) => void;
  cfRoutes: CloudflareRoute[];
  cfRouteForm: CfRouteForm;
  setCfRouteForm: (fn: (current: CfRouteForm) => CfRouteForm) => void;
  cfSettingsReady: boolean;
  nodeOptions: { label: string; value: string }[];
  busy: string | null;
  onSave: (event: React.FormEvent) => void;
  onSaveAndDeploy: () => void;
  onClose: () => void;
  openComposeEditor: () => void;
  openEnvEditor: () => void;
  publishCfRoute: (projectId: string) => void;
  toggleCfRoute: (route: CloudflareRoute) => void;
  removeCfRoute: (routeId: string) => void;
  copyText: (value: string) => Promise<void>;
};

export function ProjectModal(props: ProjectModalProps) {
  const {
    projectModal, projectForm, setProjectForm, projectEnv, setProjectEnv,
    projectCompose, projectEditorModal, setProjectEditorModal,
    cfRoutes, cfRouteForm, setCfRouteForm, cfSettingsReady, nodeOptions,
    busy,
    onSave, onSaveAndDeploy, onClose,
    openComposeEditor, openEnvEditor,
    publishCfRoute, toggleCfRoute, removeCfRoute, copyText
  } = props;

  return (
    <>
      <Modal title={projectModal === "new" ? "Add project" : "Edit project"} size="wide" closeOnEscape={!projectEditorModal} onClose={onClose}>
        <form className="project-edit-form" onSubmit={onSave}>
          <div className="project-edit-layout">
            <section className="project-edit-section">
              <div className="section-kicker">Project</div>
              <TextField label="Name" value={projectForm.name} onChange={(name) => setProjectForm((c) => ({ ...c, name }))} required />
              <TextField label="Git SSH URL" value={projectForm.gitUrl} onChange={(gitUrl) => setProjectForm((c) => ({ ...c, gitUrl }))} placeholder="Optional: git@github.com:user/repo.git" />
              <div className="project-edit-pair">
                <TextField label="Branch" value={projectForm.branch} onChange={(branch) => setProjectForm((c) => ({ ...c, branch }))} required />
                <TextField label="Compose file" value={projectForm.composeFile} onChange={(composeFile) => setProjectForm((c) => ({ ...c, composeFile }))} placeholder="docker-compose.yml" required />
              </div>
              <TextField label="Folder name" value={projectForm.folderName} onChange={(folderName) => setProjectForm((c) => ({ ...c, folderName }))} placeholder={slugifyFolderName(projectForm.name) || "Auto from project name"} />
              <CustomSelect label="Deployment node" value={projectForm.targetNodeId} options={nodeOptions} onChange={(targetNodeId) => setProjectForm((c) => ({ ...c, targetNodeId }))} />
              <ToggleField label="Auto start after restart" value={projectForm.autoStart} onChange={(autoStart) => setProjectForm((c) => ({ ...c, autoStart }))} description="Deploy with a Yanto compose override that sets restart: unless-stopped." />
              <ToggleField label="Manual API deployments" value={projectForm.manualDeployEnabled} onChange={(manualDeployEnabled) => setProjectForm((c) => ({ ...c, manualDeployEnabled }))} description="Allow deployments from the authenticated deploy action and token endpoint." />
              <ToggleField label="GitHub webhook deployments" value={projectForm.githubWebhookEnabled} onChange={(githubWebhookEnabled) => setProjectForm((c) => ({ ...c, githubWebhookEnabled }))} description="Allow signed GitHub push webhooks to deploy this project." />
            </section>

            <div className="project-edit-side">
              <section className="project-edit-section project-edit-tools">
                <div className="section-kicker">Editors</div>
                <button className="editor-launch-row" type="button" onClick={() => void openComposeEditor()} disabled={projectCompose.loading}>
                  <FileText size={16} />
                  <span><strong>Compose</strong><small>{projectCompose.loading ? "Opening..." : "Open editor"}</small></span>
                  <StatusBadge status="Open editor" />
                </button>
                <button className="editor-launch-row" type="button" onClick={() => void openEnvEditor()} disabled={projectEnv.loading}>
                  <List size={16} />
                  <span><strong>Environment</strong><small>{projectEnv.loading ? "Opening..." : "Open editor"}</small></span>
                  <StatusBadge status="Open editor" />
                </button>
              </section>

              {projectModal !== "new" ? (
                <CfRoutesSection
                  projectId={projectModal.id}
                  cfRoutes={cfRoutes}
                  cfRouteForm={cfRouteForm}
                  setCfRouteForm={setCfRouteForm}
                  cfSettingsReady={cfSettingsReady}
                  busy={busy}
                  publishCfRoute={publishCfRoute}
                  toggleCfRoute={toggleCfRoute}
                  removeCfRoute={removeCfRoute}
                  copyText={copyText}
                />
              ) : null}
            </div>
          </div>
          <div className="actions project-edit-actions">
            <Button type="button" variant="secondary" disabled={busy === "project" || projectEnv.loading || !projectForm.manualDeployEnabled} onClick={onSaveAndDeploy} icon={<Play size={15} />}>
              Save & Deploy
            </Button>
            <Button type="submit" disabled={busy === "project" || projectEnv.loading}>
              Save project
            </Button>
          </div>
        </form>
      </Modal>

      {projectEditorModal === "compose" ? (
        <Modal title="Compose editor" size="wide" onClose={() => setProjectEditorModal(null)}>
          <div className="editor-modal-body compose-section">
            <div className="editor-status-line">
              <StatusBadge status={projectCompose.source ?? "open"} />
              <span>{projectCompose.message || "Compose override"}</span>
            </div>
            <TextAreaField label="Compose content" value={projectForm.composeContent} onChange={(composeContent) => setProjectForm((c) => ({ ...c, composeContent }))} placeholder={"Optional. Paste docker-compose.yml content here for compose-only projects or to override the file during deploy."} />
          </div>
        </Modal>
      ) : null}

      {projectEditorModal === "env" ? (
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
    </>
  );
}

function CfRoutesSection({
  projectId, cfRoutes, cfRouteForm, setCfRouteForm, cfSettingsReady, busy,
  publishCfRoute, toggleCfRoute, removeCfRoute, copyText
}: {
  projectId: string;
  cfRoutes: CloudflareRoute[];
  cfRouteForm: CfRouteForm;
  setCfRouteForm: (fn: (current: CfRouteForm) => CfRouteForm) => void;
  cfSettingsReady: boolean;
  busy: string | null;
  publishCfRoute: (projectId: string) => void;
  toggleCfRoute: (route: CloudflareRoute) => void;
  removeCfRoute: (routeId: string) => void;
  copyText: (value: string) => Promise<void>;
}) {
  return (
    <section className="project-edit-section cf-routes-section">
      <div className="cf-route-head">
        <div>
          <div className="section-kicker">Cloudflare Tunnel</div>
          <p className="muted">{cfSettingsReady ? "Publish a public hostname to this project's local service." : "Set Cloudflare Tunnel settings first."}</p>
        </div>
        <StatusBadge status={cfSettingsReady ? (cfRoutes.some((r) => r.enabled) ? "connected" : "idle") : "disabled"} />
      </div>
      <div className="cf-route-list compact">
        {cfRoutes.map((route) => (
          <div key={route.id} className="cf-route-row">
            <div className="cf-route-info">
              <a className="cf-route-hostname" href={`https://${route.hostname}`} target="_blank" rel="noopener noreferrer">https://{route.hostname}</a>
              <span className="cf-route-service">{route.serviceTarget}{route.noTlsVerify ? " · no TLS verify" : ""}</span>
              <StatusBadge status={route.enabled ? "enabled" : "disabled"} />
            </div>
            <div className="cf-route-actions">
              <IconButton label="Copy URL" onClick={() => void copyText(`https://${route.hostname}`)}><Copy size={14} /></IconButton>
              <Button variant="ghost" disabled={!cfSettingsReady || busy === `cf-route-toggle:${route.id}`} onClick={() => void toggleCfRoute(route)}>{route.enabled ? "Disable" : "Enable"}</Button>
              <IconButton label="Delete route" disabled={!cfSettingsReady || busy === `cf-route-delete:${route.id}`} onClick={() => void removeCfRoute(route.id)}><Trash2 size={14} /></IconButton>
            </div>
          </div>
        ))}
        {!cfRoutes.length ? <p className="muted">No public hostnames configured.</p> : null}
      </div>
      <div className="cf-route-add-form compact">
        <div className="cf-route-add-row">
          <TextField label="Hostname" value={cfRouteForm.hostname} onChange={(hostname) => setCfRouteForm((c) => ({ ...c, hostname }))} placeholder="app.example.com" disabled={!cfSettingsReady} />
          <TextField
            label="Local service target"
            value={cfRouteForm.localTarget}
            onChange={(localTarget) => {
              const parsed = parseCfServiceTarget(localTarget);
              setCfRouteForm((c) => ({ ...c, protocol: parsed.protocol, localTarget: parsed.localTarget, noTlsVerify: parsed.protocol === "https" ? c.noTlsVerify : false }));
            }}
            placeholder="container-name:3000"
            disabled={!cfSettingsReady}
          />
          <CustomSelect<CfRouteProtocol>
            label="Protocol"
            value={cfRouteForm.protocol}
            options={[{ label: "HTTP", value: "http" }, { label: "HTTPS", value: "https" }]}
            onChange={(protocol) => setCfRouteForm((c) => ({ ...c, protocol, noTlsVerify: protocol === "https" ? c.noTlsVerify : false }))}
            disabled={!cfSettingsReady}
          />
          {cfRouteForm.protocol === "https" ? (
            <ToggleField label="No TLS verify" value={cfRouteForm.noTlsVerify} onChange={(noTlsVerify) => setCfRouteForm((c) => ({ ...c, noTlsVerify }))} disabled={!cfSettingsReady} />
          ) : null}
          <Button disabled={busy === "cf-route-publish" || !cfSettingsReady || !cfRouteForm.hostname || !cfRouteForm.localTarget} variant="secondary" onClick={() => void publishCfRoute(projectId)} icon={<Plus size={15} />}>Publish</Button>
        </div>
      </div>
    </section>
  );
}

export function CreatedProjectSecretModal({ secret, onClose, copyText }: {
  secret: CreatedProjectSecret;
  onClose: () => void;
  copyText: (value: string) => Promise<void>;
}) {
  return (
    <Modal title={`${secret.projectName} deploy token`} onClose={onClose}>
      <div className="form-grid compact-form">
        <label className="field"><span>Deploy URL</span><input value={secret.deployUrl} readOnly onFocus={(e) => e.currentTarget.select()} /></label>
        <label className="field"><span>GitHub webhook URL</span><input value={secret.webhookUrl} readOnly onFocus={(e) => e.currentTarget.select()} /></label>
        <label className="field"><span>Deploy token</span><input value={secret.deployToken} readOnly onFocus={(e) => e.currentTarget.select()} /></label>
        <div className="actions">
          <Button variant="secondary" onClick={() => void copyText(secret.deployUrl)} icon={<Copy size={15} />}>Deploy URL</Button>
          <Button variant="secondary" onClick={() => void copyText(secret.webhookUrl)} icon={<Copy size={15} />}>Webhook URL</Button>
          <Button onClick={() => void copyText(secret.deployToken)} icon={<Copy size={15} />}>Token</Button>
        </div>
      </div>
    </Modal>
  );
}

export function RollbackModal({ rollbackModal, onClose, onRollback }: {
  rollbackModal: { project: Project; deployments: import("../../shared/types").Deployment[] };
  onClose: () => void;
  onRollback: (project: Project, deploymentId: string) => void;
}) {
  return (
    <Modal title={`Rollback ${rollbackModal.project.name}`} onClose={onClose}>
      <div className="rollback-list">
        {rollbackModal.deployments.map((deployment) => (
          <button type="button" key={deployment.id} onClick={() => onRollback(rollbackModal.project, deployment.id)}>
            <span>{dateTime(deployment.startedAt)}</span>
            <strong>{durationBetween(deployment.startedAt, deployment.finishedAt)}</strong>
            <StatusBadge status={deployment.status} />
          </button>
        ))}
        {!rollbackModal.deployments.length ? <p className="muted">No successful deployments are available for rollback yet.</p> : null}
      </div>
    </Modal>
  );
}

export function SetupWizardModal({ settings, sshReady, cfSettingsReady, r2Ready, setupStep, setupCanGoBack, setupCanGoNext, busy, sshPrivateKey, setSshPrivateKey, r2Form, cfForm, updateR2Form, updateCfForm, saveSshPrivateKey, saveCfSettings, saveR2Settings, validateCfSettings, goToNextSetupStep, goToPreviousSetupStep, saveSetupWizard, onClose, setSetupStep }: {
  settings: any;
  sshReady: boolean;
  cfSettingsReady: boolean;
  r2Ready: boolean;
  setupStep: string;
  setupCanGoBack: boolean;
  setupCanGoNext: boolean;
  busy: string | null;
  sshPrivateKey: string;
  setSshPrivateKey: (value: string) => void;
  r2Form: any;
  cfForm: any;
  updateR2Form: (patch: any) => void;
  updateCfForm: (patch: any) => void;
  saveSshPrivateKey: (event: React.FormEvent) => void;
  saveCfSettings: (event: React.FormEvent) => void;
  saveR2Settings: (event: React.FormEvent) => void;
  validateCfSettings: () => void;
  goToNextSetupStep: () => void;
  goToPreviousSetupStep: () => void;
  saveSetupWizard: (action: "completed" | "dismissed") => void;
  onClose: () => void;
  setSetupStep: (step: any) => void;
}) {
  const steps = ["intro", "ssh", "cloudflare", "r2"];
  const stepIndex = steps.indexOf(setupStep);

  return (
    <Modal title="Quick setup" onClose={onClose}>
      <div className="setup-wizard">
        <div className="setup-progress" aria-label="Setup progress">
          {steps.map((step, index) => (
            <button key={step} type="button" className={setupStep === step ? "active" : index < stepIndex ? "done" : ""} onClick={() => setSetupStep(step)} aria-label={`Go to ${step}`}>
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
              <div><KeyRound size={16} /><span>Git SSH key</span><StatusBadge status={sshReady ? "ready" : "optional"} /></div>
              <div><ShieldCheck size={16} /><span>Cloudflare Tunnel</span><StatusBadge status={cfSettingsReady ? "ready" : "optional"} /></div>
              <div><Cloud size={16} /><span>Cloudflare R2</span><StatusBadge status={r2Ready ? "ready" : "optional"} /></div>
            </div>
          </div>
        ) : null}

        {setupStep === "ssh" ? (
          <form className="setup-step form-grid ssh-key-form" onSubmit={saveSshPrivateKey}>
            <div className="setup-status-row"><span>Active key</span><StatusBadge status={sshReady ? "ready" : "optional"} /></div>
            <dl className="settings-list ssh-status-list">
              <div><dt>Active key path</dt><dd>{settings.sshKey?.activePrivateKeyPath ?? "No key found"}</dd></div>
              <div><dt>Mounted VPS key</dt><dd>{settings.sshKey?.hasMountedKey ? settings.sshKey.mountedPrivateKeyPath : "Not found"}</dd></div>
            </dl>
            <TextAreaField label="Private key" value={sshPrivateKey} onChange={setSshPrivateKey} placeholder={"Paste the full private key, starting with -----BEGIN OPENSSH PRIVATE KEY-----"} />
            <div className="actions"><Button type="submit" disabled={busy === "ssh-key" || !sshPrivateKey.trim()} icon={<KeyRound size={16} />}>Save SSH key</Button></div>
          </form>
        ) : null}

        {setupStep === "cloudflare" ? (
          <form className="setup-step form-grid compact-form" onSubmit={saveCfSettings} autoComplete="off">
            <div className="setup-status-row"><span>Tunnel settings</span><StatusBadge status={cfSettingsReady ? "ready" : "optional"} /></div>
            <div className="cf-token-requirements"><span>Token permissions</span><ul><li>Account / Cloudflare Tunnel / Edit</li><li>Account / Account Settings / Read</li><li>Zone / Zone / Read</li><li>Zone / DNS / Edit</li></ul></div>
            <div className="settings-form-pair">
              <TextField label="Account ID" value={cfForm.accountId} onChange={(accountId) => updateCfForm({ accountId })} />
              <TextField label="Zone ID" value={cfForm.zoneId} onChange={(zoneId) => updateCfForm({ zoneId })} />
            </div>
            <TextField label="API Token" type="password" value={cfForm.apiToken} onChange={(apiToken) => updateCfForm({ apiToken })} placeholder={settings.cf?.hasApiToken ? "Saved; leave blank to keep" : ""} autoComplete="new-password" />
            <div className="actions">
              <Button variant="secondary" disabled={busy === "cf-validate"} onClick={() => void validateCfSettings()}>Validate</Button>
              <Button type="submit" disabled={busy === "cf-settings"} icon={<ShieldCheck size={16} />}>Save</Button>
            </div>
          </form>
        ) : null}

        {setupStep === "r2" ? (
          <form className="setup-step form-grid compact-form" onSubmit={saveR2Settings} autoComplete="off">
            <div className="setup-status-row"><span>R2 uploads</span><StatusBadge status={r2Ready ? "ready" : "optional"} /></div>
            <ToggleField label="Upload enabled" value={r2Form.enabled} onChange={(enabled: boolean) => updateR2Form({ enabled })} description={settings.r2?.hasSecretAccessKey ? "Secret key saved" : "Add an R2 secret key before uploading"} />
            <div className="settings-form-pair">
              <TextField label="Account ID" value={r2Form.accountId} onChange={(accountId: string) => updateR2Form({ accountId })} autoComplete="off" />
              <TextField label="Bucket" value={r2Form.bucket} onChange={(bucket: string) => updateR2Form({ bucket })} autoComplete="off" />
            </div>
            <div className="settings-form-pair">
              <TextField label="Access key ID" value={r2Form.accessKeyId} onChange={(accessKeyId: string) => updateR2Form({ accessKeyId })} placeholder={settings.r2?.maskedAccessKeyId ? `${settings.r2.maskedAccessKeyId}; leave blank to keep` : ""} autoComplete="off" />
              <TextField label="Secret access key" type="password" value={r2Form.secretAccessKey} onChange={(secretAccessKey: string) => updateR2Form({ secretAccessKey })} placeholder={settings.r2?.hasSecretAccessKey ? "Saved; leave blank to keep" : ""} autoComplete="new-password" />
            </div>
            <TextField label="Object prefix" value={r2Form.prefix} onChange={(prefix: string) => updateR2Form({ prefix })} autoComplete="off" />
            <div className="actions"><Button type="submit" disabled={busy === "r2-settings"} icon={<Cloud size={16} />}>Save R2</Button></div>
          </form>
        ) : null}

        <div className="setup-actions">
          <Button variant="ghost" disabled={!!busy?.startsWith("setup-")} onClick={() => void saveSetupWizard("dismissed")}>Skip</Button>
          <div className="actions">
            <Button variant="secondary" disabled={!setupCanGoBack} onClick={goToPreviousSetupStep} icon={<ChevronLeft size={16} />}>Back</Button>
            {setupCanGoNext ? (
              <Button onClick={goToNextSetupStep} icon={<ChevronRight size={16} />}>Next</Button>
            ) : (
              <Button disabled={!!busy?.startsWith("setup-")} onClick={() => void saveSetupWizard("completed")} icon={<ShieldCheck size={16} />}>Finish</Button>
            )}
          </div>
        </div>
      </div>
    </Modal>
  );
}
