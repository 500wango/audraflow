import { useCallback, useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import type {
  AsrEngine,
  DiagnosticsPreview,
  GlossaryEntry,
  ModelActionResult,
  ModelCatalogEntry,
  ModelDownloadProgressEvent,
  ModelInfo,
  ModelSettings,
  PrivacyActionResult,
  RuntimeComponent,
  RuntimeComponentActionResult,
  RuntimeComponentProgressEvent,
  RuntimeDependency,
  RuntimeHealth,
  RuntimeRepairResult,
  TelemetryConsentState,
} from '../types';
import {
  formatFileSize,
  runtimeComponentLabel,
  runtimeDependencyLabel,
} from '../appUtils';
import { errorToMessage, hasTauriRuntime, invokeTauri } from '../tauri';
import { useI18n } from '../useI18n';
import { SettingsContent } from './settings/SettingsContent';

export function SettingsPage({
  asrEngine,
  telemetryConsent,
  telemetryStatus,
  onTelemetryConsentChange,
  modelSettings,
  modelSettingsError,
  onModelSettingsChange,
  onRefreshModelSettings,
}: {
  asrEngine: AsrEngine;
  telemetryConsent: TelemetryConsentState;
  telemetryStatus: string | null;
  onTelemetryConsentChange: (enabled: boolean) => Promise<TelemetryConsentState>;
  modelSettings: ModelSettings | null;
  modelSettingsError: string | null;
  onModelSettingsChange: (settings: ModelSettings | null) => void;
  onRefreshModelSettings: () => void;
}) {
  const { t, language, setLanguage } = useI18n();
  const [licenseInfo, setLicenseInfo] = useState<{
    state: string;
    isUsable: boolean;
    trialDaysRemaining: number;
    modelUpdatesUntil?: string;
  } | null>(null);
  const [licenseKeyInput, setLicenseKeyInput] = useState('');
  const [licenseAction, setLicenseAction] = useState(false);
  const [statusMsg, setStatusMsg] = useState('');
  const [modelStatusMsg, setModelStatusMsg] = useState('');
  const [telemetrySaving, setTelemetrySaving] = useState(false);
  const [diagnosticsPreview, setDiagnosticsPreview] = useState<DiagnosticsPreview | null>(null);
  const [runtimeHealth, setRuntimeHealth] = useState<RuntimeHealth | null>(null);
  const [runtimeHealthStatus, setRuntimeHealthStatus] = useState<string | null>(null);
  const [runtimeHealthStatusTargetId, setRuntimeHealthStatusTargetId] = useState<string | null>(null);
  const [runtimeHealthRefreshing, setRuntimeHealthRefreshing] = useState(false);
  const [runtimeRepairAction, setRuntimeRepairAction] = useState<string | null>(null);
  const [runtimeComponents, setRuntimeComponents] = useState<RuntimeComponent[]>([]);
  const [runtimeComponentStatus, setRuntimeComponentStatus] = useState<string | null>(null);
  const [runtimeComponentStatusTargetId, setRuntimeComponentStatusTargetId] = useState<string | null>(null);
  const [runtimeComponentRefreshing, setRuntimeComponentRefreshing] = useState(false);
  const [runtimeComponentAction, setRuntimeComponentAction] = useState<string | null>(null);
  const [runtimeComponentProgress, setRuntimeComponentProgress] = useState<RuntimeComponentProgressEvent | null>(null);
  const [privacyAction, setPrivacyAction] = useState<string | null>(null);
  const [modelPathInput, setModelPathInput] = useState('');
  const [modelDownloadUrl, setModelDownloadUrl] = useState('');
  const [modelDownloadSha256, setModelDownloadSha256] = useState('');
  const [modelDownloadSizeBytes, setModelDownloadSizeBytes] = useState('');
  const [modelDownloadName, setModelDownloadName] = useState('');
  const [modelDownloadVersion, setModelDownloadVersion] = useState('');
  const [modelDownloadLanguage, setModelDownloadLanguage] = useState('zh');
  const [modelDownloadProgress, setModelDownloadProgress] = useState<ModelDownloadProgressEvent | null>(null);
  const [modelAction, setModelAction] = useState<string | null>(null);
  const [modelCatalog, setModelCatalog] = useState<ModelCatalogEntry[]>([]);
  const [modelCatalogStatus, setModelCatalogStatus] = useState<string | null>(null);
  const [glossaryEntries, setGlossaryEntries] = useState<GlossaryEntry[]>([]);
  const [glossaryAction, setGlossaryAction] = useState<string | null>(null);
  const [glossaryStatus, setGlossaryStatus] = useState<string | null>(null);
  const [glossaryFormId, setGlossaryFormId] = useState<number | null>(null);
  const [glossaryCanonical, setGlossaryCanonical] = useState('');
  const [glossaryAliases, setGlossaryAliases] = useState('');
  const [glossaryCategory, setGlossaryCategory] = useState('');
  const installedModelCount = modelSettings?.installedModels.length ?? 0;

  // Load license state on mount
  useEffect(() => {
    invokeTauri<{
      state: string;
      isUsable: boolean;
      trialDaysRemaining: number;
      modelUpdatesUntil?: string;
    }>('cmd_get_license_state')
      .then(setLicenseInfo)
      .catch(() => setLicenseInfo(null));
  }, []);

  const refreshLicenseInfo = useCallback(() => {
    void invokeTauri<{
      state: string;
      isUsable: boolean;
      trialDaysRemaining: number;
      modelUpdatesUntil?: string;
    }>('cmd_get_license_state')
      .then(setLicenseInfo)
      .catch(() => setLicenseInfo(null));
  }, []);

  const refreshDiagnosticsPreview = useCallback(() => {
    void invokeTauri<DiagnosticsPreview>('cmd_get_diagnostics_preview')
      .then(setDiagnosticsPreview)
      .catch(() => setDiagnosticsPreview(null));
  }, []);

  const refreshRuntimeHealth = useCallback(async () => {
    if (!hasTauriRuntime()) {
      setRuntimeHealth(null);
      setRuntimeHealthStatus(t('runtime.desktopRequired'));
      return;
    }

    setRuntimeHealthRefreshing(true);
    try {
      const health = await invokeTauri<RuntimeHealth>('cmd_get_runtime_health');
      setRuntimeHealth(health);
      setRuntimeHealthStatus(null);
    } catch (error) {
      setRuntimeHealth(null);
      setRuntimeHealthStatus(errorToMessage(error, t));
    } finally {
      setRuntimeHealthRefreshing(false);
    }
  }, [t]);

  const refreshRuntimeComponents = useCallback(async () => {
    if (!hasTauriRuntime()) {
      setRuntimeComponents([]);
      setRuntimeComponentStatus(t('runtime.componentsDesktopRequired'));
      return;
    }

    setRuntimeComponentRefreshing(true);
    try {
      const components = await invokeTauri<RuntimeComponent[]>('cmd_get_runtime_components');
      setRuntimeComponents(components);
      setRuntimeComponentStatus(null);
    } catch (error) {
      setRuntimeComponents([]);
      setRuntimeComponentStatus(errorToMessage(error, t));
    } finally {
      setRuntimeComponentRefreshing(false);
    }
  }, [t]);

  const handleRepairRuntimeDependency = async (item: RuntimeDependency) => {
    setRuntimeRepairAction(item.id);
    setRuntimeHealthStatusTargetId(item.id);
    setRuntimeHealthStatus(t('runtime.repairingItem', { item: runtimeDependencyLabel(item.id, t) }));
    try {
      const result = await invokeTauri<RuntimeRepairResult>('cmd_repair_runtime_dependency', {
        id: item.id,
      });
      setRuntimeHealth(result.health);
      setRuntimeComponents(result.components);
      setRuntimeHealthStatusTargetId(item.id);
      setRuntimeHealthStatus(result.message);
      onRefreshModelSettings();
      refreshDiagnosticsPreview();
    } catch (error) {
      // Keep the failed item id so the error renders next to that health row.
      setRuntimeHealthStatusTargetId(item.id);
      setRuntimeHealthStatus(errorToMessage(error, t));
      void refreshRuntimeHealth();
      void refreshRuntimeComponents();
    } finally {
      setRuntimeRepairAction(null);
    }
  };

  const handleRepairAllRuntimeDependencies = async () => {
    if (!runtimeHealth) return;
    const targets = runtimeHealth.items.filter(
      (item) => item.status !== 'ready' && item.repairable,
    );
    if (targets.length === 0) {
      setRuntimeHealthStatusTargetId(null);
      setRuntimeHealthStatus(t('runtime.noBlocking'));
      return;
    }

    setRuntimeRepairAction('__all__');
    setRuntimeHealthStatusTargetId(null);
    setRuntimeHealthStatus(t('runtime.repairAllStarting', { count: targets.length }));
    let ok = 0;
    let failed = 0;
    let lastMessage = '';
    for (const item of targets) {
      setRuntimeRepairAction(item.id);
      setRuntimeHealthStatusTargetId(item.id);
      setRuntimeHealthStatus(t('runtime.repairingItem', { item: runtimeDependencyLabel(item.id, t) }));
      try {
        const result = await invokeTauri<RuntimeRepairResult>('cmd_repair_runtime_dependency', {
          id: item.id,
        });
        setRuntimeHealth(result.health);
        setRuntimeComponents(result.components);
        lastMessage = result.message;
        ok += 1;
      } catch (error) {
        failed += 1;
        lastMessage = errorToMessage(error, t);
        setRuntimeHealthStatusTargetId(item.id);
        setRuntimeHealthStatus(lastMessage);
      }
    }
    setRuntimeHealthStatusTargetId(null);
    setRuntimeHealthStatus(
      failed === 0
        ? t('runtime.repairAllDone', { count: ok })
        : `${t('runtime.repairAllPartial', { ok, failed })} ${lastMessage}`,
    );
    setRuntimeRepairAction(null);
    onRefreshModelSettings();
    refreshDiagnosticsPreview();
    void refreshRuntimeHealth();
    void refreshRuntimeComponents();
  };

  const handleDownloadRuntimeComponent = async (component: RuntimeComponent) => {
    const label = runtimeComponentLabel(component.id, t);
    setRuntimeComponentAction(`download:${component.id}`);
    setRuntimeComponentStatusTargetId(component.id);
    setRuntimeComponentStatus(t('runtime.componentDownloadStarting', { item: label }));
    setRuntimeComponentProgress({
      id: component.id,
      downloadedBytes: 0,
      totalBytes: component.downloadSizeBytes,
      progressPct: 0,
      message: t('runtime.componentDownloadStartingEvent'),
    });

    try {
      const result = await invokeTauri<RuntimeComponentActionResult>('cmd_download_runtime_component', {
        id: component.id,
      });
      setRuntimeComponents(result.components);
      setRuntimeHealth(result.health);
      setRuntimeComponentStatusTargetId(component.id);
      setRuntimeComponentStatus(result.message);
      setRuntimeComponentProgress(null);
      refreshDiagnosticsPreview();
    } catch (error) {
      setRuntimeComponentStatusTargetId(component.id);
      setRuntimeComponentStatus(errorToMessage(error, t));
      setRuntimeComponentProgress(null);
      void refreshRuntimeComponents();
      void refreshRuntimeHealth();
    } finally {
      setRuntimeComponentAction(null);
    }
  };

  const handleDeleteRuntimeComponent = async (component: RuntimeComponent) => {
    const label = runtimeComponentLabel(component.id, t);
    if (!window.confirm(t('runtime.componentDeleteConfirm', { item: label }))) {
      return;
    }

    setRuntimeComponentAction(`delete:${component.id}`);
    setRuntimeComponentStatusTargetId(component.id);
    try {
      const result = await invokeTauri<RuntimeComponentActionResult>('cmd_delete_runtime_component', {
        id: component.id,
      });
      setRuntimeComponents(result.components);
      setRuntimeHealth(result.health);
      setRuntimeComponentStatusTargetId(component.id);
      setRuntimeComponentStatus(result.message);
      refreshDiagnosticsPreview();
    } catch (error) {
      setRuntimeComponentStatusTargetId(component.id);
      setRuntimeComponentStatus(errorToMessage(error, t));
      void refreshRuntimeComponents();
      void refreshRuntimeHealth();
    } finally {
      setRuntimeComponentAction(null);
    }
  };

  const refreshGlossaryEntries = useCallback(async () => {
    if (!hasTauriRuntime()) {
      setGlossaryEntries([]);
      setGlossaryStatus(t('glossary.desktopRequired'));
      return 0;
    }

    const entries = await invokeTauri<GlossaryEntry[]>('cmd_list_glossary_entries');
    setGlossaryEntries(entries);
    setGlossaryStatus(null);
    return entries.length;
  }, [t]);

  const refreshModelCatalog = useCallback(async () => {
    if (!hasTauriRuntime()) {
      setModelCatalog([]);
      setModelCatalogStatus(t('model.catalogDesktopRequired'));
      return;
    }

    try {
      const entries = await invokeTauri<ModelCatalogEntry[]>('cmd_get_model_catalog');
      setModelCatalog(entries);
      setModelCatalogStatus(null);
    } catch (error) {
      setModelCatalog([]);
      setModelCatalogStatus(errorToMessage(error, t));
    }
  }, [t]);

  useEffect(() => {
    let cancelled = false;
    refreshDiagnosticsPreview();
    onRefreshModelSettings();
    queueMicrotask(() => {
      if (!cancelled) {
        void refreshRuntimeHealth();
      }
    });
    queueMicrotask(() => {
      if (!cancelled) {
        void refreshRuntimeComponents();
      }
    });
    queueMicrotask(() => {
      if (!cancelled) {
        void refreshModelCatalog();
      }
    });
    queueMicrotask(() => {
      if (cancelled) return;
      void refreshGlossaryEntries().catch((error) => {
        if (!cancelled) {
          setGlossaryStatus(errorToMessage(error, t));
        }
      });
    });
    return () => {
      cancelled = true;
    };
  }, [refreshDiagnosticsPreview, refreshGlossaryEntries, refreshModelCatalog, refreshRuntimeComponents, refreshRuntimeHealth, onRefreshModelSettings, t, telemetryConsent]);

  useEffect(() => {
    if (!hasTauriRuntime()) return;

    let unlisten: (() => void) | undefined;
    void listen<ModelDownloadProgressEvent>('model://download-progress', (event) => {
      setModelDownloadProgress(event.payload);
    }).then((cleanup) => {
      unlisten = cleanup;
    });

    return () => {
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    if (!hasTauriRuntime()) return;

    let unlisten: (() => void) | undefined;
    void listen<RuntimeComponentProgressEvent>('runtime://component-download-progress', (event) => {
      setRuntimeComponentProgress(event.payload);
    }).then((cleanup) => {
      unlisten = cleanup;
    });

    return () => {
      unlisten?.();
    };
  }, []);

  const handleClearHistory = async () => {
    setPrivacyAction('history');
    try {
      const result = await invokeTauri<PrivacyActionResult>('cmd_clear_local_history');
      setStatusMsg(t('settings.freed', { message: result.message, size: formatFileSize(result.bytesFreed) }));
      refreshDiagnosticsPreview();
    } catch (error) {
      setStatusMsg(errorToMessage(error, t));
    } finally {
      setPrivacyAction(null);
    }
  };

  const handleDeleteModelCache = async () => {
    setPrivacyAction('models');
    try {
      const result = await invokeTauri<PrivacyActionResult>('cmd_delete_model_cache');
      setStatusMsg(t('settings.removedFreed', {
        message: result.message,
        count: result.itemsAffected,
        size: formatFileSize(result.bytesFreed),
      }));
      refreshDiagnosticsPreview();
      onRefreshModelSettings();
    } catch (error) {
      setStatusMsg(errorToMessage(error, t));
    } finally {
      setPrivacyAction(null);
    }
  };

  const handleImportLocalModel = async () => {
    const filePath = modelPathInput.trim();
    if (!filePath) {
      setModelStatusMsg(t('model.enterPath'));
      return;
    }
    setModelAction('import');
    setModelStatusMsg(t('model.importingStatus'));
    try {
      const settings = await invokeTauri<ModelSettings>('cmd_import_local_model', {
        request: { filePath },
      });
      onModelSettingsChange(settings);
      setModelStatusMsg(t('model.imported'));
      setModelPathInput('');
      refreshDiagnosticsPreview();
      void refreshModelCatalog();
    } catch (error) {
      setModelStatusMsg(errorToMessage(error, t));
    } finally {
      setModelAction(null);
    }
  };

  const handleChooseLocalModelFile = async () => {
    setModelStatusMsg(t('model.openingChooser'));
    try {
      const selected = await openDialog({
        title: t('model.chooserTitle'),
        multiple: false,
        filters: [
          {
            name: 'Whisper models',
            extensions: ['bin', 'gguf'],
          },
        ],
      });

      if (!selected) {
        setModelStatusMsg(t('model.chooserCancelled'));
        return;
      }
      setModelPathInput(Array.isArray(selected) ? selected[0] : selected);
      setModelStatusMsg(t('model.chooserSelected'));
    } catch (error) {
      setModelStatusMsg(
        hasTauriRuntime()
          ? errorToMessage(error, t)
          : t('model.chooserRequiresDesktop')
      );
    }
  };

  const handleDownloadModel = async () => {
    const url = modelDownloadUrl.trim();
    const sha256 = modelDownloadSha256.trim();
    const sizeBytes = Number(modelDownloadSizeBytes.trim());
    if (!url || !sha256 || !Number.isFinite(sizeBytes) || sizeBytes <= 0) {
      setModelStatusMsg(t('model.downloadMissingFields'));
      return;
    }

    setModelAction('download');
    setModelStatusMsg(t('model.downloadStarting'));
    setModelDownloadProgress({
      id: 'pending',
      downloadedBytes: 0,
      totalBytes: sizeBytes,
      progressPct: 0,
      message: t('model.downloadStartingEvent'),
    });
    try {
      const result = await invokeTauri<ModelActionResult>('cmd_download_model', {
        request: {
          url,
          sha256,
          sizeBytes,
          name: modelDownloadName.trim() || null,
          version: modelDownloadVersion.trim() || null,
          language: modelDownloadLanguage.trim() || null,
        },
      });
      onModelSettingsChange(result.settings);
      setModelStatusMsg(result.message);
      setModelDownloadUrl('');
      setModelDownloadSha256('');
      setModelDownloadSizeBytes('');
      setModelDownloadName('');
      setModelDownloadVersion('');
      setModelDownloadProgress(null);
      refreshDiagnosticsPreview();
      void refreshModelCatalog();
    } catch (error) {
      setModelStatusMsg(errorToMessage(error, t));
    } finally {
      setModelAction(null);
    }
  };

  const handleDownloadCatalogModel = async (entry: ModelCatalogEntry) => {
    const actionId = `catalog:${entry.name}:${entry.version}`;
    setModelAction(actionId);
    setModelStatusMsg(t('model.downloadStarting'));
    setModelDownloadProgress({
      id: actionId,
      downloadedBytes: 0,
      totalBytes: entry.sizeBytes,
      progressPct: 0,
      message: t('model.downloadStartingEvent'),
    });

    try {
      const result = await invokeTauri<ModelActionResult>('cmd_download_model', {
        request: {
          url: entry.downloadUrl,
          sha256: entry.sha256,
          sizeBytes: entry.sizeBytes,
          name: entry.name,
          version: entry.version,
          language: entry.language,
        },
      });
      onModelSettingsChange(result.settings);
      setModelStatusMsg(result.message);
      setModelDownloadProgress(null);
      refreshDiagnosticsPreview();
      void refreshModelCatalog();
    } catch (error) {
      setModelStatusMsg(errorToMessage(error, t));
    } finally {
      setModelAction(null);
    }
  };

  const handleSelectModel = async (model: ModelInfo) => {
    setModelAction(`${model.name}:${model.version}`);
    try {
      const settings = await invokeTauri<ModelSettings>('cmd_select_model', {
        request: { name: model.name, version: model.version },
      });
      onModelSettingsChange(settings);
      setModelStatusMsg(t('model.selectedStatus', { name: model.name, version: model.version }));
      void refreshModelCatalog();
    } catch (error) {
      setModelStatusMsg(errorToMessage(error, t));
    } finally {
      setModelAction(null);
    }
  };

  const handleDeleteModel = async (model: ModelInfo) => {
    const label = `${model.name} v${model.version}`;
    if (!window.confirm(t('model.deleteConfirm', { label }))) {
      return;
    }

    setModelAction(`delete:${model.name}:${model.version}`);
    try {
      const result = await invokeTauri<ModelActionResult>('cmd_delete_model', {
        request: { name: model.name, version: model.version },
      });
      onModelSettingsChange(result.settings);
      setModelStatusMsg(t('model.freed', { message: result.message, size: formatFileSize(result.bytesFreed) }));
      refreshDiagnosticsPreview();
      void refreshModelCatalog();
    } catch (error) {
      setModelStatusMsg(errorToMessage(error, t));
    } finally {
      setModelAction(null);
    }
  };

  const handleClearUnusedModels = async () => {
    setModelAction('clear-unused');
    try {
      const result = await invokeTauri<ModelActionResult>('cmd_clear_unused_models');
      onModelSettingsChange(result.settings);
      setModelStatusMsg(t('model.freed', { message: result.message, size: formatFileSize(result.bytesFreed) }));
      refreshDiagnosticsPreview();
      void refreshModelCatalog();
    } catch (error) {
      setModelStatusMsg(errorToMessage(error, t));
    } finally {
      setModelAction(null);
    }
  };

  const handleRefreshGlossary = async () => {
    setGlossaryAction('refresh');
    try {
      const count = await refreshGlossaryEntries();
      setGlossaryStatus(t('glossary.refreshed', { count }));
    } catch (error) {
      setGlossaryStatus(errorToMessage(error, t));
    } finally {
      setGlossaryAction(null);
    }
  };

  const handleDeleteGlossaryEntry = async (entry: GlossaryEntry) => {
    if (!window.confirm(t('glossary.deleteConfirm', { term: entry.canonical }))) {
      return;
    }

    setGlossaryAction(`delete:${entry.id}`);
    try {
      const entries = await invokeTauri<GlossaryEntry[]>('cmd_delete_glossary_entry', {
        id: entry.id,
      });
      setGlossaryEntries(entries);
      if (glossaryFormId === entry.id) {
        setGlossaryFormId(null);
        setGlossaryCanonical('');
        setGlossaryAliases('');
        setGlossaryCategory('');
      }
      setGlossaryStatus(t('glossary.deleted', { term: entry.canonical }));
    } catch (error) {
      setGlossaryStatus(errorToMessage(error, t));
    } finally {
      setGlossaryAction(null);
    }
  };

  const resetGlossaryForm = () => {
    setGlossaryFormId(null);
    setGlossaryCanonical('');
    setGlossaryAliases('');
    setGlossaryCategory('');
  };

  const handleEditGlossaryEntry = (entry: GlossaryEntry) => {
    setGlossaryFormId(entry.id);
    setGlossaryCanonical(entry.canonical);
    setGlossaryAliases(entry.aliases.map((alias) => alias.alias).join(', '));
    setGlossaryCategory(entry.category ?? '');
    setGlossaryStatus(null);
  };

  const handleSaveGlossaryEntry = async () => {
    const canonical = glossaryCanonical.trim();
    if (!canonical) {
      setGlossaryStatus(t('glossary.enterCanonical'));
      return;
    }

    const aliases = glossaryAliases
      .split(/[\n,，]/)
      .map((value) => value.trim())
      .filter(Boolean);
    setGlossaryAction('save');
    try {
      const entries = await invokeTauri<GlossaryEntry[]>('cmd_save_glossary_entry', {
        request: {
          id: glossaryFormId,
          canonical,
          aliases,
          category: glossaryCategory.trim() || null,
        },
      });
      setGlossaryEntries(entries);
      setGlossaryStatus(glossaryFormId
        ? t('glossary.updated', { term: canonical })
        : t('glossary.saved', { term: canonical }));
      resetGlossaryForm();
    } catch (error) {
      setGlossaryStatus(errorToMessage(error, t));
    } finally {
      setGlossaryAction(null);
    }
  };

  const handleExportDiagnostics = async () => {
    setPrivacyAction('diagnostics');
    try {
      const path = await invokeTauri<string>('cmd_export_diagnostics_package');
      setStatusMsg(t('settings.diagnosticsExported', { path }));
      refreshDiagnosticsPreview();
    } catch (error) {
      setStatusMsg(errorToMessage(error, t));
    } finally {
      setPrivacyAction(null);
    }
  };

  const handleActivateLicense = async () => {
    const licenseKey = licenseKeyInput.trim();
    if (!licenseKey) {
      setStatusMsg(t('settings.enterLicenseKey'));
      return;
    }

    setLicenseAction(true);
    setStatusMsg('');
    try {
      const message = await invokeTauri<string>('cmd_activate_license', { licenseKey });
      setStatusMsg(message || t('settings.licenseActivated'));
      setLicenseKeyInput('');
      refreshLicenseInfo();
    } catch (error) {
      setStatusMsg(errorToMessage(error, t));
    } finally {
      setLicenseAction(false);
    }
  };

  return (
    <SettingsContent
      asrEngine={asrEngine}
      t={t}
      language={language}
      setLanguage={setLanguage}
      telemetryConsent={telemetryConsent}
      telemetryStatus={telemetryStatus}
      onTelemetryConsentChange={onTelemetryConsentChange}
      licenseInfo={licenseInfo}
      licenseKeyInput={licenseKeyInput}
      setLicenseKeyInput={setLicenseKeyInput}
      licenseAction={licenseAction}
      handleActivateLicense={handleActivateLicense}
      statusMsg={statusMsg}
      setStatusMsg={setStatusMsg}
      telemetrySaving={telemetrySaving}
      setTelemetrySaving={setTelemetrySaving}
      diagnosticsPreview={diagnosticsPreview}
      privacyAction={privacyAction}
      handleClearHistory={handleClearHistory}
      handleDeleteModelCache={handleDeleteModelCache}
      handleExportDiagnostics={handleExportDiagnostics}
      runtimeHealth={runtimeHealth}
      runtimeHealthStatus={runtimeHealthStatus}
      runtimeHealthStatusTargetId={runtimeHealthStatusTargetId}
      runtimeHealthRefreshing={runtimeHealthRefreshing}
      runtimeRepairAction={runtimeRepairAction}
      refreshRuntimeHealth={refreshRuntimeHealth}
      handleRepairRuntimeDependency={handleRepairRuntimeDependency}
      handleRepairAllRuntimeDependencies={handleRepairAllRuntimeDependencies}
      runtimeComponents={runtimeComponents}
      runtimeComponentStatus={runtimeComponentStatus}
      runtimeComponentStatusTargetId={runtimeComponentStatusTargetId}
      runtimeComponentRefreshing={runtimeComponentRefreshing}
      runtimeComponentAction={runtimeComponentAction}
      runtimeComponentProgress={runtimeComponentProgress}
      refreshRuntimeComponents={refreshRuntimeComponents}
      handleDownloadRuntimeComponent={handleDownloadRuntimeComponent}
      handleDeleteRuntimeComponent={handleDeleteRuntimeComponent}
      modelSettings={modelSettings}
      modelSettingsError={modelSettingsError}
      modelStatusMsg={modelStatusMsg}
      modelPathInput={modelPathInput}
      setModelPathInput={setModelPathInput}
      modelDownloadUrl={modelDownloadUrl}
      setModelDownloadUrl={setModelDownloadUrl}
      modelDownloadSha256={modelDownloadSha256}
      setModelDownloadSha256={setModelDownloadSha256}
      modelDownloadSizeBytes={modelDownloadSizeBytes}
      setModelDownloadSizeBytes={setModelDownloadSizeBytes}
      modelDownloadName={modelDownloadName}
      setModelDownloadName={setModelDownloadName}
      modelDownloadVersion={modelDownloadVersion}
      setModelDownloadVersion={setModelDownloadVersion}
      modelDownloadLanguage={modelDownloadLanguage}
      setModelDownloadLanguage={setModelDownloadLanguage}
      modelDownloadProgress={modelDownloadProgress}
      modelAction={modelAction}
      modelCatalog={modelCatalog}
      modelCatalogStatus={modelCatalogStatus}
      installedModelCount={installedModelCount}
      refreshModelCatalog={refreshModelCatalog}
      handleChooseLocalModelFile={handleChooseLocalModelFile}
      handleImportLocalModel={handleImportLocalModel}
      handleDownloadModel={handleDownloadModel}
      handleDownloadCatalogModel={handleDownloadCatalogModel}
      handleSelectModel={handleSelectModel}
      handleDeleteModel={handleDeleteModel}
      handleClearUnusedModels={handleClearUnusedModels}
      glossaryEntries={glossaryEntries}
      glossaryAction={glossaryAction}
      glossaryStatus={glossaryStatus}
      glossaryFormId={glossaryFormId}
      glossaryCanonical={glossaryCanonical}
      setGlossaryCanonical={setGlossaryCanonical}
      glossaryAliases={glossaryAliases}
      setGlossaryAliases={setGlossaryAliases}
      glossaryCategory={glossaryCategory}
      setGlossaryCategory={setGlossaryCategory}
      handleRefreshGlossary={handleRefreshGlossary}
      handleSaveGlossaryEntry={handleSaveGlossaryEntry}
      resetGlossaryForm={resetGlossaryForm}
      handleEditGlossaryEntry={handleEditGlossaryEntry}
      handleDeleteGlossaryEntry={handleDeleteGlossaryEntry}
    />
  );
}
