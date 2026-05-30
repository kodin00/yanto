import { useCallback, useEffect, useState } from "react";
import type { FormEvent } from "react";
import { api } from "../lib/api";
import { useToast } from "../contexts/ToastContext";
import { emptySettingsState, type CfFormState, type R2FormState, type SettingsState, type SetupStep, setupSteps } from "../types";

export function useSettings() {
  const [settings, setSettings] = useState<SettingsState>(emptySettingsState);
  const [settingsLoaded, setSettingsLoaded] = useState(false);
  const [r2Form, setR2Form] = useState<R2FormState>({ enabled: false, accountId: "", bucket: "", accessKeyId: "", secretAccessKey: "", prefix: "postgres-dumps" });
  const [r2FormDirty, setR2FormDirty] = useState(false);
  const [cfForm, setCfForm] = useState<CfFormState>({ accountId: "", zoneId: "", apiToken: "" });
  const [cfFormDirty, setCfFormDirty] = useState(false);
  const [systemLogs, setSystemLogs] = useState("");
  const [cleanupLogs, setCleanupLogs] = useState("");
  const [cleanupLogTitle, setCleanupLogTitle] = useState("Cleanup preview");
  const [cleanupPreviewed, setCleanupPreviewed] = useState(false);
  const [sshPrivateKey, setSshPrivateKey] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [setupModalOpen, setSetupModalOpen] = useState(false);
  const [setupStep, setSetupStep] = useState<SetupStep>("intro");
  const [setupAutoPrompted, setSetupAutoPrompted] = useState(false);
  const { showToast } = useToast();

  const sshReady = Boolean(settings.sshKey?.activePrivateKeyPath);
  const r2Ready = Boolean(settings.r2?.enabled && settings.r2.accountId && settings.r2.bucket && settings.r2.hasAccessKeyId && settings.r2.hasSecretAccessKey);
  const cfSettingsReady = Boolean(settings.cf?.accountId && settings.cf.zoneId && settings.cf.hasApiToken);
  const setupStepIndex = setupSteps.indexOf(setupStep);
  const setupCanGoBack = setupStepIndex > 0;
  const setupCanGoNext = setupStepIndex < setupSteps.length - 1;
  const setupCanReopen = settingsLoaded && !setupModalOpen && !settings.setupWizard.completedAt && Boolean(settings.setupWizard.dismissedAt);

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
    if (!settingsLoaded || setupAutoPrompted) return;
    if (settings.setupWizard.completedAt || settings.setupWizard.dismissedAt) return;
    setSetupStep("intro");
    setSetupModalOpen(true);
    setSetupAutoPrompted(true);
  }, [settings.setupWizard.completedAt, settings.setupWizard.dismissedAt, settingsLoaded, setupAutoPrompted]);

  const refreshSettings = useCallback(async () => {
    const settingRows = await api.settings().catch(() => null);
    if (settingRows) {
      setSettings(settingRows);
      setSettingsLoaded(true);
    }
  }, []);

  const setSettingsFromPayload = useCallback((payload: SettingsState) => {
    setSettings(payload);
    setSettingsLoaded(true);
  }, []);

  function updateR2Form(patch: Partial<R2FormState>) {
    setR2FormDirty(true);
    setR2Form((current) => ({ ...current, ...patch }));
  }

  function updateCfForm(patch: Partial<CfFormState>) {
    setCfFormDirty(true);
    setCfForm((current) => ({ ...current, ...patch }));
  }

  async function saveR2Settings(event: FormEvent) {
    event.preventDefault();
    setBusy("r2-settings");
    showToast("Saving R2 settings...", "loading");
    try {
      const result = await api.saveR2Settings(r2Form);
      setR2Form({ enabled: result.r2.enabled, accountId: result.r2.accountId, bucket: result.r2.bucket, accessKeyId: "", secretAccessKey: "", prefix: result.r2.prefix });
      setR2FormDirty(false);
      setSettings((current) => ({ ...current, r2: result.r2 }));
      showToast("R2 settings saved.");
    } catch (error) {
      showToast(error instanceof Error ? error.message : "Unable to save R2 settings.", "error");
    } finally {
      setBusy(null);
    }
  }

  async function saveCfSettings(event: FormEvent) {
    event.preventDefault();
    setBusy("cf-settings");
    showToast("Saving Cloudflare settings...", "loading");
    try {
      const result = await api.saveCloudflareSettings(cfForm);
      setCfForm({ accountId: result.cf.accountId, zoneId: result.cf.zoneId, apiToken: "" });
      setCfFormDirty(false);
      setSettings((current) => ({ ...current, cf: result.cf }));
      showToast("Cloudflare settings saved.");
    } catch (error) {
      showToast(error instanceof Error ? error.message : "Unable to save Cloudflare settings.", "error");
    } finally {
      setBusy(null);
    }
  }

  async function validateCfSettings() {
    setBusy("cf-validate");
    showToast("Validating Cloudflare credentials...", "loading");
    try {
      const result = await api.validateCloudflareSettings(cfForm);
      showToast(`Validated. Account: ${result.accountName}${result.zoneName ? `, Zone: ${result.zoneName}` : ""}`);
    } catch (error) {
      showToast(error instanceof Error ? error.message : "Validation failed.", "error");
    } finally {
      setBusy(null);
    }
  }

  async function saveSshPrivateKey(event: FormEvent) {
    event.preventDefault();
    setBusy("ssh-key");
    showToast("Saving SSH key...", "loading");
    try {
      await api.saveSshKey(sshPrivateKey);
      setSshPrivateKey("");
      await refreshSettings();
      showToast("SSH key saved.");
    } catch (error) {
      showToast(error instanceof Error ? error.message : "Unable to save SSH key.", "error");
    } finally {
      setBusy(null);
    }
  }

  async function previewCleanup() {
    setBusy("cleanup-preview");
    showToast("Checking cleanup preview...", "loading");
    setCleanupLogTitle("Cleanup preview");
    setCleanupLogs("Checking reclaimable Docker space...");
    try {
      const result = await api.cleanupPreview();
      setCleanupPreviewed(true);
      setCleanupLogs(result.logs);
      showToast("Cleanup preview ready.");
    } catch (error) {
      setCleanupPreviewed(false);
      setCleanupLogs(error instanceof Error ? error.message : "Unable to preview cleanup.");
      showToast(error instanceof Error ? error.message : "Unable to preview cleanup.", "error");
    } finally {
      setBusy(null);
    }
  }

  async function refreshSystemLogs() {
    setBusy("system-logs");
    showToast("Refreshing system log...", "loading");
    try {
      setSystemLogs(await api.systemLogs());
      showToast("System log refreshed.");
    } catch (error) {
      showToast(error instanceof Error ? error.message : "Unable to refresh system log.", "error");
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
      showToast(action === "completed" ? "Setup finished." : "Setup skipped. You can reopen it from Settings.");
    } catch (error) {
      showToast(error instanceof Error ? error.message : "Unable to update setup status.", "error");
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

  async function copyWorkerInstallCommand() {
    showToast("Creating worker install command...", "loading");
    try {
      const result = await api.workerJoinToken();
      await navigator.clipboard.writeText(result.command);
      showToast("Copied.");
    } catch (error) {
      showToast(error instanceof Error ? error.message : "Unable to load worker command.", "error");
    }
  }

  return {
    settings, setSettings, settingsLoaded, setSettingsLoaded,
    r2Form, cfForm, busy, setBusy, sshPrivateKey, setSshPrivateKey,
    systemLogs, cleanupLogs, cleanupLogTitle, cleanupPreviewed, setCleanupLogs, setCleanupLogTitle, setCleanupPreviewed,
    sshReady, r2Ready, cfSettingsReady,
    setupModalOpen, setupStep, setSetupStep, setupCanGoBack, setupCanGoNext, setupCanReopen,
    refreshSettings, setSettingsFromPayload, updateR2Form, updateCfForm,
    saveR2Settings, saveCfSettings, validateCfSettings, saveSshPrivateKey,
    previewCleanup, refreshSystemLogs,
    openSetupWizard, goToNextSetupStep, goToPreviousSetupStep, saveSetupWizard, closeSetupWizard,
    copyWorkerInstallCommand
  };
}
