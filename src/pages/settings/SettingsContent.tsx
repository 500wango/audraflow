import { useCallback } from 'react';
import type { AppLanguage } from '../../i18nContext';
import type {
  AsrEngine,
  DiagnosticsPreview,
  GlossaryEntry,
  ModelCatalogEntry,
  ModelDownloadProgressEvent,
  ModelInfo,
  ModelSettings,
  RuntimeComponent,
  RuntimeComponentProgressEvent,
  RuntimeDependency,
  RuntimeHealth,
  TelemetryConsentState,
} from '../../types';
import {
  formatFileSize,
  runtimeComponentHint,
  runtimeComponentLabel,
  runtimeDependencyFix,
  runtimeDependencyHint,
  runtimeDependencyLabel,
  runtimeKindLabel,
  runtimeStatusLabel,
} from '../../appUtils';
import { errorToMessage, type Translate } from '../../tauri';

type StringSetter = (value: string) => void;
type BooleanSetter = (value: boolean) => void;

interface LicenseInfo {
  state: string;
  isUsable: boolean;
  trialDaysRemaining: number;
  modelUpdatesUntil?: string;
}

interface SettingsContentProps {
  asrEngine: AsrEngine;
  t: Translate;
  language: AppLanguage;
  setLanguage: (language: AppLanguage) => void;
  telemetryConsent: TelemetryConsentState;
  telemetryStatus: string | null;
  onTelemetryConsentChange: (enabled: boolean) => Promise<TelemetryConsentState>;
  licenseInfo: LicenseInfo | null;
  licenseKeyInput: string;
  setLicenseKeyInput: StringSetter;
  licenseAction: boolean;
  handleActivateLicense: () => Promise<void>;
  statusMsg: string;
  setStatusMsg: StringSetter;
  telemetrySaving: boolean;
  setTelemetrySaving: BooleanSetter;
  diagnosticsPreview: DiagnosticsPreview | null;
  privacyAction: string | null;
  handleClearHistory: () => Promise<void>;
  handleDeleteModelCache: () => Promise<void>;
  handleExportDiagnostics: () => Promise<void>;
  runtimeHealth: RuntimeHealth | null;
  runtimeHealthStatus: string | null;
  runtimeHealthRefreshing: boolean;
  runtimeRepairAction: string | null;
  refreshRuntimeHealth: () => Promise<void>;
  handleRepairRuntimeDependency: (item: RuntimeDependency) => Promise<void>;
  runtimeComponents: RuntimeComponent[];
  runtimeComponentStatus: string | null;
  /** Component id that the latest status message refers to (e.g. yt-dlp). */
  runtimeComponentStatusTargetId: string | null;
  runtimeComponentRefreshing: boolean;
  runtimeComponentAction: string | null;
  runtimeComponentProgress: RuntimeComponentProgressEvent | null;
  refreshRuntimeComponents: () => Promise<void>;
  handleDownloadRuntimeComponent: (component: RuntimeComponent) => Promise<void>;
  handleDeleteRuntimeComponent: (component: RuntimeComponent) => Promise<void>;
  modelSettings: ModelSettings | null;
  modelSettingsError: string | null;
  modelStatusMsg: string;
  modelPathInput: string;
  setModelPathInput: StringSetter;
  modelDownloadUrl: string;
  setModelDownloadUrl: StringSetter;
  modelDownloadSha256: string;
  setModelDownloadSha256: StringSetter;
  modelDownloadSizeBytes: string;
  setModelDownloadSizeBytes: StringSetter;
  modelDownloadName: string;
  setModelDownloadName: StringSetter;
  modelDownloadVersion: string;
  setModelDownloadVersion: StringSetter;
  modelDownloadLanguage: string;
  setModelDownloadLanguage: StringSetter;
  modelDownloadProgress: ModelDownloadProgressEvent | null;
  modelAction: string | null;
  modelCatalog: ModelCatalogEntry[];
  modelCatalogStatus: string | null;
  installedModelCount: number;
  refreshModelCatalog: () => Promise<void>;
  handleChooseLocalModelFile: () => Promise<void>;
  handleImportLocalModel: () => Promise<void>;
  handleDownloadModel: () => Promise<void>;
  handleDownloadCatalogModel: (entry: ModelCatalogEntry) => Promise<void>;
  handleSelectModel: (model: ModelInfo) => Promise<void>;
  handleDeleteModel: (model: ModelInfo) => Promise<void>;
  handleClearUnusedModels: () => Promise<void>;
  glossaryEntries: GlossaryEntry[];
  glossaryAction: string | null;
  glossaryStatus: string | null;
  glossaryFormId: number | null;
  glossaryCanonical: string;
  setGlossaryCanonical: StringSetter;
  glossaryAliases: string;
  setGlossaryAliases: StringSetter;
  glossaryCategory: string;
  setGlossaryCategory: StringSetter;
  handleRefreshGlossary: () => Promise<void>;
  handleSaveGlossaryEntry: () => Promise<void>;
  resetGlossaryForm: () => void;
  handleEditGlossaryEntry: (entry: GlossaryEntry) => void;
  handleDeleteGlossaryEntry: (entry: GlossaryEntry) => Promise<void>;
}

export function SettingsContent({
  asrEngine,
  t,
  language,
  setLanguage,
  telemetryConsent,
  telemetryStatus,
  onTelemetryConsentChange,
  licenseInfo,
  licenseKeyInput,
  setLicenseKeyInput,
  licenseAction,
  handleActivateLicense,
  statusMsg,
  setStatusMsg,
  telemetrySaving,
  setTelemetrySaving,
  diagnosticsPreview,
  privacyAction,
  handleClearHistory,
  handleDeleteModelCache,
  handleExportDiagnostics,
  runtimeHealth,
  runtimeHealthStatus,
  runtimeHealthRefreshing,
  runtimeRepairAction,
  refreshRuntimeHealth,
  handleRepairRuntimeDependency,
  runtimeComponents,
  runtimeComponentStatus,
  runtimeComponentStatusTargetId,
  runtimeComponentRefreshing,
  runtimeComponentAction,
  runtimeComponentProgress,
  refreshRuntimeComponents,
  handleDownloadRuntimeComponent,
  handleDeleteRuntimeComponent,
  modelSettings,
  modelSettingsError,
  modelStatusMsg,
  modelPathInput,
  setModelPathInput,
  modelDownloadUrl,
  setModelDownloadUrl,
  modelDownloadSha256,
  setModelDownloadSha256,
  modelDownloadSizeBytes,
  setModelDownloadSizeBytes,
  modelDownloadName,
  setModelDownloadName,
  modelDownloadVersion,
  setModelDownloadVersion,
  modelDownloadLanguage,
  setModelDownloadLanguage,
  modelDownloadProgress,
  modelAction,
  modelCatalog,
  modelCatalogStatus,
  installedModelCount,
  refreshModelCatalog,
  handleChooseLocalModelFile,
  handleImportLocalModel,
  handleDownloadModel,
  handleDownloadCatalogModel,
  handleSelectModel,
  handleDeleteModel,
  handleClearUnusedModels,
  glossaryEntries,
  glossaryAction,
  glossaryStatus,
  glossaryFormId,
  glossaryCanonical,
  setGlossaryCanonical,
  glossaryAliases,
  setGlossaryAliases,
  glossaryCategory,
  setGlossaryCategory,
  handleRefreshGlossary,
  handleSaveGlossaryEntry,
  resetGlossaryForm,
  handleEditGlossaryEntry,
  handleDeleteGlossaryEntry,
}: SettingsContentProps) {
  const settingsNavItems = [
    { id: 'settings-interface', label: t('settings.interface') },
    { id: 'settings-runtime', label: t('runtime.sectionTitle') },
    { id: 'settings-license', label: t('settings.license') },
    { id: 'settings-model', label: t('model.sectionTitle') },
    { id: 'settings-glossary', label: t('glossary.sectionTitle') },
    { id: 'settings-privacy', label: t('telemetry.sectionTitle') },
  ];

  const scrollToSetting = useCallback((sectionId: string) => {
    const target = document.getElementById(sectionId);
    if (!target) return;
    const reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    target.scrollIntoView({
      block: 'start',
      behavior: reduceMotion ? 'auto' : 'smooth',
    });
  }, []);

  return (
    <div className="page settings-page">
      <h2>{t('settings.title')}</h2>
      <nav className="settings-section-nav" aria-label={t('settings.sectionNav')}>
        {settingsNavItems.map((item) => (
          <button
            key={item.id}
            className="btn-secondary btn-sm-inline"
            type="button"
            onClick={() => scrollToSetting(item.id)}
          >
            {item.label}
          </button>
        ))}
      </nav>

      <section id="settings-interface" className="settings-section">
        <h3>{t('settings.interface')}</h3>
        <div className="setting-row">
          <div>
            <strong>{t('language.label')}</strong>
          </div>
          <select
            className="language-select"
            value={language}
            onChange={(event) => setLanguage(event.target.value as AppLanguage)}
          >
            <option value="en">{t('language.english')}</option>
            <option value="zh">{t('language.chinese')}</option>
          </select>
        </div>
      </section>

      <section id="settings-runtime" className="settings-section">
        <div className="settings-section-header">
          <h3>{t('runtime.sectionTitle')}</h3>
          <button
            className="btn-secondary btn-sm-inline"
            disabled={runtimeHealthRefreshing}
            onClick={() => {
              void refreshRuntimeHealth();
            }}
          >
            {runtimeHealthRefreshing ? t('runtime.refreshing') : t('runtime.refresh')}
          </button>
        </div>
        {runtimeHealth ? (
          <>
            <div className="runtime-health-summary">
              <span>
                {runtimeHealth.blockingCount > 0
                  ? t('runtime.blockingSummary', { count: runtimeHealth.blockingCount })
                  : t('runtime.noBlocking')}
              </span>
              <span>{t('runtime.warningSummary', { count: runtimeHealth.warningCount })}</span>
            </div>
            <div className="runtime-health-list">
              {runtimeHealth.items.map((item) => {
                const label = runtimeDependencyLabel(item.id, t);
                return (
                  <div key={item.id} className="runtime-health-row">
                    <div className="runtime-health-main">
                      <div className="runtime-health-title">
                        <strong>{label}</strong>
                        <span className={`runtime-status-badge runtime-status-${item.status}`}>
                          {runtimeStatusLabel(item.status, t)}
                        </span>
                        <span className="runtime-kind-badge">{runtimeKindLabel(item.kind, t)}</span>
                      </div>
                      <p className="setting-meta">{runtimeDependencyHint(item.id, t)}</p>
                      {item.path ? <p className="setting-meta">{t('runtime.path', { path: item.path })}</p> : null}
                      {item.version ? <p className="setting-meta">{t('runtime.version', { version: item.version })}</p> : null}
                      {item.detail ? <p className="setting-meta">{t('runtime.detail', { detail: item.detail })}</p> : null}
                    </div>
                    {item.status !== 'ready' ? (
                      <div className="runtime-health-actions">
                        <p className="runtime-fix">{runtimeDependencyFix(item.id, t)}</p>
                        {item.repairable ? (
                          <button
                            className="btn-secondary btn-sm-inline"
                            disabled={runtimeRepairAction !== null}
                            onClick={() => {
                              void handleRepairRuntimeDependency(item);
                            }}
                          >
                            {runtimeRepairAction === item.id ? t('runtime.repairing') : t('runtime.repair')}
                          </button>
                        ) : null}
                      </div>
                    ) : null}
                  </div>
                );
              })}
            </div>
          </>
        ) : (
          <p className="status-msg model-status-msg">
            {runtimeHealthStatus || t('runtime.loading')}
          </p>
        )}
        {runtimeHealthStatus && runtimeHealth ? (
          <p className="status-msg model-status-msg">{runtimeHealthStatus}</p>
        ) : null}
        <div className="runtime-component-panel">
          <div className="settings-section-header">
            <div>
              <h4>{t('runtime.componentsTitle')}</h4>
              <p className="setting-meta">{t('runtime.componentsDescription')}</p>
            </div>
            <button
              className="btn-secondary btn-sm-inline"
              disabled={runtimeComponentRefreshing || runtimeComponentAction !== null}
              onClick={() => {
                void refreshRuntimeComponents();
              }}
            >
              {runtimeComponentRefreshing ? t('runtime.refreshing') : t('runtime.componentsRefresh')}
            </button>
          </div>
          {runtimeComponents.length > 0 ? (
            <div className="runtime-component-list">
              {runtimeComponents.map((component) => {
                const label = runtimeComponentLabel(component.id, t);
                const isDownloading = runtimeComponentAction === `download:${component.id}`;
                const isDeleting = runtimeComponentAction === `delete:${component.id}`;
                const progress = runtimeComponentProgress?.id === component.id
                  ? runtimeComponentProgress
                  : null;
                const canDelete = component.id !== 'vc-redist' &&
                  (component.status === 'ready' || component.installedSizeBytes > 0);

                return (
                  <div key={component.id} className="runtime-component-row">
                    <div className="runtime-component-main">
                      <div className="runtime-health-title">
                        <strong>{label}</strong>
                        <span className={`runtime-status-badge runtime-status-${component.status}`}>
                          {runtimeStatusLabel(component.status, t)}
                        </span>
                        <span className="runtime-kind-badge">{runtimeKindLabel(component.kind, t)}</span>
                      </div>
                      <p className="setting-meta">{runtimeComponentHint(component.id, t)}</p>
                      <p className="setting-meta">{t('runtime.componentInstallDir', { path: component.installDir })}</p>
                      <div className="runtime-component-meta">
                        <span>{t('runtime.componentDownloadSize', { size: formatFileSize(component.downloadSizeBytes) })}</span>
                        <span>{t('runtime.componentInstalledSize', { size: formatFileSize(component.installedSizeBytes) })}</span>
                        <span>{t('runtime.componentFiles', { files: component.requiredFiles.join(', ') })}</span>
                      </div>
                      {component.detail ? (
                        <p className="setting-meta">{t('runtime.detail', { detail: component.detail })}</p>
                      ) : null}
                      {progress ? (
                        <>
                          <div className="runtime-component-progress-text">
                            <span>{progress.message}</span>
                            <span>{formatFileSize(progress.downloadedBytes)} / {formatFileSize(progress.totalBytes)}</span>
                          </div>
                          <div className="model-download-progress" aria-label={t('runtime.componentDownload')}>
                            <div style={{ width: `${progress.progressPct}%` }} />
                          </div>
                        </>
                      ) : null}
                      {/* Show per-row action result so errors are not lost below a long list. */}
                      {runtimeComponentStatus
                        && runtimeComponentStatusTargetId === component.id ? (
                        <p className="status-msg model-status-msg runtime-component-row-status">
                          {runtimeComponentStatus}
                        </p>
                      ) : null}
                    </div>
                    <div className="runtime-component-actions">
                      <button
                        className="btn-secondary btn-sm-inline"
                        disabled={runtimeComponentAction !== null || !component.installable}
                        onClick={() => {
                          void handleDownloadRuntimeComponent(component);
                        }}
                      >
                        {isDownloading
                          ? t('runtime.componentDownloading')
                          : component.status === 'ready'
                            ? t('runtime.componentReinstall')
                            : t('runtime.componentDownload')}
                      </button>
                      <button
                        className="btn-secondary btn-sm-inline"
                        disabled={runtimeComponentAction !== null || !canDelete}
                        onClick={() => {
                          void handleDeleteRuntimeComponent(component);
                        }}
                      >
                        {isDeleting ? t('runtime.componentDeleting') : t('runtime.componentDelete')}
                      </button>
                      {!component.installable ? (
                        <p className="runtime-fix">{t('runtime.componentUnavailable')}</p>
                      ) : null}
                    </div>
                  </div>
                );
              })}
            </div>
          ) : (
            <p className="status-msg model-status-msg">
              {runtimeComponentStatus || (runtimeComponentRefreshing ? t('runtime.refreshing') : t('runtime.componentEmpty'))}
            </p>
          )}
          {runtimeComponentStatus && runtimeComponents.length > 0 ? (
            <p className="status-msg model-status-msg">{runtimeComponentStatus}</p>
          ) : null}
        </div>
      </section>

      {/* License */}
      <section id="settings-license" className="settings-section">
        <h3>{t('settings.license')}</h3>
        {licenseInfo ? (
          <div className="license-info">
            <div className="info-row">
              <span>{t('settings.status')}</span>
              <span className={`license-badge license-${licenseInfo.state}`}>
                {licenseInfo.state === 'trial' && t('settings.trial')}
                {licenseInfo.state === 'activated' && t('settings.activated')}
                {licenseInfo.state === 'trial_expired' && t('settings.expired')}
                {licenseInfo.state === 'not_activated' && t('settings.notActivated')}
              </span>
            </div>
            {licenseInfo.state === 'trial' && (
              <div className="info-row">
                <span>{t('settings.daysRemaining')}</span>
                <span>{t('settings.days', { count: licenseInfo.trialDaysRemaining })}</span>
              </div>
            )}
            {licenseInfo.state === 'activated' && licenseInfo.modelUpdatesUntil && (
              <div className="info-row">
                <span>{t('settings.modelUpdatesUntil')}</span>
                <span>{new Date(licenseInfo.modelUpdatesUntil).toLocaleDateString()}</span>
              </div>
            )}
          </div>
        ) : (
          <p className="placeholder-text">{t('settings.licenseUnavailable')}</p>
        )}
        <div className="license-activation-row">
          <input
            value={licenseKeyInput}
            onChange={(event) => setLicenseKeyInput(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') {
                void handleActivateLicense();
              }
            }}
            placeholder={t('settings.licenseKeyPlaceholder')}
            disabled={licenseAction}
          />
          <button
            className="btn-secondary"
            disabled={licenseAction || !licenseKeyInput.trim()}
            onClick={() => {
              void handleActivateLicense();
            }}
          >
            {licenseAction ? t('settings.activating') : t('settings.activateLicense')}
          </button>
        </div>
      </section>

      <section id="settings-model" className="settings-section">
        <h3>{t('model.sectionTitle')}</h3>
        <div className="model-summary">
          <div className="info-row">
            <span>{t('model.current')}</span>
            <span>
              {modelSettings?.selectedModel
                ? `${modelSettings.selectedModel.name} v${modelSettings.selectedModel.version}`
                : t('model.notSelectedValue')}
            </span>
          </div>
          <div className="info-row">
            <span>{t('model.directory')}</span>
            <span>{modelSettings?.modelsDir || t('model.unavailable')}</span>
          </div>
        </div>
        {modelSettingsError ? (
          <p className="status-msg model-status-msg">{t('model.settingsUnavailable', { error: modelSettingsError })}</p>
        ) : null}
        <div className="model-import-row">
          <input
            type="text"
            value={modelPathInput}
            onChange={(event) => setModelPathInput(event.target.value)}
            placeholder="C:\\models\\ggml-base.bin"
            disabled={modelAction !== null}
          />
          <button
            className="btn-secondary"
            disabled={modelAction !== null}
            onClick={() => {
              void handleChooseLocalModelFile();
            }}
          >
            {t('model.chooseFile')}
          </button>
          <button
            className="btn-secondary"
            disabled={modelAction !== null || !modelPathInput.trim()}
            onClick={() => {
              void handleImportLocalModel();
            }}
          >
            {modelAction === 'import' ? t('model.importing') : t('model.importLocal')}
          </button>
        </div>
        {modelStatusMsg ? <p className="status-msg model-status-msg">{modelStatusMsg}</p> : null}
        <div className="model-catalog-panel">
          <div className="settings-section-header">
            <h4>{t('model.catalogTitle')}</h4>
            <button
              className="btn-secondary btn-sm-inline"
              disabled={modelAction !== null}
              onClick={() => {
                void refreshModelCatalog();
              }}
            >
              {t('model.catalogRefresh')}
            </button>
          </div>
          {modelCatalogStatus ? <p className="status-msg model-status-msg">{modelCatalogStatus}</p> : null}
          <div className="model-catalog-list">
            {modelCatalog.map((entry) => {
              const installedMatch = modelSettings?.installedModels.find((model) =>
                model.name === entry.name && model.version === entry.version
              );
              const actionId = `catalog:${entry.name}:${entry.version}`;
              return (
                <div key={`${entry.name}:${entry.version}`} className="model-row model-catalog-row">
                  <div>
                    <strong>
                      {entry.name} {entry.recommended ? t('model.recommendedBadge') : ''}
                    </strong>
                    <p className="setting-meta">
                      {entry.language} · {formatFileSize(entry.sizeBytes)} · {entry.sha256.slice(0, 12)}
                    </p>
                    <p className="setting-meta">{entry.description}</p>
                  </div>
                  <div className="model-row-actions">
                    {installedMatch ? (
                      <button
                        className="btn-secondary"
                        disabled={installedMatch.selected || modelAction !== null}
                        onClick={() => {
                          void handleSelectModel(installedMatch);
                        }}
                      >
                        {installedMatch.selected ? t('model.selected') : t('model.select')}
                      </button>
                    ) : (
                      <button
                        className="btn-secondary"
                        disabled={modelAction !== null}
                        onClick={() => {
                          void handleDownloadCatalogModel(entry);
                        }}
                      >
                        {modelAction === actionId ? t('model.downloading') : t('model.download')}
                      </button>
                    )}
                  </div>
                </div>
              );
            })}
            {modelCatalog.length === 0 && !modelCatalogStatus ? (
              <p className="placeholder-text">{t('model.catalogEmpty')}</p>
            ) : null}
          </div>
        </div>
        <div className="model-download-panel">
          <div className="model-download-grid">
            <input
              type="url"
              value={modelDownloadUrl}
              onChange={(event) => setModelDownloadUrl(event.target.value)}
              placeholder="https://example.com/ggml-base.bin"
              disabled={modelAction !== null}
            />
            <input
              type="text"
              value={modelDownloadSha256}
              onChange={(event) => setModelDownloadSha256(event.target.value)}
              placeholder={t('model.shaPlaceholder')}
              disabled={modelAction !== null}
            />
            <input
              type="number"
              min="1"
              value={modelDownloadSizeBytes}
              onChange={(event) => setModelDownloadSizeBytes(event.target.value)}
              placeholder={t('model.sizePlaceholder')}
              disabled={modelAction !== null}
            />
            <input
              type="text"
              value={modelDownloadName}
              onChange={(event) => setModelDownloadName(event.target.value)}
              placeholder={t('model.namePlaceholder')}
              disabled={modelAction !== null}
            />
            <input
              type="text"
              value={modelDownloadVersion}
              onChange={(event) => setModelDownloadVersion(event.target.value)}
              placeholder={t('model.versionPlaceholder')}
              disabled={modelAction !== null}
            />
            <input
              type="text"
              value={modelDownloadLanguage}
              onChange={(event) => setModelDownloadLanguage(event.target.value)}
              placeholder={t('model.languagePlaceholder')}
              disabled={modelAction !== null}
            />
          </div>
          <div className="model-download-actions">
            <button
              className="btn-secondary"
              disabled={
                modelAction !== null ||
                !modelDownloadUrl.trim() ||
                !modelDownloadSha256.trim() ||
                !modelDownloadSizeBytes.trim()
              }
              onClick={() => {
                void handleDownloadModel();
              }}
            >
              {modelAction === 'download' ? t('model.downloading') : t('model.download')}
            </button>
            {modelDownloadProgress ? (
              <span>
                {modelDownloadProgress.message} · {formatFileSize(modelDownloadProgress.downloadedBytes)} / {formatFileSize(modelDownloadProgress.totalBytes)}
              </span>
            ) : null}
          </div>
          {modelDownloadProgress ? (
            <div className="model-download-progress" aria-label={t('model.download')}>
              <div style={{ width: `${modelDownloadProgress.progressPct}%` }} />
            </div>
          ) : null}
        </div>
        <div className="model-maintenance-row">
          <span>
            {t('model.installedCount', {
              count: installedModelCount,
              plural: installedModelCount === 1 ? '' : 's',
            })}
          </span>
          <button
            className="btn-secondary"
            disabled={modelAction !== null || (modelSettings?.installedModels ?? []).length <= 1}
            onClick={() => {
              void handleClearUnusedModels();
            }}
          >
            {modelAction === 'clear-unused' ? t('model.clearing') : t('model.clearUnused')}
          </button>
        </div>
        <div className="model-list">
          {(modelSettings?.installedModels ?? []).length === 0 ? (
            <p className="placeholder-text">{t('model.noLocalModels')}</p>
          ) : (
            modelSettings!.installedModels.map((model) => (
              <div key={`${model.name}:${model.version}`} className="model-row">
                <div>
                  <strong>{model.name} v{model.version}</strong>
                  {model.bundled ? <span className="runtime-kind-badge">{t('model.bundled')}</span> : null}
                  <p className="setting-meta">
                    {model.language} · {formatFileSize(model.sizeBytes)} · {model.sha256.slice(0, 12)}
                  </p>
                  <p className="setting-meta">{model.path}</p>
                </div>
                <div className="model-row-actions">
                  <button
                    className="btn-secondary"
                    disabled={model.selected || modelAction !== null}
                    onClick={() => {
                      void handleSelectModel(model);
                    }}
                  >
                    {model.selected ? t('model.selected') : modelAction === `${model.name}:${model.version}` ? t('model.selecting') : t('model.select')}
                  </button>
                  <button
                    className="btn-secondary danger-button"
                    disabled={modelAction !== null || model.bundled}
                    onClick={() => {
                      void handleDeleteModel(model);
                    }}
                  >
                    {model.bundled ? t('model.bundled') : modelAction === `delete:${model.name}:${model.version}` ? t('model.deleting') : t('model.delete')}
                  </button>
                </div>
              </div>
            ))
          )}
        </div>
      </section>

      <section id="settings-glossary" className="settings-section">
        <div className="settings-section-header">
          <h3>{t('glossary.sectionTitle')}</h3>
          <button
            className="btn-secondary btn-sm-inline"
            disabled={glossaryAction !== null}
            onClick={() => {
              void handleRefreshGlossary();
            }}
          >
            {glossaryAction === 'refresh' ? t('glossary.refreshing') : t('glossary.refresh')}
          </button>
        </div>
        <p className="hint">{t('glossary.description')}</p>
        <div className="glossary-form">
          <input
            type="text"
            value={glossaryCanonical}
            onChange={(event) => setGlossaryCanonical(event.target.value)}
            placeholder={t('glossary.canonicalPlaceholder')}
            disabled={glossaryAction !== null}
          />
          <input
            type="text"
            value={glossaryAliases}
            onChange={(event) => setGlossaryAliases(event.target.value)}
            placeholder={t('glossary.aliasesPlaceholder')}
            disabled={glossaryAction !== null}
          />
          <input
            type="text"
            value={glossaryCategory}
            onChange={(event) => setGlossaryCategory(event.target.value)}
            placeholder={t('glossary.categoryPlaceholder')}
            disabled={glossaryAction !== null}
          />
          <div className="glossary-form-actions">
            <button
              className="btn-secondary"
              disabled={glossaryAction !== null || !glossaryCanonical.trim()}
              onClick={() => {
                void handleSaveGlossaryEntry();
              }}
            >
              {glossaryAction === 'save'
                ? t('glossary.saving')
                : glossaryFormId
                  ? t('glossary.update')
                  : t('glossary.save')}
            </button>
            <button
              className="btn-secondary"
              disabled={glossaryAction !== null}
              onClick={resetGlossaryForm}
            >
              {t('glossary.clearForm')}
            </button>
          </div>
        </div>
        <div className="glossary-summary">
          {t('glossary.count', { count: glossaryEntries.length })}
        </div>
        {glossaryStatus ? <p className="status-msg model-status-msg">{glossaryStatus}</p> : null}
        <div className="glossary-list">
          {glossaryEntries.length === 0 ? (
            <p className="placeholder-text">{t('glossary.empty')}</p>
          ) : (
            glossaryEntries.map((entry) => (
              <div key={entry.id} className="glossary-row">
                <div>
                  <div className="glossary-row-title">
                    <strong>{entry.canonical}</strong>
                    {entry.category ? <span>{entry.category}</span> : null}
                  </div>
                  <p className="setting-meta">
                    {entry.aliases.length > 0
                      ? t('glossary.aliases', {
                          aliases: entry.aliases.map((alias) => alias.alias).join(', '),
                        })
                      : t('glossary.noAliases')}
                  </p>
                  <p className="setting-meta">
                    {t('glossary.createdAt', {
                      time: new Date(entry.createdAt).toLocaleString(),
                    })}
                  </p>
                </div>
                <div className="model-row-actions">
                  <button
                    className="btn-secondary"
                    disabled={glossaryAction !== null}
                    onClick={() => handleEditGlossaryEntry(entry)}
                  >
                    {t('glossary.edit')}
                  </button>
                  <button
                    className="btn-secondary danger-button"
                    disabled={glossaryAction !== null}
                    onClick={() => {
                      void handleDeleteGlossaryEntry(entry);
                    }}
                  >
                    {glossaryAction === `delete:${entry.id}` ? t('glossary.deleting') : t('glossary.delete')}
                  </button>
                </div>
              </div>
            ))
          )}
        </div>
      </section>

      {/* Privacy */}
      <section id="settings-privacy" className="settings-section">
        <h3>{t('telemetry.sectionTitle')}</h3>
        <div className="setting-row">
          <div>
            <strong>{t('telemetry.settingTitle')}</strong>
            <p className="hint">{t('telemetry.settingDescription')}</p>
            <p className="setting-meta">
              {telemetryConsent.decided
                ? telemetryConsent.updatedAtMs
                  ? t('telemetry.choiceSaved', { time: new Date(telemetryConsent.updatedAtMs).toLocaleString() })
                  : t('telemetry.choiceSavedNoTime')
                : t('telemetry.noChoice')}
            </p>
          </div>
          <label className="toggle-switch">
            <input
              type="checkbox"
              checked={telemetryConsent.enabled}
              disabled={telemetrySaving}
              onChange={(e) => {
                const enabled = e.target.checked;
                setTelemetrySaving(true);
                void onTelemetryConsentChange(enabled)
                  .catch((error) => {
                    setStatusMsg(errorToMessage(error, t));
                  })
                  .finally(() => setTelemetrySaving(false));
              }}
            />
            <span className="toggle-slider" />
          </label>
        </div>

        <div className="setting-actions">
          <button
            className="btn-secondary"
            disabled={privacyAction !== null}
            onClick={() => {
              void handleClearHistory();
            }}
          >
            {privacyAction === 'history' ? t('settings.clearing') : t('settings.clearHistory')}
          </button>
          <button
            className="btn-secondary"
            disabled={privacyAction !== null}
            onClick={() => {
              void handleDeleteModelCache();
            }}
          >
            {privacyAction === 'models' ? t('settings.deleting') : t('settings.deleteModelCache')}
          </button>
          <button
            className="btn-secondary"
            disabled={privacyAction !== null}
            onClick={() => {
              void handleExportDiagnostics();
            }}
          >
            {privacyAction === 'diagnostics' ? t('settings.exporting') : t('settings.exportDiagnostics')}
          </button>
        </div>
        {diagnosticsPreview && (
          <div className="diagnostics-preview">
            <div className="info-row">
              <span>{t('settings.localHistory')}</span>
              <span>{formatFileSize(diagnosticsPreview.localHistoryBytes)}</span>
            </div>
            <div className="info-row">
              <span>{t('telemetry.events')}</span>
              <span>{formatFileSize(diagnosticsPreview.telemetryEventsBytes)}</span>
            </div>
            <div className="info-row">
              <span>{t('settings.modelCache')}</span>
              <span>
                {formatFileSize(diagnosticsPreview.modelCacheBytes)} / {t('settings.items', { count: diagnosticsPreview.modelCacheItems })}
              </span>
            </div>
            <div className="diagnostics-fields">
              {diagnosticsPreview.fields.map((field) => (
                <span key={field}>{field}</span>
              ))}
            </div>
          </div>
        )}
        {telemetryStatus && <p className="status-msg">{telemetryStatus}</p>}
        {statusMsg && <p className="status-msg">{statusMsg}</p>}
      </section>

      {/* About */}
      <section className="settings-section">
        <h3>{t('settings.about')}</h3>
        <div className="info-row"><span>{t('settings.version')}</span><span>1.0.0-alpha</span></div>
        <div className="info-row">
          <span>{t('settings.asrEngine')}</span>
          <span>
            {asrEngine === 'auto'
              ? t('import.engineAuto')
              : asrEngine === 'sensevoice'
                ? t('import.engineSenseVoice')
                : asrEngine === 'funasr'
                  ? t('import.engineFunAsr')
                : t('import.engineWhisper')}
          </span>
        </div>
        <div className="info-row"><span>{t('settings.platform')}</span><span>{t('settings.platformValue')}</span></div>
        <div className="info-row"><span>{t('settings.privacy')}</span><span>{t('settings.privacyValue')}</span></div>
      </section>
    </div>
  );
}
