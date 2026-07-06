import { useCallback, useEffect, useState, type MouseEvent } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { listen } from '@tauri-apps/api/event';
import './App.css';
import { AppNavigation } from './components/AppNavigation';
import { TelemetryConsentDialog } from './components/TelemetryConsentDialog';
import { ImportPage } from './pages/ImportPage';
import { QueuePage } from './pages/QueuePage';
import { EditorPage } from './pages/EditorPage';
import { ExportPage } from './pages/ExportPage';
import { SettingsPage } from './pages/SettingsPage';
import type {
  AsrEngine,
  AudioMode,
  BackendJobLogEvent,
  BackendJobProgressEvent,
  BackendJobStatus,
  JobInfo,
  ModelSettings,
  Page,
  PersistedJobSummary,
  PlatformDownloadOptions,
  QueueAction,
  TelemetryConsentState,
  TranscriptionLanguage,
  VocalSeparationMode,
  WindowControlAction,
} from './types';
import {
  buildEffectiveTranscriptionPlan,
  createLog,
  fileNameFromSource,
  formatDurationSeconds,
  formatFileSize,
  inspectLocalMediaFiles,
  jobPhaseFromStatus,
  jobProgressFromStatus,
  jobStatusMessage,
  mergePersistedJobs,
  persistedJobToJobInfo,
  readBrowserTelemetryConsent,
  runWithConcurrency,
  stateLabel,
  writeBrowserTelemetryConsent,
} from './appUtils';
import { errorToMessage, hasTauriRuntime, invokeTauri, localTranscriptionUnavailableText } from './tauri';
import { useI18n } from './useI18n';

function App() {
  const { t } = useI18n();
  const [currentPage, setCurrentPage] = useState<Page>('import');
  const [jobs, setJobs] = useState<JobInfo[]>([]);
  const [selectedJobId, setSelectedJobId] = useState<string | null>(null);
  const [extremeAccuracy, setExtremeAccuracy] = useState(false);
  const [asrEngine, setAsrEngine] = useState<AsrEngine>('auto');
  const [transcriptionLanguage, setTranscriptionLanguage] = useState<TranscriptionLanguage>('auto');
  const [audioMode, setAudioMode] = useState<AudioMode>('speech');
  const [vocalSeparation, setVocalSeparation] = useState<VocalSeparationMode>('off');
  const [estimate, setEstimate] = useState<{ seconds: number; explanation: string } | null>(null);
  const [telemetryConsent, setTelemetryConsent] = useState<TelemetryConsentState>({
    enabled: false,
    decided: !hasTauriRuntime(),
    updatedAtMs: null,
  });
  const [showTelemetryPrompt, setShowTelemetryPrompt] = useState(false);
  const [telemetryStatus, setTelemetryStatus] = useState<string | null>(null);
  const [modelSettings, setModelSettings] = useState<ModelSettings | null>(null);
  const [modelSettingsError, setModelSettingsError] = useState<string | null>(null);
  const selectedModel = modelSettings?.selectedModel ?? null;
  const installedModels = modelSettings?.installedModels ?? [];
  const transcriptionPlan = buildEffectiveTranscriptionPlan(
    asrEngine,
    audioMode,
    selectedModel,
    installedModels,
    extremeAccuracy
  );
  const modelUnavailableText = modelSettingsError
    ? t('model.settingsUnavailable', { error: modelSettingsError })
    : localTranscriptionUnavailableText(t);
  const transcriptionReady = modelSettingsError ? false : transcriptionPlan.ready;

  const handleAudioModeChange = useCallback((nextMode: AudioMode) => {
    setAudioMode(nextMode);
    if (nextMode !== 'music') {
      setVocalSeparation('off');
    }
  }, []);

  const handleWindowControl = useCallback((action: WindowControlAction) => {
    if (!hasTauriRuntime()) return;

    const appWindow = getCurrentWindow();
    const command =
      action === 'close'
        ? appWindow.close()
        : action === 'minimize'
          ? appWindow.minimize()
          : appWindow.toggleMaximize();

    void command.catch((error) => {
      console.warn('Window control failed', error);
    });
  }, []);

  const handleTitlebarDoubleClick = useCallback((event: MouseEvent<HTMLElement>) => {
    const target = event.target;
    if (!(target instanceof Element)) return;
    if (target.closest('button, input, select, textarea, a')) return;
    handleWindowControl('zoom');
  }, [handleWindowControl]);

  const refreshModelSettings = useCallback(() => {
    void invokeTauri<ModelSettings>('cmd_get_model_settings')
      .then((settings) => {
        setModelSettings(settings);
        setModelSettingsError(null);
      })
      .catch((error) => {
        setModelSettings(null);
        setModelSettingsError(errorToMessage(error, t));
      });
  }, [t]);
  const updateModelSettings = useCallback((settings: ModelSettings | null) => {
    setModelSettings(settings);
    if (settings) {
      setModelSettingsError(null);
    }
  }, []);

  const refreshJobsFromStorage = useCallback(async () => {
    const persistedJobs = await invokeTauri<PersistedJobSummary[]>('cmd_list_jobs', { limit: 100 });
    const restored = persistedJobs.map((job) => persistedJobToJobInfo(job, t));
    setJobs((prev) => mergePersistedJobs(prev, restored));
    return restored.length;
  }, [t]);

  const applyJobStatus = useCallback((jobId: string, status: BackendJobStatus) => {
    setJobs((prev) => prev.map((job) => {
      if (job.jobId !== jobId) return job;
      const message = jobStatusMessage(status, t);
      const last = job.logs[job.logs.length - 1];
      const shouldAppend = !last || last.message !== message;
      const isTerminal = ['completed', 'cancelled', 'failed', 'notFound'].includes(status.state);
      const isRetryReset = status.state === 'pending' && ['cancelled', 'failed', 'notFound'].includes(job.state);
      const now = Date.now();

      return {
        ...job,
        state: status.state,
        progress: jobProgressFromStatus(status),
        phase: jobPhaseFromStatus(status, job.phase),
        activity: message,
        error: status.state === 'failed' ? message : undefined,
        createdAtMs: isRetryReset ? now : job.createdAtMs,
        completedAtMs: isTerminal ? job.completedAtMs ?? now : undefined,
        rtfCurrent: status.rtfCurrent ?? job.rtfCurrent,
        ttfvS: status.ttfvS ?? job.ttfvS,
        logs: shouldAppend
          ? [...job.logs, createLog(status.state === 'failed' ? 'error' : 'info', message)].slice(-20)
          : job.logs,
      };
    }));
  }, [t]);

  const updateTelemetryConsent = useCallback(async (enabled: boolean) => {
    const applyState = (state: TelemetryConsentState) => {
      setTelemetryConsent(state);
      setShowTelemetryPrompt(false);
      setTelemetryStatus(enabled ? t('telemetry.enabled') : t('telemetry.disabled'));
      window.setTimeout(() => setTelemetryStatus(null), 3000);
      return state;
    };

    if (!hasTauriRuntime()) {
      return applyState(writeBrowserTelemetryConsent(enabled));
    }

    const state = await invokeTauri<TelemetryConsentState>('cmd_set_telemetry_consent', {
      request: { enabled },
    });
    return applyState(state);
  }, [t]);

  useEffect(() => {
    refreshModelSettings();
    if (!hasTauriRuntime()) {
      const storedConsent = readBrowserTelemetryConsent();
      if (storedConsent) {
        setTelemetryConsent(storedConsent);
        setShowTelemetryPrompt(false);
      } else {
        setTelemetryConsent({ enabled: false, decided: false, updatedAtMs: null });
        setShowTelemetryPrompt(true);
      }
      return;
    }

    void invokeTauri<TelemetryConsentState>('cmd_get_telemetry_consent')
      .then((state) => {
        setTelemetryConsent(state);
        setShowTelemetryPrompt(!state.decided);
      })
      .catch(() => {
        setTelemetryConsent({ enabled: false, decided: false, updatedAtMs: null });
        setShowTelemetryPrompt(true);
      });
  }, [refreshModelSettings]);

  useEffect(() => {
    void refreshJobsFromStorage().catch(() => {});
  }, [refreshJobsFromStorage]);

  const handleFilesSelected = useCallback((files: string[]) => {
    if (!transcriptionReady) {
      const now = Date.now();
      setJobs((prev) => [...prev, {
        jobId: `setup-${now}`,
        fileName: t('model.setup'),
        duration: '--:--',
        durationSeconds: null,
        state: 'failed',
        progress: 0,
        phase: 'setup',
        activity: modelUnavailableText,
        error: modelUnavailableText,
        createdAtMs: now,
        completedAtMs: now,
        logs: [createLog('error', modelUnavailableText)],
      }]);
      setCurrentPage('queue');
      return;
    }

    void (async () => {
      const mediaFiles = await inspectLocalMediaFiles(files);
      if (mediaFiles.length === 0) return;

      const createdAtMs = Date.now();
      const placeholders: JobInfo[] = mediaFiles.map((file, i) => ({
        jobId: `local-${Date.now()}-${i}`,
        fileName: file.fileName || fileNameFromSource(file.filePath),
        duration: formatDurationSeconds(file.durationSeconds),
        durationSeconds: file.durationSeconds,
        format: file.format || undefined,
        size: formatFileSize(file.sizeBytes),
        state: 'pending',
        progress: 0,
        phase: 'import',
        activity: t('import.creatingLocalJob'),
        createdAtMs,
        logs: [createLog('info', t('import.creatingLocalJob'))],
      }));
      setJobs((prev) => [...prev, ...placeholders]);
      setCurrentPage('queue');

      await runWithConcurrency(mediaFiles, 4, async (file, i) => {
        const placeholderId = placeholders[i].jobId;
        try {
          const status = await invokeTauri<BackendJobStatus>('cmd_create_job', {
            filePath: file.filePath,
            fileHash: '',
            asrEngine,
            language: transcriptionLanguage,
            audioMode,
            vocalSeparation: audioMode === 'music' ? vocalSeparation : 'off',
            extremeAccuracy,
            exportFormats: ['markdown'],
          });
          setJobs((prev) => prev.map((job) =>
            job.jobId === placeholderId
              ? {
                  ...job,
                  jobId: status.jobId,
                  state: status.state,
                  progress: status.progressPct,
                  activity: status.message || (status.state === 'pending' ? t('import.queuedWaiting') : stateLabel(status.state, t)),
                  logs: [...job.logs, createLog('info', t('import.jobQueued'))],
                }
              : job
          ));
        } catch (error) {
          const message = errorToMessage(error, t);
          setJobs((prev) => prev.map((job) =>
            job.jobId === placeholderId
              ? {
                  ...job,
                  state: 'failed',
                  activity: t('import.localJobFailed'),
                  error: message,
                  completedAtMs: Date.now(),
                  logs: [...job.logs, createLog('error', message)],
                }
              : job
          ));
        }
      });
    })();
  }, [asrEngine, audioMode, extremeAccuracy, modelUnavailableText, t, transcriptionLanguage, transcriptionReady, vocalSeparation]);

  const handleUrlSelected = useCallback((url: string, options: PlatformDownloadOptions) => {
    if (!transcriptionReady) {
      const now = Date.now();
      setJobs((prev) => [...prev, {
        jobId: `setup-${now}`,
        fileName: fileNameFromSource(url),
        duration: '--:--',
        durationSeconds: null,
        state: 'failed',
        progress: 0,
        phase: 'setup',
        activity: modelUnavailableText,
        error: modelUnavailableText,
        createdAtMs: now,
        completedAtMs: now,
        logs: [createLog('error', modelUnavailableText)],
      }]);
      setCurrentPage('queue');
      return;
    }

    const placeholder: JobInfo = {
      jobId: `url-${Date.now()}`,
      fileName: fileNameFromSource(url),
      duration: '--:--',
      durationSeconds: null,
      state: 'pending',
      progress: 0,
      phase: 'import',
      activity: t('import.urlStarting'),
      createdAtMs: Date.now(),
      logs: [createLog('info', t('import.urlSubmitted'))],
    };

    setJobs((prev) => [...prev, placeholder]);
    setCurrentPage('queue');

    void (async () => {
      try {
        const status = await invokeTauri<BackendJobStatus>('cmd_create_job_from_url', {
          request: {
            clientJobId: placeholder.jobId,
            url,
            audioQuality: options.audioQuality,
            audioFormat: options.audioFormat,
            skipStartSeconds: options.skipStartSeconds,
            asrEngine: options.asrEngine,
            language: options.language,
            audioMode: options.audioMode,
            vocalSeparation: options.vocalSeparation,
            extremeAccuracy,
            exportFormats: ['markdown'],
          },
        });
        setJobs((prev) => prev.map((job) =>
          job.jobId === placeholder.jobId
            ? {
                ...job,
                jobId: status.jobId,
                state: status.state,
                progress: Math.max(job.progress, status.progressPct),
                phase: job.phase,
                activity: status.message || (status.state === 'pending' ? t('import.queuedWaiting') : stateLabel(status.state, t)),
              }
            : job
        ));
      } catch (error) {
        setJobs((prev) => prev.map((job) =>
          job.jobId === placeholder.jobId
            ? {
                ...job,
                state: 'failed',
                activity: t('import.urlFailed'),
                error: errorToMessage(error, t),
                completedAtMs: Date.now(),
                logs: [
                  ...job.logs,
                  createLog('error', errorToMessage(error, t)),
                ],
              }
            : job
        ));
      }
    })();
  }, [extremeAccuracy, modelUnavailableText, t, transcriptionReady]);

  const handleQueueAction = useCallback(async (jobId: string, action: QueueAction) => {
    if (action === 'open') {
      setSelectedJobId(jobId);
      setCurrentPage('editor');
      return;
    }

    const commandByAction: Record<Exclude<QueueAction, 'open'>, string> = {
      pause: 'cmd_pause_job',
      resume: 'cmd_resume_job',
      cancel: 'cmd_cancel_job',
      retry: 'cmd_retry_job',
      skip: 'cmd_skip_job',
    };
    const actionLabel = t(`queue.action.${action}`);
    const actionRequested = t('queue.actionRequested', { action: actionLabel });

    setJobs((prev) => prev.map((job) =>
      job.jobId === jobId
        ? {
            ...job,
            activity: actionRequested,
            logs: [...job.logs, createLog('info', actionRequested)].slice(-20),
          }
        : job
    ));

    try {
      const status = await invokeTauri<BackendJobStatus>(commandByAction[action], { jobId });
      if (status.state === 'notFound' && status.message?.startsWith('Job cannot')) {
        throw new Error(status.message);
      }
      applyJobStatus(jobId, status);
    } catch (error) {
      setJobs((prev) => prev.map((job) =>
        job.jobId === jobId
          ? {
              ...job,
              activity: errorToMessage(error, t),
              logs: [...job.logs, createLog('error', errorToMessage(error, t))].slice(-20),
            }
          : job
      ));
    }
  }, [applyJobStatus, t]);

  useEffect(() => {
    if (!hasTauriRuntime()) return;

    let unlisten: (() => void) | undefined;
    void listen<BackendJobLogEvent>('job://log', (event) => {
      setJobs((prev) => prev.map((job) => {
        if (job.jobId !== event.payload.jobId) return job;
        const last = job.logs[job.logs.length - 1];
        if (last?.level === event.payload.level && last.message === event.payload.message) {
          return {
            ...job,
            activity: event.payload.message,
          };
        }
        return {
          ...job,
          activity: event.payload.message,
          logs: [
            ...job.logs,
            createLog(event.payload.level, event.payload.message),
          ].slice(-20),
        };
      }));
    }).then((cleanup) => {
      unlisten = cleanup;
    });

    let unlistenProgress: (() => void) | undefined;
    void listen<BackendJobProgressEvent>('job://progress', (event) => {
      setJobs((prev) => prev.map((job) => {
        if (job.jobId !== event.payload.jobId) return job;
        return {
          ...job,
          phase: event.payload.phase,
          progress: event.payload.progressPct,
          activity: event.payload.message,
        };
      }));
    }).then((cleanup) => {
      unlistenProgress = cleanup;
    });

    return () => {
      unlisten?.();
      unlistenProgress?.();
    };
  }, []);

  useEffect(() => {
    if (!hasTauriRuntime()) return;

    const activeJobIds = jobs
      .filter((job) => !job.jobId.startsWith('url-'))
      .filter((job) => !['completed', 'cancelled', 'failed', 'notFound'].includes(job.state))
      .map((job) => job.jobId);

    if (activeJobIds.length === 0) return;

    const interval = window.setInterval(() => {
      for (const jobId of activeJobIds) {
        void invokeTauri<BackendJobStatus>('cmd_get_job_status', { jobId })
          .then((status) => {
            applyJobStatus(jobId, status);
          })
          .catch((error) => {
            setJobs((prev) => prev.map((job) =>
              job.jobId === jobId
                ? {
                    ...job,
                    activity: errorToMessage(error, t),
                  }
                : job
            ));
          });
      }
    }, 1000);

    return () => window.clearInterval(interval);
  }, [applyJobStatus, jobs, t]);

  return (
    <div className="app">
      <header
        className="app-header"
        data-tauri-drag-region
        onDoubleClick={handleTitlebarDoubleClick}
      >
        <div className="window-chrome-controls">
          <button
            type="button"
            className="window-dot window-dot-close"
            aria-label={t('window.close')}
            title={t('window.close')}
            onClick={() => handleWindowControl('close')}
          />
          <button
            type="button"
            className="window-dot window-dot-minimize"
            aria-label={t('window.minimize')}
            title={t('window.minimize')}
            onClick={() => handleWindowControl('minimize')}
          />
          <button
            type="button"
            className="window-dot window-dot-zoom"
            aria-label={t('window.zoom')}
            title={t('window.zoom')}
            onClick={() => handleWindowControl('zoom')}
          />
        </div>
        <h1 className="app-title" data-tauri-drag-region>AudraFlow</h1>
        <AppNavigation
          className="app-nav-mobile"
          currentPage={currentPage}
          jobsCount={jobs.length}
          onNavigate={setCurrentPage}
        />
      </header>

      <div className="app-body">
        <aside className="app-sidebar">
          <AppNavigation
            className="app-nav-sidebar"
            currentPage={currentPage}
            jobsCount={jobs.length}
            onNavigate={setCurrentPage}
          />
        </aside>

        <main className="app-main">
          {currentPage === 'import' && (
            <ImportPage
              extremeAccuracy={extremeAccuracy}
              onExtremeAccuracyChange={setExtremeAccuracy}
              asrEngine={asrEngine}
              onAsrEngineChange={setAsrEngine}
              transcriptionLanguage={transcriptionLanguage}
              onTranscriptionLanguageChange={setTranscriptionLanguage}
              audioMode={audioMode}
              onAudioModeChange={handleAudioModeChange}
              vocalSeparation={vocalSeparation}
              onVocalSeparationChange={setVocalSeparation}
              onFilesSelected={handleFilesSelected}
              onUrlSelected={handleUrlSelected}
              estimate={estimate}
              onEstimateChange={setEstimate}
              transcriptionPlan={transcriptionPlan}
              modelSettingsError={modelSettingsError}
              onOpenSettings={() => setCurrentPage('settings')}
            />
          )}
          {currentPage === 'queue' && (
            <QueuePage
              jobs={jobs}
              onImport={() => setCurrentPage('import')}
              onJobAction={handleQueueAction}
              onRefreshJobs={refreshJobsFromStorage}
            />
          )}
          {currentPage === 'editor' && (
            <EditorPage jobId={selectedJobId} />
          )}
          {currentPage === 'export' && (
            <ExportPage jobs={jobs} />
          )}
          {currentPage === 'settings' && (
            <SettingsPage
              asrEngine={asrEngine}
              telemetryConsent={telemetryConsent}
              telemetryStatus={telemetryStatus}
              onTelemetryConsentChange={updateTelemetryConsent}
              modelSettings={modelSettings}
              modelSettingsError={modelSettingsError}
              onModelSettingsChange={updateModelSettings}
              onRefreshModelSettings={refreshModelSettings}
            />
          )}
        </main>
      </div>

      <footer className="app-footer">
        <span>AudraFlow v0.1.0-alpha</span>
        <span>{t('app.footerTagline')}</span>
      </footer>
      {showTelemetryPrompt && (
        <TelemetryConsentDialog
          onChoose={updateTelemetryConsent}
        />
      )}
    </div>
  );
}


export default App;
