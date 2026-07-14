import { Check, Cloud, Copy, DatabaseZap, GitBranch, GitPullRequest, KeyRound, RefreshCw, ScrollText, Server, Settings, ShieldCheck, Trash2 } from "lucide-react";
import { memo } from "react";
import type { FormEvent } from "react";
import { Button, CustomSelect, LogViewer, StatusBadge, TextAreaField, TextField, ToggleField } from "../components/ui";
import { UserAccessPanel } from "../components/UserAccessPanel";
import { api } from "../lib/api";
import type { McpAccessLevel, McpAccessToken, Project, SessionUser } from "../../shared/types";
import type { CfFormState, ConfirmState, CreatedMcpTokenState, McpTokenFormState, R2FormState, SettingsState } from "./types";

type Props = {
  session: SessionUser;
  projects: Project[];
  settings: SettingsState;
  mcpTokens: McpAccessToken[];
  mcpTokenForm: McpTokenFormState;
  createdMcpToken: CreatedMcpTokenState;
  r2Form: R2FormState;
  cfForm: CfFormState;
  busy: string | null;
  sshPrivateKey: string;
  systemLogs: string;
  cleanupLogs: string;
  cleanupLogTitle: string;
  cleanupPreviewed: boolean;
  updateR2Form: (patch: Partial<R2FormState>) => void;
  updateCfForm: (patch: Partial<CfFormState>) => void;
  updateMcpTokenForm: (patch: Partial<McpTokenFormState>) => void;
  saveR2Settings: (event: FormEvent) => void;
  saveCfSettings: (event: FormEvent) => void;
  createMcpToken: (event: FormEvent) => void;
  revokeMcpToken: (token: McpAccessToken) => void;
  dismissCreatedMcpToken: () => void;
  saveMultiNodeSettings: (enabled: boolean) => void;
  validateCfSettings: () => void;
  saveSshPrivateKey: (event: FormEvent) => void;
  generateSshPrivateKey: () => void;
  setSshPrivateKey: (value: string) => void;
  copyText: (value: string) => Promise<void>;
  copyWorkerInstallCommand: () => void;
  openSetupWizard: () => void;
  previewCleanup: () => void;
  refreshSystemLogs: () => void;
  refreshContainers: () => Promise<void>;
  setConfirm: (state: ConfirmState) => void;
  setBusy: (value: string | null) => void;
  setCleanupLogTitle: (value: string) => void;
  setCleanupLogs: (value: string) => void;
  setCleanupPreviewed: (value: boolean) => void;
};

export const SettingsView = memo(function SettingsView(props: Props) {
  const {
    session,
    projects,
    settings,
    mcpTokens,
    mcpTokenForm,
    createdMcpToken,
    r2Form,
    cfForm,
    busy,
    sshPrivateKey,
    systemLogs,
    cleanupLogs,
    cleanupLogTitle,
    cleanupPreviewed,
    updateR2Form,
    updateCfForm,
    updateMcpTokenForm,
    saveR2Settings,
    saveCfSettings,
    createMcpToken,
    revokeMcpToken,
    dismissCreatedMcpToken,
    saveMultiNodeSettings,
    validateCfSettings,
    saveSshPrivateKey,
    generateSshPrivateKey,
    setSshPrivateKey,
    copyText,
    copyWorkerInstallCommand,
    openSetupWizard,
    previewCleanup,
    refreshSystemLogs,
    refreshContainers,
    setConfirm,
    setBusy,
    setCleanupLogTitle,
    setCleanupLogs,
    setCleanupPreviewed,
  } = props;

  return (
    <section className="settings-grid">
      <div className="settings-column settings-column-wide">
        <UserAccessPanel projects={projects} currentUserId={session.id} copyText={copyText} />
      </div>
      <div className="settings-column">
        <section className="panel r2-settings-panel">
          <div className="panel-head">
            <div className="panel-head-title">
              <span className="settings-panel-icon"><Cloud size={16} /></span>
              <h2>Cloudflare R2</h2>
            </div>
          </div>
          <form className="form-grid compact-form" onSubmit={saveR2Settings} autoComplete="off">
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
        </section>

        <section className="panel runtime-settings-panel">
          <div className="panel-head">
            <div className="panel-head-title">
              <span className="settings-panel-icon"><Settings size={16} /></span>
              <h2>Runtime</h2>
            </div>
            <div className="runtime-head-actions">
              <StatusBadge status={settings.multiNode.releaseStage} label="Beta" />
              <Button variant="secondary" onClick={() => openSetupWizard()} icon={<Settings size={16} />}>
                Setup
              </Button>
            </div>
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
              <dt>Base URL</dt>
              <dd>{settings.appBaseUrl}</dd>
            </div>
          </dl>
          <ToggleField
            label="Multi-node"
            value={settings.multiNode.enabled}
            onChange={saveMultiNodeSettings}
            description="Opt in to worker nodes, node targeting, and worker install commands."
            disabled={busy === "multi-node-settings"}
          />
        </section>

        <section className="panel mcp-settings-panel">
          <div className="panel-head">
            <div className="panel-head-title">
              <span className="settings-panel-icon"><KeyRound size={16} /></span>
              <h2>MCP access</h2>
            </div>
          </div>
          <p className="muted">Create bearer tokens for AI clients. Tokens authorize `/mcp` and local stdio only; token administration stays dashboard-only.</p>
          {createdMcpToken ? (
            <div className="created-secret-box">
              <div>
                <strong>Copy this token now</strong>
                <p className="muted">Yanto stores only a hash and will not show the secret again.</p>
              </div>
              <div className="token-box">
                <span>{createdMcpToken.token}</span>
                <button type="button" onClick={() => void copyText(createdMcpToken.token)} title="Copy MCP token" aria-label="Copy MCP token">
                  <Copy size={15} />
                </button>
              </div>
              <Button type="button" variant="secondary" onClick={dismissCreatedMcpToken}>
                I copied it
              </Button>
            </div>
          ) : null}
          <form className="form-grid compact-form" onSubmit={createMcpToken} autoComplete="off">
            <div className="settings-form-pair">
              <TextField label="Token name" value={mcpTokenForm.name} onChange={(name) => updateMcpTokenForm({ name })} placeholder="Codex local" autoComplete="off" />
              <CustomSelect<McpAccessLevel> label="Access level" value={mcpTokenForm.accessLevel} options={[{ label: "Read", value: "read" }, { label: "Write", value: "write" }, { label: "Admin", value: "admin" }]} onChange={(accessLevel) => updateMcpTokenForm({ accessLevel })} />
            </div>
            <p className="muted">Scopes are hierarchical: write includes read; admin includes both. Destructive and secret-reveal MCP tools still require <code>confirm: true</code>.</p>
            <div className="actions">
              <Button type="submit" disabled={busy === "mcp-token-create" || !mcpTokenForm.name.trim()} icon={<KeyRound size={16} />}>
                Create MCP token
              </Button>
            </div>
          </form>
          <div className="token-table">
            {mcpTokens.length ? (
              mcpTokens.map((token) => (
                <div className="token-row" key={token.id}>
                  <div>
                    <strong>{token.name}</strong>
                    <p className="muted">
                      {token.accessLevel} · created {new Date(token.createdAt).toLocaleString()}
                      {token.lastUsedAt ? ` · last used ${new Date(token.lastUsedAt).toLocaleString()}` : ""}
                    </p>
                  </div>
                  <div className="actions">
                    <StatusBadge status={token.revokedAt ? "revoked" : token.accessLevel} />
                    <Button type="button" variant="danger" disabled={Boolean(token.revokedAt) || busy === `mcp-token:${token.id}`} onClick={() => revokeMcpToken(token)} icon={<Trash2 size={16} />}>
                      Revoke
                    </Button>
                  </div>
                </div>
              ))
            ) : (
              <p className="muted">No MCP tokens yet.</p>
            )}
          </div>
        </section>

        <section className="panel webhook-settings compact-settings-panel">
          <div className="panel-head">
            <div className="panel-head-title">
              <span className="settings-panel-icon"><GitBranch size={16} /></span>
              <h2>Deployment webhook</h2>
            </div>
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

        {settings.multiNode.enabled ? (
          <section className="panel webhook-settings compact-settings-panel">
            <div className="panel-head">
              <div className="panel-head-title">
                <span className="settings-panel-icon"><Server size={16} /></span>
                <h2>Worker install</h2>
              </div>
            </div>
            <Button variant="secondary" onClick={() => void copyWorkerInstallCommand()} icon={<Copy size={16} />}>
              Copy worker command
            </Button>
          </section>
        ) : null}

        <section className="panel cleanup-settings-panel">
          <div className="panel-head">
            <div className="panel-head-title">
              <span className="settings-panel-icon"><DatabaseZap size={16} /></span>
              <h2>Cleanup</h2>
            </div>
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
                  return;
                }
                setConfirm({
                  title: "Run cleanup",
                  body: "This removes unused Docker cache and unused Docker resources shown by the preview. Running containers, named volumes, and Yanto containers are protected.",
                  label: "Clean cache",
                  danger: true,
                  loadingMessage: "Cleaning Docker cache...",
                  successMessage: "Cleanup completed.",
                  action: async () => {
                    setBusy("cleanup");
                    setCleanupLogTitle("Cleanup logs");
                    setCleanupLogs("Cleaning unused Docker cache and resources...");
                    try {
                      const result = await api.cleanup();
                      setCleanupPreviewed(false);
                      setCleanupLogs(result.logs);
                      await refreshContainers();
                    } finally {
                      setBusy(null);
                    }
                  },
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
      </div>

      <div className="settings-column">
        <section className="panel cf-tunnel-settings-panel">
          <div className="panel-head">
            <div className="panel-head-title">
              <span className="settings-panel-icon"><ShieldCheck size={16} /></span>
              <h2>Cloudflare Tunnel</h2>
            </div>
          </div>
          <form className="form-grid compact-form" onSubmit={saveCfSettings} autoComplete="off">
            <div className="cf-help">
              <div>
                <strong>Where to find these values</strong>
                <p>Open Cloudflare Dashboard, choose your account and zone, then copy Account ID and Zone ID from the zone overview. Create a custom API token from the API Tokens page and use the scoped permissions below.</p>
              </div>
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
            <div className={`credential-status ${settings.cf?.hasApiToken ? "saved" : ""}`}>
              {settings.cf?.hasApiToken ? <Check size={15} /> : <ShieldCheck size={15} />}
              <span>{settings.cf?.hasApiToken ? "API token saved" : "API token not saved"}</span>
            </div>
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
            <div className="panel-head-title">
              <span className="settings-panel-icon"><KeyRound size={16} /></span>
              <h2>Git SSH key</h2>
            </div>
          </div>
          <div className={`credential-status ${settings.sshKey?.activePrivateKeyPath ? "saved" : ""}`}>
            {settings.sshKey?.activePrivateKeyPath ? <Check size={15} /> : <KeyRound size={15} />}
            <span>{settings.sshKey?.activePrivateKeyPath ? "SSH key saved" : "SSH key not saved"}</span>
          </div>
          <dl className="settings-list ssh-status-list">
            <div>
              <dt>Saved key</dt>
              <dd>{settings.sshKey?.hasManagedKey ? "Saved in app volume" : "Not saved"}</dd>
            </div>
            <div>
              <dt>Git access</dt>
              <dd>{settings.sshKey?.activePrivateKeyPath ? "Ready" : "Add or generate a key"}</dd>
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
              <Button type="button" variant="secondary" disabled={busy === "ssh-key-generate" || settings.sshKey?.hasManagedKey} onClick={() => void generateSshPrivateKey()} icon={<GitPullRequest size={16} />}>
                Generate key
              </Button>
              <Button type="submit" disabled={busy === "ssh-key" || !sshPrivateKey.trim()} icon={<KeyRound size={16} />}>
                Save SSH key
              </Button>
            </div>
          </form>
        </section>

        <section className="panel system-log-panel">
          <div className="panel-head">
            <div className="panel-head-title">
              <span className="settings-panel-icon"><ScrollText size={16} /></span>
              <h2>System log</h2>
            </div>
            <Button variant="secondary" disabled={busy === "system-logs"} onClick={() => void refreshSystemLogs()} icon={<RefreshCw size={16} className={busy === "system-logs" ? "spin" : ""} />}>
              {busy === "system-logs" ? "Reloading log" : "Reload log"}
            </Button>
          </div>
          <LogViewer logs={systemLogs || "No system log entries recorded yet."} />
        </section>
      </div>
    </section>
  );
});
