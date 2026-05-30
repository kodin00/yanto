import type { CloudflarePublicSettings, R2PublicSettings, SetupWizardStatus } from "../shared/types";

export type View = "dashboard" | "projects" | "deployments" | "containers" | "nodes" | "backups" | "audit" | "settings";
export type ToastState = { message: string; kind?: "ok" | "error" | "loading" } | null;
export type ConfirmState = { title: string; body: string; label: string; danger?: boolean; loadingMessage?: string; successMessage?: string; action: () => Promise<void> };
export type LogModalState = { title: string; logs: string; streamPath?: string; live?: boolean; status?: string };
export type LogStreamPayload = { logs?: string; chunk?: string; status?: string; error?: string; done?: boolean };
export type ThemeMode = "light" | "dark";

export type CfRouteProtocol = "http" | "https";
export type CfRouteForm = { hostname: string; protocol: CfRouteProtocol; localTarget: string; noTlsVerify: boolean };
export type ProjectComposeState = { open: boolean; loading: boolean; available: boolean; source: "saved" | "file" | "empty" | null; message: string };
export type RollbackModalState = { project: import("../shared/types").Project; deployments: import("../shared/types").Deployment[] };
export type CreatedProjectSecret = { projectName: string; deployUrl: string; webhookUrl: string; deployToken: string };
export type SetupStep = "intro" | "ssh" | "cloudflare" | "r2";

export type SettingsState = {
  projectsRoot: string;
  hostProjectsRoot: string;
  sshKeysDir: string;
  appBaseUrl: string;
  sshKey: {
    hasManagedKey: boolean;
    hasMountedKey: boolean;
    managedPrivateKeyPath: string;
    mountedPrivateKeyPath: string;
    activePrivateKeyPath: string | null;
    publicKey: string | null;
  };
  r2: R2PublicSettings;
  cf: CloudflarePublicSettings;
  setupWizard: SetupWizardStatus;
};

export type R2FormState = {
  enabled: boolean;
  accountId: string;
  bucket: string;
  accessKeyId: string;
  secretAccessKey: string;
  prefix: string;
};

export type CfFormState = {
  accountId: string;
  zoneId: string;
  apiToken: string;
};

export const emptyProject = {
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

export const emptySshKeySettings = {
  hasManagedKey: false,
  hasMountedKey: false,
  managedPrivateKeyPath: "/data/ssh/id_ed25519",
  mountedPrivateKeyPath: "/root/.ssh/id_ed25519",
  activePrivateKeyPath: null as string | null,
  publicKey: null as string | null
};

export const emptyR2Settings: R2PublicSettings = {
  enabled: false,
  accountId: "",
  bucket: "",
  maskedAccessKeyId: "",
  hasAccessKeyId: false,
  hasSecretAccessKey: false,
  prefix: "postgres-dumps"
};

export const emptyCfSettings: CloudflarePublicSettings = {
  accountId: "",
  zoneId: "",
  hasApiToken: false
};

export const emptySetupWizardStatus: SetupWizardStatus = {
  completedAt: null,
  dismissedAt: null,
  updatedAt: null
};

export const emptySettingsState: SettingsState = {
  projectsRoot: "/projects",
  hostProjectsRoot: "~/projects",
  sshKeysDir: "",
  appBaseUrl: "",
  sshKey: emptySshKeySettings,
  r2: emptyR2Settings,
  cf: emptyCfSettings,
  setupWizard: emptySetupWizardStatus
};

export const setupSteps: SetupStep[] = ["intro", "ssh", "cloudflare", "r2"];

export const emptyProjectComposeState: ProjectComposeState = {
  open: false,
  loading: false,
  available: true,
  source: null,
  message: ""
};

export type ProjectFormState = typeof emptyProject;

export function parseCfServiceTarget(serviceTarget: string): Pick<CfRouteForm, "protocol" | "localTarget"> {
  const match = serviceTarget.trim().match(/^(https?):\/\/(.+)$/);
  if (!match) return { protocol: "http", localTarget: serviceTarget.trim() };
  return { protocol: match[1] as CfRouteProtocol, localTarget: match[2] };
}

export function buildCfRouteForm(hostname = "", serviceTarget = "", noTlsVerify = false): CfRouteForm {
  const parsed = parseCfServiceTarget(serviceTarget);
  return {
    hostname,
    protocol: parsed.protocol,
    localTarget: parsed.localTarget,
    noTlsVerify: parsed.protocol === "https" ? noTlsVerify : false
  };
}

export function cfRouteServiceTarget(form: CfRouteForm) {
  const parsed = parseCfServiceTarget(form.localTarget);
  return `${parsed.protocol === "https" ? "https" : form.protocol}://${parsed.localTarget}`;
}

export const themeStorageKey = "yanto-theme";

export function getInitialTheme(): ThemeMode {
  if (typeof window === "undefined") return "light";
  const storedTheme = window.localStorage.getItem(themeStorageKey);
  if (storedTheme === "light" || storedTheme === "dark") return storedTheme;
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}
