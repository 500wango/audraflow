import { useCallback, useEffect, useRef, useState, type DragEvent, type FormEvent } from 'react';
import { convertFileSrc } from '@tauri-apps/api/core';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import type {
  AsrEngine,
  AudioMode,
  DeviceDiagnostics,
  EffectiveTranscriptionPlan,
  PlatformDownloadOptions,
  TranscriptionLanguage,
  UrlPreviewResponse,
  VocalSeparationMode,
} from '../types';
import {
  filePathFromDrop,
  formatFileSize,
  formatTimeInput,
  isSupportedLocalMediaPath,
  parseTimeToSeconds,
} from '../appUtils';
import { errorToMessage, hasTauriRuntime, invokeTauri, localTranscriptionUnavailableText } from '../tauri';
import { useI18n } from '../useI18n';

export function ImportPage({
  extremeAccuracy,
  onExtremeAccuracyChange,
  asrEngine,
  onAsrEngineChange,
  transcriptionLanguage,
  onTranscriptionLanguageChange,
  audioMode,
  onAudioModeChange,
  vocalSeparation,
  onVocalSeparationChange,
  onFilesSelected,
  onUrlSelected,
  estimate,
  onEstimateChange,
  transcriptionPlan,
  modelSettingsError,
  onOpenSettings,
}: {
  extremeAccuracy: boolean;
  onExtremeAccuracyChange: (v: boolean) => void;
  asrEngine: AsrEngine;
  onAsrEngineChange: (v: AsrEngine) => void;
  transcriptionLanguage: TranscriptionLanguage;
  onTranscriptionLanguageChange: (v: TranscriptionLanguage) => void;
  audioMode: AudioMode;
  onAudioModeChange: (v: AudioMode) => void;
  vocalSeparation: VocalSeparationMode;
  onVocalSeparationChange: (v: VocalSeparationMode) => void;
  onFilesSelected: (files: string[]) => void;
  onUrlSelected: (url: string, options: PlatformDownloadOptions) => void;
  estimate: { seconds: number; explanation: string } | null;
  onEstimateChange: (e: { seconds: number; explanation: string } | null) => void;
  transcriptionPlan: EffectiveTranscriptionPlan;
  modelSettingsError: string | null;
  onOpenSettings: () => void;
}) {
  const { t } = useI18n();
  const [dragOver, setDragOver] = useState(false);
  const [mediaUrl, setMediaUrl] = useState('');
  const [urlError, setUrlError] = useState<string | null>(null);
  const [audioQuality, setAudioQuality] = useState<PlatformDownloadOptions['audioQuality']>('auto');
  const [audioFormat, setAudioFormat] = useState<PlatformDownloadOptions['audioFormat']>('source');
  const [skipStartSeconds, setSkipStartSeconds] = useState('0');
  const [urlPreview, setUrlPreview] = useState<UrlPreviewResponse | null>(null);
  const [previewSrc, setPreviewSrc] = useState<string | null>(null);
  const [previewStatus, setPreviewStatus] = useState<string | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [localImportStatus, setLocalImportStatus] = useState<string | null>(null);
  const [deviceDiagnostics, setDeviceDiagnostics] = useState<DeviceDiagnostics | null>(() => (
    hasTauriRuntime()
      ? null
      : {
          cpuCores: navigator.hardwareConcurrency || 0,
          cudaAvailable: false,
          vramGb: null,
          gpuModel: null,
          cudaVersion: null,
          driverVersion: null,
          deviceTier: 'BrowserPreview',
          fallbackMessage: t('import.diagnosticsDesktop'),
        }
  ));
  const [deviceDiagnosticsError, setDeviceDiagnosticsError] = useState<string | null>(null);
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const modelReady = !modelSettingsError && transcriptionPlan.ready;
  const effectiveModel = transcriptionPlan.model;
  const planModelText = effectiveModel
    ? `${effectiveModel.name} v${effectiveModel.version} · ${formatFileSize(effectiveModel.sizeBytes)}`
    : t('model.noWhisperModelRequired');
  const planEngineText = t(`import.engine.${transcriptionPlan.engine}`);
  const missingModelText = modelSettingsError
    ? t('model.settingsUnavailable', { error: modelSettingsError })
    : transcriptionPlan.engine === 'whisper'
      ? localTranscriptionUnavailableText(t)
      : '';

  const blockMissingModel = useCallback(() => {
    setLocalImportStatus(missingModelText);
    setUrlError(missingModelText);
    return false;
  }, [missingModelText]);

  useEffect(() => {
    if (!hasTauriRuntime()) {
      return;
    }

    invokeTauri<DeviceDiagnostics>('cmd_get_device_diagnostics')
      .then((diagnostics) => {
        setDeviceDiagnostics(diagnostics);
        setDeviceDiagnosticsError(null);
      })
      .catch((error) => {
        setDeviceDiagnostics(null);
        setDeviceDiagnosticsError(errorToMessage(error, t));
      });
  }, [t]);

  const handleExtremeToggle = async (checked: boolean) => {
    onExtremeAccuracyChange(checked);
    try {
      const result = await invokeTauri<{ estimatedSeconds: number; explanation: string }>(
        'cmd_estimate_job',
        { audioDurationS: 3600, extremeAccuracy: checked }
      );
      onEstimateChange({
        seconds: result.estimatedSeconds,
        explanation: result.explanation,
      });
    } catch {
      onEstimateChange(null);
    }
  };

  const collectMediaPaths = useCallback(async (paths: string[]): Promise<string[]> => {
    const mediaFiles: string[] = [];
    let expandedFolders = 0;

    for (const path of paths) {
      try {
        const folderFiles = await invokeTauri<string[]>('cmd_scan_media_folder', { folderPath: path });
        mediaFiles.push(...folderFiles);
        expandedFolders += 1;
        continue;
      } catch {
        // Not a folder, not a Tauri runtime, or a folder without supported media.
      }

      if (isSupportedLocalMediaPath(path)) {
        mediaFiles.push(path);
      }
    }

    const uniqueFiles = [...new Set(mediaFiles)];
    if (uniqueFiles.length > 0) {
      const folderText = expandedFolders > 0
        ? t('import.folderText', { count: expandedFolders, plural: expandedFolders > 1 ? 's' : '' })
        : '';
      setLocalImportStatus(t('import.addedFiles', {
        count: uniqueFiles.length,
        plural: uniqueFiles.length > 1 ? 's' : '',
        folderText,
      }));
    } else {
      setLocalImportStatus(t('import.noSupportedFiles'));
    }
    return uniqueFiles;
  }, [t]);

  const handleDrop = async (e: DragEvent) => {
    e.preventDefault();
    setDragOver(false);
    if (!modelReady) {
      blockMissingModel();
      return;
    }
    const files: string[] = [];
    if (e.dataTransfer.files) {
      for (let i = 0; i < e.dataTransfer.files.length; i++) {
        files.push(filePathFromDrop(e.dataTransfer.files[i]));
      }
    }
    if (files.length > 0) {
      const mediaFiles = await collectMediaPaths(files);
      if (mediaFiles.length > 0) onFilesSelected(mediaFiles);
    }
  };

  const handleFilePicker = async () => {
    if (!modelReady) {
      blockMissingModel();
      return;
    }

    try {
      // Use Tauri dialog plugin
      const selected = await openDialog({
        multiple: true,
        filters: [{
          name: 'Audio/Video',
          extensions: ['mp3', 'wav', 'm4a', 'mp4', 'mov', 'aac', 'flac'],
        }],
      });
      if (selected && Array.isArray(selected)) {
        const mediaFiles = await collectMediaPaths(selected as string[]);
        if (mediaFiles.length > 0) onFilesSelected(mediaFiles);
      } else if (selected) {
        const mediaFiles = await collectMediaPaths([selected as string]);
        if (mediaFiles.length > 0) onFilesSelected(mediaFiles);
      }
    } catch (error) {
      if (hasTauriRuntime()) {
        setLocalImportStatus(errorToMessage(error, t));
        return;
      }

      const input = document.createElement('input');
      input.type = 'file';
      input.multiple = true;
      input.accept = 'audio/*,video/*';
      input.onchange = async () => {
        const files = Array.from(input.files || []).map((f) => f.name);
        const mediaFiles = await collectMediaPaths(files);
        if (mediaFiles.length > 0) onFilesSelected(mediaFiles);
      };
      input.click();
    }
  };

  const handleFolderPicker = async () => {
    if (!modelReady) {
      blockMissingModel();
      return;
    }

    try {
      const selected = await openDialog({
        directory: true,
        multiple: false,
      });
      if (!selected) return;
      const folderPath = Array.isArray(selected) ? selected[0] : selected;
      const mediaFiles = await invokeTauri<string[]>('cmd_scan_media_folder', { folderPath });
      setLocalImportStatus(t('import.addedFolderFiles', {
        count: mediaFiles.length,
        plural: mediaFiles.length > 1 ? 's' : '',
      }));
      onFilesSelected(mediaFiles);
    } catch (error) {
      if (hasTauriRuntime()) {
        setLocalImportStatus(errorToMessage(error, t));
        return;
      }

      const input = document.createElement('input');
      input.type = 'file';
      input.multiple = true;
      input.accept = 'audio/*,video/*';
      input.setAttribute('webkitdirectory', '');
      input.onchange = async () => {
        const files = Array.from(input.files || []).map((file) => file.name);
        const mediaFiles = await collectMediaPaths(files);
        if (mediaFiles.length > 0) onFilesSelected(mediaFiles);
      };
      input.click();
    }
  };

  const handleUrlSubmit = (e: FormEvent) => {
    e.preventDefault();
    const trimmed = mediaUrl.trim();
    if (!modelReady) {
      setUrlError(localTranscriptionUnavailableText(t));
      return;
    }
    if (!trimmed) {
      setUrlError(t('import.enterUrl'));
      return;
    }

    try {
      const url = new URL(trimmed);
      if (url.protocol !== 'http:' && url.protocol !== 'https:') {
        setUrlError(t('import.httpOnly'));
        return;
      }
    } catch {
      setUrlError(t('import.validUrl'));
      return;
    }

    const skipSeconds = parseTimeToSeconds(skipStartSeconds);
    if (skipSeconds === null || skipSeconds < 0) {
      setUrlError(t('import.skipInvalid'));
      return;
    }

    setUrlError(null);
    setMediaUrl('');
    onUrlSelected(trimmed, {
      audioQuality,
      audioFormat,
      skipStartSeconds: Math.min(skipSeconds, 43200),
      asrEngine,
      language: transcriptionLanguage,
      audioMode,
      vocalSeparation: audioMode === 'music' ? vocalSeparation : 'off',
    });
  };

  const handlePreviewUrl = async () => {
    const trimmed = mediaUrl.trim();
    if (!trimmed) {
      setUrlError(t('import.urlBeforePreview'));
      return;
    }

    try {
      const url = new URL(trimmed);
      if (url.protocol !== 'http:' && url.protocol !== 'https:') {
        setUrlError(t('import.httpOnly'));
        return;
      }
    } catch {
      setUrlError(t('import.validUrl'));
      return;
    }

    setUrlError(null);
    setPreviewLoading(true);
    setPreviewStatus(t('import.creatingPreview'));
    setUrlPreview(null);
    setPreviewSrc(null);

    try {
      const preview = await invokeTauri<UrlPreviewResponse>('cmd_create_url_preview', {
        request: {
          url: trimmed,
          previewSeconds: 120,
        },
      });
      setUrlPreview(preview);
      setPreviewSrc(convertFileSrc(preview.filePath));
      setPreviewStatus(preview.message);
    } catch (error) {
      setPreviewStatus(null);
      setUrlError(errorToMessage(error, t));
    } finally {
      setPreviewLoading(false);
    }
  };

  const handleUseCurrentPreviewTime = () => {
    const current = audioRef.current?.currentTime ?? 0;
    setSkipStartSeconds(formatTimeInput(current));
    setPreviewStatus(t('import.skipPointSet', { time: formatTimeInput(current) }));
  };

  const previewDetails = [
    urlPreview?.source ? t('import.source', { source: urlPreview.source }) : null,
    urlPreview?.previewSeconds
      ? t('import.previewSeconds', { seconds: Math.round(urlPreview.previewSeconds) })
      : null,
  ].filter(Boolean).join(' · ');
  const cpuCoreCount = deviceDiagnostics?.cpuCores || navigator.hardwareConcurrency || 0;

  return (
    <div className="page import-page">
      <div className={`model-readiness ${modelReady ? 'model-ready' : 'model-missing'}`}>
        <div>
          <strong>{modelReady ? t('model.ready') : t('model.notSelected')}</strong>
          <p>
            {modelSettingsError
              ? t('model.settingsUnavailable', { error: modelSettingsError })
              : t(transcriptionPlan.reasonKey)}
          </p>
          <div className="model-readiness-details">
            <span>
              <small>{t('model.effectiveEngine')}</small>
              <b>{planEngineText}</b>
            </span>
            <span>
              <small>{t('model.effectiveModel')}</small>
              <b>{modelReady ? planModelText : t('model.selectBeforeJobs')}</b>
            </span>
          </div>
        </div>
        <button className="btn-secondary" type="button" onClick={onOpenSettings}>
          {t('nav.settings')}
        </button>
      </div>
      <div
        className={`drop-zone ${dragOver ? 'drag-over' : ''} ${modelReady ? '' : 'drop-disabled'}`}
        onDragOver={(e) => { e.preventDefault(); setDragOver(true); }}
        onDragLeave={() => setDragOver(false)}
        onDrop={handleDrop}
      >
        <div className="drop-zone-content">
          <svg className="drop-icon" viewBox="0 0 48 48" width="48" height="48">
            <path d="M24 4L14 14h6v16h8V14h6L24 4zM8 30v6c0 2.2 1.8 4 4 4h24c2.2 0 4-1.8 4-4v-6h-4v6H12v-6H8z" fill="currentColor"/>
          </svg>
          <h2>{t('import.dropTitle')}</h2>
          <p>{t('import.supportedFormats')}</p>
          <div className="local-import-actions">
            <button className="btn-primary" type="button" onClick={handleFilePicker}>
              {t('import.chooseFiles')}
            </button>
            <button className="btn-secondary" type="button" onClick={handleFolderPicker}>
              {t('import.chooseFolder')}
            </button>
          </div>
          {localImportStatus ? <p className="local-import-status">{localImportStatus}</p> : null}
        </div>
      </div>

      <form className="url-import" onSubmit={handleUrlSubmit}>
        <label htmlFor="media-url">{t('import.mediaUrl')}</label>
        <div className="url-import-row">
          <input
            id="media-url"
            type="url"
            value={mediaUrl}
            onChange={(e) => {
              setMediaUrl(e.target.value);
              setUrlError(null);
            }}
            placeholder="https://example.com/audio.mp3"
          />
          <button
            className="btn-secondary"
            type="button"
            onClick={handlePreviewUrl}
            disabled={previewLoading}
          >
            {previewLoading ? t('import.previewing') : t('import.previewStart')}
          </button>
          <button className="btn-primary" type="submit" disabled={!modelReady}>
            {t('import.addLink')}
          </button>
        </div>
        {previewSrc ? (
          <div className="url-preview-panel">
            <audio
              ref={audioRef}
              controls
              preload="metadata"
              src={previewSrc}
            />
            <div className="url-preview-actions">
              <button className="btn-secondary" type="button" onClick={handleUseCurrentPreviewTime}>
                {t('import.useCurrentTime')}
              </button>
              <span>{previewDetails}</span>
            </div>
          </div>
        ) : null}
        <div className="url-options-row">
          <label>
            {t('import.quality')}
            <select value={audioQuality} onChange={(e) => setAudioQuality(e.target.value as PlatformDownloadOptions['audioQuality'])}>
              <option value="auto">{t('import.qualityAuto')}</option>
              <option value="small">{t('import.qualitySmall')}</option>
              <option value="medium">{t('import.qualityMedium')}</option>
              <option value="best">{t('import.qualityBest')}</option>
            </select>
          </label>
          <label>
            {t('import.format')}
            <select value={audioFormat} onChange={(e) => setAudioFormat(e.target.value as PlatformDownloadOptions['audioFormat'])}>
              <option value="source">{t('import.sourceFormat')}</option>
              <option value="m4a">M4A</option>
              <option value="mp3">MP3</option>
              <option value="wav">WAV</option>
            </select>
          </label>
          <label>
            {t('import.skipFirst')}
            <input
              className="skip-input"
              type="text"
              inputMode="numeric"
              value={skipStartSeconds}
              onChange={(e) => setSkipStartSeconds(e.target.value)}
              placeholder="0 or 1:30"
            />
            <span>{t('import.skipUnit')}</span>
          </label>
          <div className="skip-presets" aria-label={t('import.skipPresets')}>
            {[15, 30, 60, 90].map((seconds) => (
              <button
                key={seconds}
                type="button"
                onClick={() => setSkipStartSeconds(formatTimeInput(seconds))}
              >
                {seconds}s
              </button>
            ))}
          </div>
        </div>
        {urlError ? (
          <p className="hint error-text">{urlError}</p>
        ) : previewStatus ? (
          <p className="hint">{previewStatus}</p>
        ) : (
          <p className="hint">{t('import.previewHint')}</p>
        )}
      </form>

      <div className="import-options">
        <label>
          {t('import.asrEngine')}
          <select value={asrEngine} onChange={(event) => onAsrEngineChange(event.target.value as AsrEngine)}>
            <option value="auto">{t('import.engineAuto')}</option>
            <option value="sensevoice">{t('import.engineSenseVoice')}</option>
            <option value="whisper">{t('import.engineWhisper')}</option>
            <option value="funasr">{t('import.engineFunAsr')}</option>
          </select>
        </label>
        <label>
          {t('import.audioLanguage')}
          <select
            value={transcriptionLanguage}
            onChange={(event) => onTranscriptionLanguageChange(event.target.value as TranscriptionLanguage)}
          >
            <option value="auto">{t('import.languageAuto')}</option>
            <option value="zh">{t('import.languageChinese')}</option>
            <option value="en">{t('import.languageEnglish')}</option>
          </select>
        </label>
        <label className="checkbox-label">
          <input
            type="checkbox"
            checked={extremeAccuracy}
            onChange={(e) => handleExtremeToggle(e.target.checked)}
          />
          <span>{t('import.extremeAccuracy')}</span>
        </label>
        <label className="checkbox-label">
          <input
            type="checkbox"
            checked={audioMode === 'music'}
            onChange={(e) => onAudioModeChange(e.target.checked ? 'music' : 'speech')}
          />
          <span>{t('import.lyricsMode')}</span>
        </label>
        {audioMode === 'music' ? (
          <label className="checkbox-label" title={t('import.vocalSeparationHint')}>
            <input
              type="checkbox"
              checked={vocalSeparation === 'demucs'}
              onChange={(e) => onVocalSeparationChange(e.target.checked ? 'demucs' : 'off')}
            />
            <span>{t('import.vocalSeparation')}</span>
          </label>
        ) : null}
        {estimate ? (
          <p className="hint estimate-detail">
            {t('import.estimate', {
              minutes: Math.round(estimate.seconds / 60),
              explanation: estimate.explanation,
            })}
          </p>
        ) : (
          <p className="hint">
            {t('import.speedHint')}
          </p>
        )}
      </div>

      <div className="device-diagnostics">
        <h3>{t('import.deviceDiagnostics')}</h3>
        <div className="diag-grid">
          <div className="diag-item">
            <span className="diag-label">{t('import.cuda')}</span>
            <span className="diag-value">
              {deviceDiagnostics
                ? (deviceDiagnostics.cudaAvailable ? t('import.available') : t('import.notDetected'))
                : t('import.detecting')}
            </span>
          </div>
          <div className="diag-item">
            <span className="diag-label">{t('import.vram')}</span>
            <span className="diag-value">
              {deviceDiagnostics?.vramGb ? `${deviceDiagnostics.vramGb.toFixed(1)} GB` : '--'}
            </span>
          </div>
          <div className="diag-item">
            <span className="diag-label">{t('import.cpu')}</span>
            <span className="diag-value">
              {cpuCoreCount ? t('import.cores', { count: cpuCoreCount }) : '--'}
            </span>
          </div>
          <div className="diag-item">
            <span className="diag-label">{t('import.deviceTier')}</span>
            <span className="diag-value">{deviceDiagnostics?.deviceTier || '--'}</span>
          </div>
          <div className="diag-item">
            <span className="diag-label">{t('import.gpu')}</span>
            <span className="diag-value">{deviceDiagnostics?.gpuModel || t('import.cpuFallback')}</span>
          </div>
          <div className="diag-item">
            <span className="diag-label">{t('import.driverCuda')}</span>
            <span className="diag-value">
              {deviceDiagnostics
                ? [deviceDiagnostics.driverVersion, deviceDiagnostics.cudaVersion].filter(Boolean).join(' / ') || '--'
                : '--'}
            </span>
          </div>
        </div>
        {deviceDiagnostics?.fallbackMessage ? (
          <p className="hint warning-text">{deviceDiagnostics.fallbackMessage}</p>
        ) : deviceDiagnosticsError ? (
          <p className="hint error-text">{deviceDiagnosticsError}</p>
        ) : null}
      </div>
    </div>
  );
}
