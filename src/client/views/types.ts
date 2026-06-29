import type { CloudflarePublicSettings, MultiNodePublicSettings, R2PublicSettings, SetupWizardStatus } from "../../shared/types";
import type { SshKeyStatus } from "../lib/api";

export type View = "dashboard" | "projects" | "deployments" | "containers" | "nodes" | "backups" | "hostnames" | "frp" | "dns" | "audit" | "settings";
export type ToastState = { message: string; kind?: "ok" | "error" | "loading" } | null;
export type ConfirmState = { title: string; body: string; label: string; danger?: boolean; loadingMessage?: string; successMessage?: string; action: () => Promise<void> };

export type SettingsState = {
  projectsRoot: string;
  hostProjectsRoot: string;
  sshKeysDir: string;
  appBaseUrl: string;
  sshKey: SshKeyStatus;
  r2: R2PublicSettings;
  cf: CloudflarePublicSettings;
  setupWizard: SetupWizardStatus;
  multiNode: MultiNodePublicSettings;
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
