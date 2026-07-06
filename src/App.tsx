import { useRef, useState, useCallback, useEffect, useMemo } from 'react';
import { convertFileSrc, invoke, isTauri as detectTauriRuntime } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
import { listen } from '@tauri-apps/api/event';
import { type AppLanguage } from './i18nContext';
import { useI18n } from './useI18n';
import './App.css';

type TauriRuntimeWindow = Window & {
  __TAURI_INTERNALS__?: { invoke?: unknown };
  isTauri?: boolean;
};

function hasTauriRuntime(): boolean {
  if (typeof window === 'undefined') return false;
  const tauriWindow = window as TauriRuntimeWindow;
  return (
    detectTauriRuntime() ||
    tauriWindow.isTauri === true ||
    typeof tauriWindow.__TAURI_INTERNALS__?.invoke === 'function'
  );
}

type Translate = (key: string, params?: Record<string, string | number>) => string;

function localTranscriptionUnavailableText(t: Translate): string {
  return hasTauriRuntime() ? t('model.required') : t('model.desktopRequired');
}

async function invokeTauri<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    if (!hasTauriRuntime() && isMissingTauriRuntimeError(error)) {
      throw new Error('This action requires the Tauri desktop app window, not the browser page.', {
        cause: error,
      });
    }
    throw error;
  }
}

function rawErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function errorToMessage(error: unknown, t?: Translate): string {
  const message = rawErrorMessage(error);
  return t ? friendlyErrorMessage(message, t) : message;
}

function friendlyErrorMessage(message: string, t: Translate): string {
  const normalized = message.toLowerCase();
  const withDetail = (key: string) => t(key, { detail: message });
  const missingExecutable =
    normalized.includes('was not found') ||
    normalized.includes('is required') ||
    normalized.includes('no such file') ||
    normalized.includes('os error 2');

  if (normalized.includes('command plugin:dialog|save not allowed by acl') ||
      normalized.includes('plugin:dialog|save') ||
      (normalized.includes('not allowed by acl') && normalized.includes('dialog'))) {
    return withDetail('error.dialogAcl');
  }
  if (normalized.includes('this action requires the tauri desktop app') ||
      normalized.includes('tauri desktop app window') ||
      isMissingTauriRuntimeMessage(normalized)) {
    return withDetail('error.desktopRequired');
  }
  if (normalized.includes('no space left') ||
      normalized.includes('disk full') ||
      normalized.includes('not enough space') ||
      normalized.includes('磁盘空间')) {
    return withDetail('error.diskSpace');
  }
  if (normalized.includes('permission denied') ||
      normalized.includes('access is denied') ||
      normalized.includes('eacces') ||
      normalized.includes('权限不够')) {
    return withDetail('error.permissionDenied');
  }
  if (normalized.includes('yt-dlp is required') ||
      normalized.includes('yt-dlp was not found') ||
      (normalized.includes('failed to run yt-dlp') && missingExecutable) ||
      normalized.includes('audraflow_yt_dlp_bin')) {
    return withDetail('error.ytDlpMissing');
  }
  if (normalized.includes('url returned an html page')) {
    return withDetail('error.directUrlHtml');
  }
  if (normalized.includes('yt-dlp could not download') ||
      normalized.includes('http error 403') ||
      normalized.includes('403: forbidden') ||
      normalized.includes('content is not available') ||
      normalized.includes('latest version of youtube') ||
      normalized.includes('platform download failed')) {
    return withDetail('error.platformDownloadFailed');
  }
  if (normalized.includes('timed out') || normalized.includes('timeout')) {
    return withDetail('error.timeout');
  }
  if (normalized.includes('ffprobe') &&
      (missingExecutable || normalized.includes('audraflow_ffprobe_bin'))) {
    return withDetail('error.ffprobeMissing');
  }
  if (normalized.includes('ffmpeg') &&
      (missingExecutable || normalized.includes('audraflow_ffmpeg_bin'))) {
    return withDetail('error.ffmpegMissing');
  }
  if (normalized.includes('selected asr model file is missing')) {
    return withDetail('error.modelFileMissing');
  }
  if (normalized.includes('no asr model is selected')) {
    return withDetail('error.noModelSelected');
  }
  if (normalized.includes('whisper cli') || normalized.includes('whisper-cli')) {
    return withDetail('error.whisperCliMissing');
  }
  if (normalized.includes('sensevoice requires python')) {
    return withDetail('error.sensevoicePythonMissing');
  }
  if (normalized.includes('funasr and modelscope') ||
      normalized.includes('sensevoice requires python packages')) {
    return withDetail('error.sensevoicePackagesMissing');
  }
  if (normalized.includes('fun-asr') || normalized.includes('funasr')) {
    if (normalized.includes('gguf') || normalized.includes('model')) {
      return withDetail('error.funasrModelsMissing');
    }
    return withDetail('error.funasrCliMissing');
  }
  if (normalized.includes('failed to download') ||
      normalized.includes('download failed') ||
      normalized.includes('connection refused') ||
      normalized.includes('network')) {
    return withDetail('error.networkDownload');
  }

  return message;
}

function isMissingTauriRuntimeMessage(message: string): boolean {
  return (
    message.includes('__tauri') ||
    message.includes('tauri internals') ||
    message.includes('not in a tauri') ||
    message.includes('invoke is not a function') ||
    message.includes('cannot read properties of undefined')
  );
}

function isMissingTauriRuntimeError(error: unknown): boolean {
  const message = errorToMessage(error).toLowerCase();
  return isMissingTauriRuntimeMessage(message);
}

// ── Page Routing ──────────────────────────────────────────────────────────

type Page = 'import' | 'queue' | 'editor' | 'export' | 'settings';
type WindowControlAction = 'close' | 'minimize' | 'zoom';

// ── Types ─────────────────────────────────────────────────────────────────

interface JobInfo {
  jobId: string;
  fileName: string;
  duration: string;
  durationSeconds?: number | null;
  format?: string;
  size?: string;
  state: 'pending' | 'running' | 'paused' | 'completed' | 'cancelled' | 'failed' | 'notFound';
  progress: number;
  phase?: string;
  activity?: string;
  error?: string;
  createdAtMs: number;
  completedAtMs?: number;
  rtfCurrent?: number | null;
  ttfvS?: number | null;
  logs: JobLogEntry[];
}

type QueueAction = 'pause' | 'resume' | 'cancel' | 'retry' | 'skip' | 'open';

interface BackendJobStatus {
  jobId: string;
  state: JobInfo['state'];
  progressPct: number;
  message?: string | null;
  estimatedRemainingS?: number | null;
  rtfCurrent?: number | null;
  ttfvS?: number | null;
}

interface JobLogEntry {
  id: string;
  time: string;
  level: 'info' | 'warn' | 'error';
  message: string;
}

interface BackendJobLogEvent {
  jobId: string;
  level: JobLogEntry['level'];
  message: string;
}

interface BackendJobProgressEvent {
  jobId: string;
  phase: string;
  progressPct: number;
  message: string;
}

interface ModelDownloadProgressEvent {
  id: string;
  downloadedBytes: number;
  totalBytes: number;
  progressPct: number;
  message: string;
}

interface UrlPreviewResponse {
  filePath: string;
  previewSeconds: number;
  source: string;
  message: string;
}

interface MediaFileInfo {
  filePath: string;
  fileName: string;
  format: string;
  sizeBytes: number;
  durationSeconds?: number | null;
}

type AudioMode = 'speech' | 'music';
type AsrEngine = 'auto' | 'sensevoice' | 'whisper' | 'funasr';
type TranscriptionLanguage = 'auto' | 'zh' | 'en';
type VocalSeparationMode = 'off' | 'demucs';

interface PersistedJobSummary {
  jobId: string;
  filePath: string;
  fileName: string;
  format: string;
  sizeBytes: number;
  durationSeconds?: number | null;
  state: string;
  extremeAccuracy: boolean;
  segmentCount: number;
  createdAt: string;
  completedAt?: string | null;
}

interface TranscriptResponse {
  jobId: string;
  filePath: string;
  mediaSrcPath: string;
  segments: TranscriptSegment[];
}

interface PlatformDownloadOptions {
  audioQuality: 'auto' | 'small' | 'medium' | 'best';
  audioFormat: 'source' | 'mp3' | 'm4a' | 'wav';
  skipStartSeconds: number;
  asrEngine: AsrEngine;
  language: TranscriptionLanguage;
  audioMode: AudioMode;
  vocalSeparation: VocalSeparationMode;
}

interface TelemetryConsentState {
  enabled: boolean;
  decided: boolean;
  updatedAtMs?: number | null;
}

interface PrivacyActionResult {
  message: string;
  bytesFreed: number;
  itemsAffected: number;
}

interface DiagnosticsPreview {
  fields: string[];
  localHistoryBytes: number;
  telemetryEventsBytes: number;
  modelCacheBytes: number;
  modelCacheItems: number;
  telemetryEnabled: boolean;
}

interface DeviceDiagnostics {
  cpuCores: number;
  cudaAvailable: boolean;
  vramGb?: number | null;
  gpuModel?: string | null;
  cudaVersion?: string | null;
  driverVersion?: string | null;
  deviceTier: string;
  fallbackMessage?: string | null;
}

type RuntimeDependencyStatus = 'ready' | 'missing' | 'warning';
type RuntimeDependencyKind = 'required' | 'recommended' | 'optional' | 'experimental';

interface RuntimeDependency {
  id: string;
  status: RuntimeDependencyStatus;
  kind: RuntimeDependencyKind;
  path?: string | null;
  version?: string | null;
  detail?: string | null;
  repairable: boolean;
}

interface RuntimeHealth {
  generatedAtMs: number;
  blockingCount: number;
  warningCount: number;
  items: RuntimeDependency[];
}

interface RuntimeRepairResult {
  id: string;
  message: string;
  health: RuntimeHealth;
}

interface ModelInfo {
  name: string;
  version: string;
  language: string;
  sizeBytes: number;
  sha256: string;
  path: string;
  installedAtMs: number;
  selected: boolean;
  bundled: boolean;
}

interface ModelCatalogEntry {
  name: string;
  version: string;
  language: string;
  sizeBytes: number;
  sha256: string;
  downloadUrl: string;
  description: string;
  recommended: boolean;
  installed: boolean;
  selected: boolean;
}

interface ModelSettings {
  modelsDir: string;
  selectedModel?: ModelInfo | null;
  installedModels: ModelInfo[];
}

interface ModelActionResult {
  message: string;
  bytesFreed: number;
  itemsAffected: number;
  settings: ModelSettings;
}

interface EffectiveTranscriptionPlan {
  engine: AsrEngine;
  model?: ModelInfo | null;
  ready: boolean;
  reasonKey: string;
}

interface GlossaryAlias {
  id: number;
  alias: string;
  pinyin?: string | null;
}

interface GlossaryEntry {
  id: number;
  canonical: string;
  category?: string | null;
  enabled: boolean;
  createdAt: string;
  aliases: GlossaryAlias[];
}

type TelemetryEventPayload = Record<string, string | number | boolean | null | undefined>;

const TELEMETRY_CONSENT_STORAGE_KEY = 'audraflow.telemetryConsent';

function createTelemetryConsentState(enabled: boolean): TelemetryConsentState {
  return {
    enabled,
    decided: true,
    updatedAtMs: Date.now(),
  };
}

function readBrowserTelemetryConsent(): TelemetryConsentState | null {
  if (typeof window === 'undefined') return null;

  try {
    const stored = window.localStorage.getItem(TELEMETRY_CONSENT_STORAGE_KEY);
    if (!stored) return null;

    const parsed = JSON.parse(stored) as Partial<TelemetryConsentState>;
    if (typeof parsed.enabled !== 'boolean' || typeof parsed.decided !== 'boolean') {
      return null;
    }

    return {
      enabled: parsed.enabled,
      decided: parsed.decided,
      updatedAtMs: typeof parsed.updatedAtMs === 'number' ? parsed.updatedAtMs : null,
    };
  } catch {
    return null;
  }
}

function writeBrowserTelemetryConsent(enabled: boolean): TelemetryConsentState {
  const state = createTelemetryConsentState(enabled);

  if (typeof window !== 'undefined') {
    window.localStorage.setItem(TELEMETRY_CONSENT_STORAGE_KEY, JSON.stringify(state));
  }

  return state;
}

type TauriDroppedFile = File & { path?: string };

function filePathFromDrop(file: File): string {
  return (file as TauriDroppedFile).path || file.name;
}

function fileNameFromSource(source: string): string {
  try {
    const url = new URL(source);
    const name = url.pathname.split('/').filter(Boolean).pop();
    return name || url.hostname;
  } catch {
    return source.split(/[\\/]/).pop() || source;
  }
}

function createLog(level: JobLogEntry['level'], message: string): JobLogEntry {
  return {
    id: `${Date.now()}-${Math.random().toString(16).slice(2)}`,
    time: new Date().toLocaleTimeString(),
    level,
    message,
  };
}

function parseTimeToSeconds(value: string): number | null {
  const trimmed = value.trim();
  if (!trimmed) return 0;

  if (!trimmed.includes(':')) {
    const seconds = Number(trimmed);
    return Number.isFinite(seconds) ? seconds : null;
  }

  const parts = trimmed.split(':').map((part) => part.trim());
  if (parts.length < 2 || parts.length > 3 || parts.some((part) => part === '')) {
    return null;
  }

  const values = parts.map(Number);
  if (values.some((part) => !Number.isFinite(part) || part < 0)) {
    return null;
  }

  if (values.length === 2) {
    return values[0] * 60 + values[1];
  }
  return values[0] * 3600 + values[1] * 60 + values[2];
}

function formatTimeInput(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return '0';
  const rounded = Math.max(0, Math.round(seconds));
  const minutes = Math.floor(rounded / 60);
  const remainingSeconds = rounded % 60;
  return `${minutes}:${String(remainingSeconds).padStart(2, '0')}`;
}

function formatTime(ms: number): string {
  const s = Math.floor(ms / 1000);
  const m = Math.floor(s / 60);
  return `${m.toString().padStart(2, '0')}:${(s % 60).toString().padStart(2, '0')}`;
}

function stateLabel(state: JobInfo['state'], t: Translate) {
  return t(`state.${state}`);
}

function runtimeDependencyLabel(id: string, t: Translate): string {
  return t(`runtime.item.${id}`);
}

function runtimeDependencyHint(id: string, t: Translate): string {
  return t(`runtime.hint.${id}`);
}

function runtimeDependencyFix(id: string, t: Translate): string {
  return t(`runtime.fix.${id}`);
}

function runtimeStatusLabel(status: RuntimeDependencyStatus, t: Translate): string {
  return t(`runtime.status.${status}`);
}

function runtimeKindLabel(kind: RuntimeDependencyKind, t: Translate): string {
  return t(`runtime.kind.${kind}`);
}

function jobStatusMessage(status: BackendJobStatus, t: Translate): string {
  if (status.state === 'completed') {
    return status.message && status.message !== 'Completed'
      ? status.message
      : t('state.transcriptReady');
  }
  return status.message || stateLabel(status.state, t);
}

function jobPhaseFromStatus(status: BackendJobStatus, previousPhase?: string): string | undefined {
  const phases: Partial<Record<JobInfo['state'], string>> = {
    pending: 'queue',
    running: 'transcribe',
    paused: 'paused',
    completed: 'ready',
    cancelled: 'cancelled',
    failed: 'failed',
    notFound: 'missing',
  };
  return phases[status.state] ?? previousPhase;
}

function jobProgressFromStatus(status: BackendJobStatus): number {
  if (status.state === 'completed') return 100;
  return Math.max(0, Math.min(100, status.progressPct));
}

function normalizeJobState(value: string): JobInfo['state'] {
  const normalized = value.trim();
  if (
    normalized === 'pending' ||
    normalized === 'running' ||
    normalized === 'paused' ||
    normalized === 'completed' ||
    normalized === 'cancelled' ||
    normalized === 'failed' ||
    normalized === 'notFound'
  ) {
    return normalized;
  }
  return 'notFound';
}

function parseSqliteTimestampMs(value?: string | null): number | undefined {
  if (!value) return undefined;
  const normalized = value.includes('T') ? value : `${value.replace(' ', 'T')}Z`;
  const ms = Date.parse(normalized);
  return Number.isFinite(ms) ? ms : undefined;
}

function persistedJobToJobInfo(job: PersistedJobSummary, t: Translate): JobInfo {
  const state = normalizeJobState(job.state);
  const status: BackendJobStatus = {
    jobId: job.jobId,
    state,
    progressPct: state === 'completed' ? 100 : 0,
    message: null,
  };
  const createdAtMs = parseSqliteTimestampMs(job.createdAt) ?? Date.now();
  const completedAtMs = parseSqliteTimestampMs(job.completedAt);
  const activity = jobStatusMessage(status, t);

  return {
    jobId: job.jobId,
    fileName: job.fileName || fileNameFromSource(job.filePath),
    duration: formatDurationSeconds(job.durationSeconds),
    durationSeconds: job.durationSeconds,
    format: job.format || undefined,
    size: job.sizeBytes > 0 ? formatFileSize(job.sizeBytes) : undefined,
    state,
    progress: jobProgressFromStatus(status),
    phase: jobPhaseFromStatus(status),
    activity,
    error: state === 'failed' ? activity : undefined,
    createdAtMs,
    completedAtMs: completedAtMs ?? (['completed', 'cancelled', 'failed', 'notFound'].includes(state) ? createdAtMs : undefined),
    logs: [
      createLog(
        state === 'failed' ? 'error' : 'info',
        job.segmentCount > 0
          ? `${activity} (${job.segmentCount} segment${job.segmentCount === 1 ? '' : 's'})`
          : activity
      ),
    ],
  };
}

function mergePersistedJobInfo(existing: JobInfo, restored: JobInfo): JobInfo {
  const shouldUseRestoredState = isTerminalJob(restored.state) || isTerminalJob(existing.state);
  if (shouldUseRestoredState) {
    const logs = existing.logs.length > 0 ? existing.logs : restored.logs;
    const last = logs[logs.length - 1];
    const restoredActivity = restored.activity;
    const shouldAppend = typeof restoredActivity === 'string' && last?.message !== restoredActivity;
    return {
      ...restored,
      logs: shouldAppend
        ? [...logs, createLog(restored.state === 'failed' ? 'error' : 'info', restoredActivity)].slice(-20)
        : logs,
    };
  }

  return {
    ...existing,
    fileName: restored.fileName || existing.fileName,
    duration: restored.duration,
    durationSeconds: restored.durationSeconds,
    format: restored.format,
    size: restored.size,
    createdAtMs: restored.createdAtMs,
    completedAtMs: restored.completedAtMs ?? existing.completedAtMs,
  };
}

function mergePersistedJobs(current: JobInfo[], restored: JobInfo[]): JobInfo[] {
  const currentById = new Map(current.map((job) => [job.jobId, job]));
  const restoredIds = new Set(restored.map((job) => job.jobId));
  const mergedRestored = restored.map((job) => {
    const existing = currentById.get(job.jobId);
    return existing ? mergePersistedJobInfo(existing, job) : job;
  });
  const unsavedJobs = current.filter((job) => !restoredIds.has(job.jobId));
  return [...mergedRestored, ...unsavedJobs];
}

interface BatchQueueReport {
  total: number;
  completed: number;
  failed: number;
  cancelled: number;
  active: number;
  terminal: number;
  failureRatePct: number;
  endToEndRtf: number | null;
  exportReady: number;
  exportBlocked: number;
  mcmSamples: number;
  allDone: boolean;
}

function isTerminalJob(state: JobInfo['state']): boolean {
  return ['completed', 'cancelled', 'failed', 'notFound'].includes(state);
}

function buildBatchQueueReport(jobs: JobInfo[]): BatchQueueReport {
  const total = jobs.length;
  const completed = jobs.filter((job) => job.state === 'completed').length;
  const failed = jobs.filter((job) => job.state === 'failed' || job.state === 'notFound').length;
  const cancelled = jobs.filter((job) => job.state === 'cancelled').length;
  const terminalJobs = jobs.filter((job) => isTerminalJob(job.state));
  const terminal = terminalJobs.length;
  const active = total - terminal;
  const totalAudioSeconds = terminalJobs.reduce(
    (sum, job) => sum + (Number.isFinite(job.durationSeconds ?? NaN) ? Math.max(0, job.durationSeconds ?? 0) : 0),
    0
  );
  const totalElapsedSeconds = terminalJobs.reduce(
    (sum, job) => sum + Math.max(0, (job.completedAtMs ?? job.createdAtMs) - job.createdAtMs) / 1000,
    0
  );

  return {
    total,
    completed,
    failed,
    cancelled,
    active,
    terminal,
    failureRatePct: total > 0 ? (failed / total) * 100 : 0,
    endToEndRtf: totalAudioSeconds > 0 && terminal > 0 ? totalElapsedSeconds / totalAudioSeconds : null,
    exportReady: completed,
    exportBlocked: failed + cancelled,
    mcmSamples: 0,
    allDone: total > 0 && terminal === total,
  };
}

function recordTelemetry(payload: TelemetryEventPayload): void {
  if (!hasTauriRuntime()) return;
  void invokeTauri<void>('cmd_record_telemetry_event', { request: payload }).catch(() => {});
}

function formatPercent(value: number): string {
  return `${value.toFixed(value >= 10 ? 0 : 1)}%`;
}

function formatRtf(value: number | null): string {
  return value == null || !Number.isFinite(value) ? '--' : value.toFixed(2);
}

const LOCAL_MEDIA_EXTENSIONS = new Set([
  'mp3',
  'wav',
  'm4a',
  'mp4',
  'mov',
  'aac',
  'flac',
  'ogg',
  'webm',
  'mkv',
  'wma',
  'opus',
]);

function isSupportedLocalMediaPath(path: string): boolean {
  const ext = path.split(/[\\/]/).pop()?.split('.').pop()?.toLowerCase();
  return !!ext && LOCAL_MEDIA_EXTENSIONS.has(ext);
}

function formatDurationSeconds(seconds?: number | null): string {
  if (!seconds || !Number.isFinite(seconds) || seconds <= 0) return '--:--';
  const rounded = Math.round(seconds);
  const hours = Math.floor(rounded / 3600);
  const minutes = Math.floor((rounded % 3600) / 60);
  const remainingSeconds = rounded % 60;
  if (hours > 0) {
    return `${hours}:${String(minutes).padStart(2, '0')}:${String(remainingSeconds).padStart(2, '0')}`;
  }
  return `${minutes}:${String(remainingSeconds).padStart(2, '0')}`;
}

function formatFileSize(bytes?: number): string {
  if (!bytes || bytes <= 0) return '--';
  const units = ['B', 'KB', 'MB', 'GB'];
  let value = bytes;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  return `${value >= 10 || unitIndex === 0 ? value.toFixed(0) : value.toFixed(1)} ${units[unitIndex]}`;
}

function whisperModelNameMatchesPreference(name: string, preference: string): boolean {
  const normalized = name.trim().toLowerCase();
  if (preference === 'large' || preference === 'medium') {
    return normalized.startsWith(preference);
  }
  return normalized === preference || normalized.startsWith(`${preference}-`);
}

function preferredLyricsWhisperModel(
  installedModels: ModelInfo[],
  selectedModel: ModelInfo | null,
  extremeAccuracy: boolean
): ModelInfo | null {
  if (selectedModel) return selectedModel;

  const preferences = extremeAccuracy
    ? ['large-v3-turbo', 'large-v3', 'large', 'medium', 'small', 'base']
    : ['small', 'medium', 'large-v3-turbo', 'large-v3', 'large', 'base'];

  for (const preference of preferences) {
    const match = installedModels.find((model) =>
      whisperModelNameMatchesPreference(model.name, preference)
    );
    if (match) return match;
  }

  return installedModels[0] ?? null;
}

function buildEffectiveTranscriptionPlan(
  asrEngine: AsrEngine,
  audioMode: AudioMode,
  selectedModel: ModelInfo | null,
  installedModels: ModelInfo[],
  extremeAccuracy: boolean
): EffectiveTranscriptionPlan {
  const lyricsModel = preferredLyricsWhisperModel(installedModels, selectedModel, extremeAccuracy);

  if (asrEngine === 'whisper') {
    const model = audioMode === 'music' ? lyricsModel : selectedModel;
    return {
      engine: 'whisper',
      model,
      ready: Boolean(model),
      reasonKey: audioMode === 'music' ? 'model.reasonExplicitWhisperMusic' : 'model.reasonExplicitWhisper',
    };
  }

  if (asrEngine === 'sensevoice') {
    return {
      engine: 'sensevoice',
      model: null,
      ready: true,
      reasonKey: 'model.reasonExplicitSenseVoice',
    };
  }

  if (asrEngine === 'funasr') {
    return {
      engine: 'funasr',
      model: null,
      ready: true,
      reasonKey: 'model.reasonExplicitFunAsr',
    };
  }

  const autoWhisperModel = audioMode === 'music' ? lyricsModel : selectedModel ?? lyricsModel;
  if (autoWhisperModel) {
    return {
      engine: 'whisper',
      model: autoWhisperModel,
      ready: true,
      reasonKey: audioMode === 'music' ? 'model.reasonAutoMusicWhisper' : 'model.reasonAutoSpeech',
    };
  }

  return {
    engine: 'sensevoice',
    model: null,
    ready: true,
    reasonKey: 'model.reasonAutoSenseVoiceFallback',
  };
}

async function inspectLocalMediaFiles(files: string[]): Promise<MediaFileInfo[]> {
  const uniqueFiles = [...new Set(files.filter(isSupportedLocalMediaPath))];
  if (uniqueFiles.length === 0) return [];

  try {
    return await invokeTauri<MediaFileInfo[]>('cmd_inspect_media_files', { filePaths: uniqueFiles });
  } catch {
    // Fall through to extension-only metadata for browser preview.
  }

  return uniqueFiles.map((filePath) => {
    const fileName = fileNameFromSource(filePath);
    const format = fileName.split('.').pop()?.toUpperCase() || '';
    return {
      filePath,
      fileName,
      format,
      sizeBytes: 0,
      durationSeconds: null,
    };
  });
}

async function runWithConcurrency<T>(
  items: T[],
  limit: number,
  worker: (item: T, index: number) => Promise<void>
): Promise<void> {
  let nextIndex = 0;
  const workerCount = Math.min(Math.max(1, limit), items.length);
  const workers = Array.from({ length: workerCount }, async () => {
    while (nextIndex < items.length) {
      const index = nextIndex;
      nextIndex += 1;
      await worker(items[index], index);
    }
  });
  await Promise.all(workers);
}

function AppNavigation({
  currentPage,
  jobsCount,
  onNavigate,
  className = '',
}: {
  currentPage: Page;
  jobsCount: number;
  onNavigate: (page: Page) => void;
  className?: string;
}) {
  const { t } = useI18n();
  const navItems: { page: Page; label: string; badge?: number }[] = [
    { page: 'import', label: t('nav.import') },
    { page: 'queue', label: t('nav.queue'), badge: jobsCount > 0 ? jobsCount : undefined },
    { page: 'editor', label: t('nav.editor') },
    { page: 'export', label: t('nav.export') },
    { page: 'settings', label: t('nav.settings') },
  ];

  return (
    <nav className={`app-nav ${className}`.trim()} aria-label={t('nav.primary')}>
      {navItems.map((item, index) => (
        <button
          key={item.page}
          type="button"
          className={currentPage === item.page ? 'active' : ''}
          onClick={() => onNavigate(item.page)}
        >
          <span className="nav-index">{String(index + 1).padStart(2, '0')}</span>
          <span className="nav-label">{item.label}</span>
          {item.badge !== undefined && (
            <span className="badge">{item.badge}</span>
          )}
        </button>
      ))}
    </nav>
  );
}

// ── Main App ──────────────────────────────────────────────────────────────

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

  const handleTitlebarDoubleClick = useCallback((event: React.MouseEvent<HTMLElement>) => {
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

// ── Import Page ────────────────────────────────────────────────────────────

function TelemetryConsentDialog({
  onChoose,
}: {
  onChoose: (enabled: boolean) => Promise<TelemetryConsentState>;
}) {
  const { t } = useI18n();
  const [savingChoice, setSavingChoice] = useState<boolean | null>(null);
  const [error, setError] = useState<string | null>(null);

  const handleChoose = (enabled: boolean) => {
    setSavingChoice(enabled);
    setError(null);
    void onChoose(enabled)
      .catch((chooseError) => {
        setError(errorToMessage(chooseError, t));
      })
      .finally(() => setSavingChoice(null));
  };

  return (
    <div className="modal-backdrop" role="presentation">
      <div className="consent-dialog" role="dialog" aria-modal="true" aria-labelledby="telemetry-title">
        <h2 id="telemetry-title">{t('telemetry.title')}</h2>
        <p>{t('telemetry.description')}</p>
        <div className="consent-facts">
          <span>{t('telemetry.factLocal')}</span>
          <span>{t('telemetry.factHashed')}</span>
          <span>{t('telemetry.factDisable')}</span>
        </div>
        <div className="consent-actions">
          <button className="btn-secondary" disabled={savingChoice !== null} onClick={() => handleChoose(false)}>
            {savingChoice === false ? t('telemetry.saving') : t('telemetry.keepOff')}
          </button>
          <button className="btn-primary" disabled={savingChoice !== null} onClick={() => handleChoose(true)}>
            {savingChoice === true ? t('telemetry.saving') : t('telemetry.enable')}
          </button>
        </div>
        {error && <p className="consent-error">{error}</p>}
      </div>
    </div>
  );
}

function ImportPage({
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

  const handleDrop = async (e: React.DragEvent) => {
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

  const handleUrlSubmit = (e: React.FormEvent) => {
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

// ── Queue Page ─────────────────────────────────────────────────────────────

function QueuePage({
  jobs,
  onImport,
  onJobAction,
  onRefreshJobs,
}: {
  jobs: JobInfo[];
  onImport: () => void;
  onJobAction: (jobId: string, action: QueueAction) => void;
  onRefreshJobs: () => Promise<number>;
}) {
  const { t } = useI18n();
  const [refreshing, setRefreshing] = useState(false);
  const [refreshMessage, setRefreshMessage] = useState<string | null>(null);
  const report = useMemo(() => buildBatchQueueReport(jobs), [jobs]);
  const handleRefresh = useCallback(() => {
    setRefreshing(true);
    setRefreshMessage(null);
    void onRefreshJobs()
      .then((count) => {
        setRefreshMessage(t('queue.refreshed', {
          count,
          plural: count === 1 ? '' : 's',
        }));
      })
      .catch((error) => {
        setRefreshMessage(t('queue.refreshFailed', { error: errorToMessage(error, t) }));
      })
      .finally(() => setRefreshing(false));
  }, [onRefreshJobs, t]);

  if (jobs.length === 0) {
    return (
      <div className="page queue-page">
        <div className="empty-state">
          <h2>{t('queue.emptyTitle')}</h2>
          <p>{t('queue.emptyDescription')}</p>
          <div className="empty-state-actions">
            <button className="btn-primary" type="button" onClick={onImport}>
              {t('queue.importAction')}
            </button>
            <button className="btn-secondary" type="button" disabled={refreshing} onClick={handleRefresh}>
              {refreshing ? t('queue.refreshing') : t('queue.refresh')}
            </button>
          </div>
          {refreshMessage ? <p className="queue-sync-status">{refreshMessage}</p> : null}
        </div>
      </div>
    );
  }

  return (
    <div className="page queue-page">
      <div className="queue-header">
        <h2>{t('queue.title', { count: jobs.length })}</h2>
        <button className="btn-secondary" type="button" disabled={refreshing} onClick={handleRefresh}>
          {refreshing ? t('queue.refreshing') : t('queue.refresh')}
        </button>
      </div>
      <div className="queue-note">
        {t('queue.note')}
      </div>
      {refreshMessage ? <p className="queue-sync-status">{refreshMessage}</p> : null}
      <div className={`batch-report ${report.allDone ? 'batch-report-done' : ''}`}>
        <div className="batch-report-title">
          <span>{report.allDone ? t('queue.batchComplete') : t('queue.batchInProgress')}</span>
          <strong>{t('queue.completedCount', { completed: report.completed, total: report.total })}</strong>
        </div>
        <div className="batch-report-grid">
          <div>
            <span>{t('queue.active')}</span>
            <strong>{report.active}</strong>
          </div>
          <div>
            <span>{t('queue.failed')}</span>
            <strong>{report.failed}</strong>
          </div>
          <div>
            <span>{t('queue.failureRate')}</span>
            <strong>{formatPercent(report.failureRatePct)}</strong>
          </div>
          <div>
            <span>{t('queue.rtf')}</span>
            <strong>{formatRtf(report.endToEndRtf)}</strong>
          </div>
          <div>
            <span>{t('queue.mcmSamples')}</span>
            <strong>{report.mcmSamples}</strong>
          </div>
          <div>
            <span>{t('queue.exportReady')}</span>
            <strong>{report.exportReady}/{report.total}</strong>
          </div>
        </div>
        {report.exportBlocked > 0 && (
          <p className="batch-report-note">
            {t('queue.notExportable', { count: report.exportBlocked })}
          </p>
        )}
      </div>
      <div className="job-list">
        {jobs.map((job) => (
          <div key={job.jobId} className={`job-card job-${job.state}`}>
            <div className="job-info">
              <span className="job-name">{job.fileName}</span>
              <span className="job-duration">{job.activity || job.duration}</span>
              <span className="job-media-meta">
                {[job.duration, job.format, job.size].filter(Boolean).join(' · ')}
              </span>
              <div className="job-progress-row">
                <div className="progress-bar">
                  <div
                    className="progress-fill"
                    style={{ width: `${job.progress}%` }}
                  />
                </div>
                <span className="progress-label">
                  {job.phase || 'queued'} · {Math.round(job.progress)}%
                </span>
              </div>
              {job.logs.length > 0 && (
                <div className="job-log">
                  {job.logs.slice(-8).map((entry) => (
                    <div key={entry.id} className={`job-log-line log-${entry.level}`}>
                      <span className="job-log-time">{entry.time}</span>
                      <span className="job-log-message">{entry.message}</span>
                    </div>
                  ))}
                </div>
              )}
            </div>
            <div className="job-status">
              <span className={`status-badge status-${job.state}`}>
                {stateLabel(job.state, t)}
              </span>
              {job.error && <span className="job-error" title={job.error}>{job.error}</span>}
            </div>
            <div className="job-actions">
              {job.state === 'paused' ? (
                <button onClick={() => onJobAction(job.jobId, 'resume')}>
                  {t('queue.resume')}
                </button>
              ) : (
                <button
                  disabled={['completed', 'cancelled', 'failed', 'notFound'].includes(job.state)}
                  onClick={() => onJobAction(job.jobId, 'pause')}
                >
                  {t('queue.pause')}
                </button>
              )}
              <button
                disabled={!['failed', 'cancelled'].includes(job.state)}
                onClick={() => onJobAction(job.jobId, 'retry')}
              >
                {t('queue.retry')}
              </button>
              <button
                disabled={['completed', 'cancelled', 'notFound'].includes(job.state)}
                onClick={() => onJobAction(job.jobId, 'skip')}
              >
                {t('queue.skip')}
              </button>
              <button
                disabled={['completed', 'cancelled', 'failed', 'notFound'].includes(job.state)}
                onClick={() => onJobAction(job.jobId, 'cancel')}
              >
                {t('queue.cancel')}
              </button>
              <button
                disabled={job.state !== 'completed'}
                onClick={() => onJobAction(job.jobId, 'open')}
              >
                {job.state === 'completed' ? t('queue.openTranscript') : t('queue.open')}
              </button>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

// ── Editor Page ────────────────────────────────────────────────────────────

// ── Editor Page ────────────────────────────────────────────────────────────

interface TranscriptSegment {
  id: string;
  startMs: number;
  endMs: number;
  speaker: string;
  text: string;
  rawText: string;
  confidence: number;
  lowConfidenceReasons: string[];
  hasCorrection: boolean;
  hasMark: boolean;
  marks: TimestampMark[];
}

interface GlossaryApplyResult {
  updatedSegments: TranscriptSegment[];
  updatedCount: number;
  entry: GlossaryEntry;
}

interface TimestampMark {
  id: number;
  segmentId: string;
  markMs: number;
  label?: string | null;
  note?: string | null;
}

const LOW_CONFIDENCE_REASON_LABELS: Record<string, string> = {
  low_snr: 'Low SNR',
  high_noise: 'Noise',
  term_conflict: 'Term',
  overlapping_speech: 'Overlap',
  speaker_uncertain: 'Speaker',
  low_confidence: 'ASR',
};

const LOW_CONFIDENCE_REASON_WEIGHTS: Record<string, number> = {
  overlapping_speech: 40,
  term_conflict: 34,
  low_snr: 30,
  high_noise: 26,
  speaker_uncertain: 24,
  low_confidence: 18,
};

function lowConfidenceRiskScore(segment: TranscriptSegment): number {
  const confidencePenalty = Math.round((1 - Math.max(0, Math.min(1, segment.confidence))) * 70);
  const reasonPenalty = segment.lowConfidenceReasons.reduce(
    (max, reason) => Math.max(max, LOW_CONFIDENCE_REASON_WEIGHTS[reason] ?? 16),
    0
  );
  return Math.min(100, confidencePenalty + reasonPenalty);
}

function riskLevel(score: number): 'high' | 'medium' | 'low' {
  if (score >= 70) return 'high';
  if (score >= 45) return 'medium';
  return 'low';
}

function riskLabel(level: ReturnType<typeof riskLevel>, t: Translate): string {
  return t(`editor.risk.${level}`);
}

function reasonLabel(reason: string, t: Translate): string {
  const key = `editor.reason.${reason}`;
  const translated = t(key);
  return translated === key ? LOW_CONFIDENCE_REASON_LABELS[reason] ?? reason.replace(/_/g, ' ') : translated;
}

function segmentTextDiff(segment: TranscriptSegment): { original: string; replacement: string } | null {
  if (segment.rawText === segment.text) return null;

  const before = Array.from(segment.rawText);
  const after = Array.from(segment.text);
  let prefix = 0;
  while (prefix < before.length && prefix < after.length && before[prefix] === after[prefix]) {
    prefix += 1;
  }

  let beforeSuffix = before.length - 1;
  let afterSuffix = after.length - 1;
  while (
    beforeSuffix >= prefix &&
    afterSuffix >= prefix &&
    before[beforeSuffix] === after[afterSuffix]
  ) {
    beforeSuffix -= 1;
    afterSuffix -= 1;
  }

  const original = before.slice(prefix, beforeSuffix + 1).join('').trim();
  const replacement = after.slice(prefix, afterSuffix + 1).join('').trim();
  if (!original || !replacement || original === replacement) return null;
  return { original, replacement };
}

const WAVEFORM_BAR_COUNT = 120;
const MAX_WAVEFORM_DECODE_BYTES = 120 * 1024 * 1024;

function extractWaveformPeaks(audioBuffer: AudioBuffer, barCount = WAVEFORM_BAR_COUNT): number[] {
  const peaks: number[] = [];
  const channelCount = Math.max(1, Math.min(audioBuffer.numberOfChannels, 2));
  const samplesPerBar = Math.max(1, Math.floor(audioBuffer.length / barCount));

  for (let i = 0; i < barCount; i += 1) {
    const start = i * samplesPerBar;
    const end = Math.min(audioBuffer.length, start + samplesPerBar);
    const step = Math.max(1, Math.floor((end - start) / 240));
    let peak = 0;

    for (let sample = start; sample < end; sample += step) {
      let value = 0;
      for (let channel = 0; channel < channelCount; channel += 1) {
        value += Math.abs(audioBuffer.getChannelData(channel)[sample] ?? 0);
      }
      peak = Math.max(peak, value / channelCount);
    }

    peaks.push(peak);
  }

  const maxPeak = Math.max(...peaks, 0.01);
  return peaks.map((peak) => Math.max(0.08, Math.min(1, peak / maxPeak)));
}

function buildSegmentTimelinePeaks(
  segments: TranscriptSegment[],
  totalDuration: number,
  barCount = WAVEFORM_BAR_COUNT
): number[] {
  if (segments.length === 0 || totalDuration <= 0) return [];

  const peaks = Array.from({ length: barCount }, () => 0.08);
  for (const segment of segments) {
    const start = Math.max(0, Math.floor((segment.startMs / 1000 / totalDuration) * barCount));
    const end = Math.min(barCount - 1, Math.ceil((segment.endMs / 1000 / totalDuration) * barCount));
    const base = Math.max(0.22, Math.min(0.92, 0.26 + segment.confidence * 0.58));
    for (let index = start; index <= end; index += 1) {
      const variation = ((index % 7) / 7) * 0.16;
      peaks[index] = Math.max(peaks[index], Math.min(1, base + variation));
    }
  }

  return peaks;
}

// Demo data simulating a transcribed interview
const DEMO_SEGMENTS: TranscriptSegment[] = [
  { id: 's1', startMs: 0, endMs: 4200, speaker: 'A', text: '今天我们来聊一聊人工智能在医疗领域的应用。', rawText: '今天我们来聊一聊人工智能在医疗领域的应用。', confidence: 0.94, lowConfidenceReasons: [], hasCorrection: false, hasMark: false, marks: [] },
  { id: 's2', startMs: 4500, endMs: 9800, speaker: 'B', text: '是的，最近腾训在医疗AI方面投入很大。', rawText: '是的，最近腾训在医疗AI方面投入很大。', confidence: 0.72, lowConfidenceReasons: ['term_conflict'], hasCorrection: true, hasMark: false, marks: [] },
  { id: 's3', startMs: 10200, endMs: 15800, speaker: 'A', text: '你说的是腾讯吧？他们确实在医学影像分析上做得不错。', rawText: '你说的是腾训吧？他们确实在医学影像分析上做得不错。', confidence: 0.88, lowConfidenceReasons: [], hasCorrection: true, hasMark: false, marks: [] },
  { id: 's4', startMs: 16200, endMs: 22100, speaker: 'B', text: '对，就是腾讯。另外，字节跳动也在探索AI辅助诊断。', rawText: '对，就是腾训。另外，字节跳动也在探索AI辅助诊断。', confidence: 0.85, lowConfidenceReasons: [], hasCorrection: true, hasMark: false, marks: [] },
  { id: 's5', startMs: 22500, endMs: 26300, speaker: 'A', text: '这是一个值得关注的方向。', rawText: '这是一个值得关注的方向。', confidence: 0.96, lowConfidenceReasons: [], hasCorrection: false, hasMark: true, marks: [{ id: 1, segmentId: 's5', markMs: 24000, label: 'Mark' }] },
];

function EditorPage({ jobId }: { jobId: string | null }) {
  const { t } = useI18n();
  const editorIsTauriRuntime = hasTauriRuntime();
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const proofreadSessionRef = useRef<{
    jobId: string;
    startedAtMs: number;
    completedRatio: number;
  } | null>(null);
  const [currentSegmentId, setCurrentSegmentId] = useState<string | null>(null);
  const [isPlaying, setIsPlaying] = useState(false);
  const [speed, setSpeed] = useState(1.0);
  const [currentTime, setCurrentTime] = useState(0);
  const [segments, setSegments] = useState<TranscriptSegment[]>(() => (
    editorIsTauriRuntime ? [] : DEMO_SEGMENTS
  ));
  const [editingSpeaker, setEditingSpeaker] = useState<string | null>(null);
  const [speakerEdits, setSpeakerEdits] = useState<Record<string, string>>({});
  const [termActionSegmentId, setTermActionSegmentId] = useState<string | null>(null);
  const [mediaSrc, setMediaSrc] = useState<string | null>(null);
  const [waveform, setWaveform] = useState<{ source: string; peaks: number[] } | null>(null);
  const [loadState, setLoadState] = useState<'idle' | 'loading' | 'ready' | 'error'>(() => (
    editorIsTauriRuntime ? 'idle' : 'ready'
  ));
  const [editorStatus, setEditorStatus] = useState<string | null>(() => (
    editorIsTauriRuntime ? null : t('editor.browserPreview')
  ));
  const [searchQuery, setSearchQuery] = useState('');
  const [searchResults, setSearchResults] = useState<TranscriptSegment[]>([]);
  const [searchIndex, setSearchIndex] = useState(0);
  const [searchBusy, setSearchBusy] = useState(false);

  const totalDuration = segments.length > 0
    ? segments[segments.length - 1].endMs / 1000
    : 0;
  const displayedWaveformPeaks = useMemo(
    () => (
      waveform?.source === mediaSrc && waveform.peaks.length > 0
        ? waveform.peaks
        : buildSegmentTimelinePeaks(segments, totalDuration)
    ),
    [mediaSrc, segments, totalDuration, waveform]
  );
  const searchResultIds = useMemo(
    () => new Set(searchResults.map((segment) => segment.id)),
    [searchResults]
  );

  useEffect(() => {
    let cancelled = false;
    if (!mediaSrc) return;

    const AudioContextCtor = window.AudioContext
      ?? (window as Window & { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
    if (!AudioContextCtor) return;

    const controller = new AbortController();
    let audioContext: AudioContext | null = null;

    void fetch(mediaSrc, { signal: controller.signal })
      .then((response) => {
        if (!response.ok) {
          throw new Error(`Could not load waveform source: ${response.status}`);
        }
        return response.arrayBuffer();
      })
      .then(async (buffer) => {
        if (buffer.byteLength > MAX_WAVEFORM_DECODE_BYTES) {
          throw new Error('Audio is too large for inline waveform decoding.');
        }
        audioContext = new AudioContextCtor();
        const decoded = await audioContext.decodeAudioData(buffer);
        if (!cancelled) {
          setWaveform({ source: mediaSrc, peaks: extractWaveformPeaks(decoded) });
        }
      })
      .catch((error) => {
        if (!cancelled && error instanceof Error && error.name !== 'AbortError') {
          setWaveform((current) => current?.source === mediaSrc ? null : current);
        }
      })
      .finally(() => {
        void audioContext?.close().catch(() => {});
      });

    return () => {
      cancelled = true;
      controller.abort();
    };
  }, [mediaSrc]);

  useEffect(() => {
    let cancelled = false;

    if (!jobId) {
      queueMicrotask(() => {
        if (cancelled) return;
        setIsPlaying(false);
        setCurrentTime(0);
        setCurrentSegmentId(null);
        setMediaSrc(null);
        setSpeakerEdits({});
        setSearchQuery('');
        setSearchResults([]);
        setSearchIndex(0);
        if (editorIsTauriRuntime) {
          setSegments([]);
          setLoadState('idle');
          setEditorStatus(t('editor.openCompleted'));
        } else {
          setSegments(DEMO_SEGMENTS);
          setLoadState('ready');
          setEditorStatus(t('editor.browserPreview'));
        }
      });
      return () => {
        cancelled = true;
      };
    }

    queueMicrotask(() => {
      if (cancelled) return;
      setIsPlaying(false);
      setCurrentTime(0);
      setSegments([]);
      setSpeakerEdits({});
      setSearchResults([]);
      setSearchIndex(0);
      setMediaSrc(null);
      setLoadState('loading');
      setEditorStatus(t('editor.loading'));
    });

    void invokeTauri<TranscriptResponse>('cmd_get_transcript', { jobId })
      .then((response) => {
        if (cancelled) return;
        setSegments(response.segments);
        setSpeakerEdits({});
        setCurrentSegmentId(response.segments[0]?.id ?? null);
        setMediaSrc(convertFileSrc(response.mediaSrcPath || response.filePath));
        setLoadState('ready');
        setEditorStatus(t('editor.loadedSegments', { count: response.segments.length }));
        proofreadSessionRef.current = {
          jobId,
          startedAtMs: Date.now(),
          completedRatio: response.segments.length > 0 ? 1 : 0,
        };
        recordTelemetry({
          eventType: 'proofread_session_start',
          jobId,
          audioHours: (response.segments.at(-1)?.endMs ?? 0) / 3_600_000,
          transcriptChars: response.segments.reduce((sum, segment) => sum + segment.text.length, 0),
        });
      })
      .catch((error) => {
        if (cancelled) return;
        setLoadState('error');
        setEditorStatus(errorToMessage(error, t));
      });

    return () => {
      cancelled = true;
      const session = proofreadSessionRef.current;
      if (session?.jobId === jobId) {
        const activeSeconds = Math.max(0, (Date.now() - session.startedAtMs) / 1000);
        recordTelemetry({
          eventType: 'proofread_session_end',
          jobId,
          activeSeconds,
          inactiveSeconds: 0,
          completedRatio: session.completedRatio,
        });
        proofreadSessionRef.current = null;
      }
    };
  }, [editorIsTauriRuntime, jobId, t]);

  const handleInsertMark = useCallback(() => {
    const currentMs = Math.round(currentTime * 1000);
    const activeSegment = segments.find((segment) =>
      currentMs >= segment.startMs && currentMs < segment.endMs
    ) ?? segments.find((segment) => segment.id === currentSegmentId);
    if (!activeSegment) return;

    const optimisticMark: TimestampMark = {
      id: Date.now(),
      segmentId: activeSegment.id,
      markMs: currentMs,
      label: 'Mark',
    };
    setSegments(prev => prev.map(s =>
      s.id === activeSegment.id
        ? { ...s, hasMark: true, marks: [...s.marks, optimisticMark] }
        : s
    ));
    setEditorStatus(t('editor.markAdded', { time: formatTime(currentMs) }));

    if (!jobId) return;

    void invokeTauri<TranscriptSegment>('cmd_add_timestamp_mark', {
      request: {
        segmentId: activeSegment.id,
        markMs: currentMs,
        label: 'Mark',
      },
    })
      .then((updated) => {
        setSegments((prev) => prev.map((segment) =>
          segment.id === activeSegment.id ? updated : segment
        ));
        setEditorStatus(t('editor.markSaved', { time: formatTime(currentMs) }));
      })
      .catch((error) => {
        setSegments((prev) => prev.map((segment) =>
          segment.id === activeSegment.id
            ? {
                ...segment,
                marks: segment.marks.filter((mark) => mark.id !== optimisticMark.id),
                hasMark: segment.marks.some((mark) => mark.id !== optimisticMark.id),
              }
            : segment
        ));
        setEditorStatus(errorToMessage(error, t));
      });
  }, [currentSegmentId, currentTime, jobId, segments, t]);

  const handleAcceptCandidate = useCallback((target?: TranscriptSegment) => {
    const segment = target ?? segments.find((item) => item.id === currentSegmentId) ?? segments.find((item) => item.hasCorrection);
    if (!segment) return;

    setTermActionSegmentId(segment.id);
    if (!jobId) {
      setSegments((prev) => prev.map((item) =>
        item.id === segment.id
          ? {
              ...item,
              lowConfidenceReasons: item.lowConfidenceReasons.filter((reason) => reason !== 'term_conflict'),
              hasCorrection: true,
            }
          : item
      ));
      setEditorStatus(t('editor.acceptedCandidate'));
      setTermActionSegmentId(null);
      return;
    }

    void invokeTauri<TranscriptSegment>('cmd_accept_term_candidate', {
      request: { segmentId: segment.id },
    })
      .then((updated) => {
        setSegments((prev) => prev.map((item) => item.id === updated.id ? updated : item));
        setEditorStatus(t('editor.acceptedCandidate'));
      })
      .catch((error) => setEditorStatus(errorToMessage(error, t)))
      .finally(() => setTermActionSegmentId(null));
  }, [currentSegmentId, jobId, segments, t]);

  const handleAddToGlossary = useCallback((target?: TranscriptSegment) => {
    const segment = target ?? segments.find((item) => item.id === currentSegmentId) ?? segments.find((item) => item.hasCorrection);
    if (!segment) return;

    const diff = segmentTextDiff(segment);
    if (!diff) {
      setEditorStatus(t('editor.glossaryNoDiff'));
      return;
    }

    setTermActionSegmentId(segment.id);
    if (!jobId) {
      setEditorStatus(t('editor.addedGlossary'));
      setTermActionSegmentId(null);
      return;
    }

    void invokeTauri<GlossaryApplyResult>('cmd_add_glossary_entry', {
      request: {
        jobId,
        canonical: diff.replacement,
        aliases: [diff.original],
        category: 'term',
      },
    })
      .then((result) => {
        if (result.updatedSegments.length > 0) {
          setSegments(result.updatedSegments);
        }
        setEditorStatus(t('editor.glossaryApplied', {
          canonical: result.entry.canonical,
          count: result.updatedCount,
        }));
      })
      .catch((error) => setEditorStatus(errorToMessage(error, t)))
      .finally(() => setTermActionSegmentId(null));
  }, [currentSegmentId, jobId, segments, t]);

  const seekAudio = useCallback((
    seconds: number,
    play = false,
    trigger = 'manual_scrub',
    segmentId?: string
  ) => {
    const bounded = Math.max(0, Math.min(totalDuration || seconds, seconds));
    const fromMs = Math.round(currentTime * 1000);
    const toMs = Math.round(bounded * 1000);
    const audio = audioRef.current;
    if (audio) {
      audio.currentTime = bounded;
      if (play) {
        void audio.play().catch((error) => {
          setIsPlaying(false);
          setEditorStatus(errorToMessage(error, t));
        });
      }
    }
    setCurrentTime(bounded);
    if (jobId && fromMs !== toMs) {
      recordTelemetry({
        eventType: 'playback_seek',
        jobId,
        segmentId: segmentId ?? currentSegmentId,
        fromMs,
        toMs,
        trigger,
      });
    }
  }, [currentSegmentId, currentTime, jobId, t, totalDuration]);

  const focusSearchResult = useCallback((results: TranscriptSegment[], nextIndex: number) => {
    const result = results[nextIndex];
    if (!result) return;
    setSearchIndex(nextIndex);
    setCurrentSegmentId(result.id);
    seekAudio(result.startMs / 1000, false, 'search_result', result.id);
    window.requestAnimationFrame(() => {
      document.getElementById(`seg-${result.id}`)?.scrollIntoView({
        block: 'center',
        behavior: 'smooth',
      });
    });
  }, [seekAudio]);

  const handleSearch = useCallback(async () => {
    const query = searchQuery.trim();
    if (!query) {
      setSearchResults([]);
      setSearchIndex(0);
      setEditorStatus(t('editor.searchEmpty'));
      return;
    }

    setSearchBusy(true);
    try {
      const results = jobId
        ? await invokeTauri<TranscriptSegment[]>('cmd_search_transcript', { jobId, query })
        : segments.filter((segment) => {
            const needle = query.toLowerCase();
            return (
              segment.text.toLowerCase().includes(needle) ||
              segment.rawText.toLowerCase().includes(needle) ||
              segment.speaker.toLowerCase().includes(needle)
            );
          });
      setSearchResults(results);
      setEditorStatus(t('editor.searchCount', { count: results.length }));
      if (results.length > 0) {
        focusSearchResult(results, 0);
      }
    } catch (error) {
      setEditorStatus(errorToMessage(error, t));
    } finally {
      setSearchBusy(false);
    }
  }, [focusSearchResult, jobId, searchQuery, segments, t]);

  const handleSearchStep = useCallback((direction: 1 | -1) => {
    if (searchResults.length === 0) return;
    const nextIndex = (searchIndex + direction + searchResults.length) % searchResults.length;
    focusSearchResult(searchResults, nextIndex);
  }, [focusSearchResult, searchIndex, searchResults]);

  const saveSegmentChange = useCallback(async (
    segmentId: string,
    patch: { text?: string; speaker?: string }
  ) => {
    if (!jobId) return;

    const current = segments.find((segment) => segment.id === segmentId);
    if (!current) return;
    if (patch.speaker !== undefined && patch.speaker === current.speaker) return;

    setSegments((prev) => prev.map((segment) =>
      segment.id === segmentId
        ? {
            ...segment,
            ...patch,
            hasCorrection: true,
          }
        : segment
    ));
    setEditorStatus(t('editor.saving'));

    try {
      const updated = await invokeTauri<TranscriptSegment>('cmd_update_segment', {
        request: {
          segmentId,
          ...patch,
        },
      });
      setSegments((prev) => prev.map((segment) =>
        segment.id === segmentId ? updated : segment
      ));
      setEditorStatus(t('editor.saved'));
    } catch (error) {
      setSegments((prev) => prev.map((segment) =>
        segment.id === segmentId ? current : segment
      ));
      setEditorStatus(errorToMessage(error, t));
    }
  }, [jobId, segments, t]);

  // ── Keyboard shortcuts (PRD §11.1) ──────────────────────────────────────
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      // Don't capture when typing in an input
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;

      const idx = segments.findIndex(s => s.id === currentSegmentId);

      switch (e.key) {
        case ' ':
          e.preventDefault();
          setIsPlaying(p => !p);
          break;
        case 'ArrowUp':
          e.preventDefault();
          if (idx > 0) setCurrentSegmentId(segments[idx - 1].id);
          break;
        case 'ArrowDown':
          e.preventDefault();
          if (idx < segments.length - 1) setCurrentSegmentId(segments[idx + 1].id);
          break;
        case 'j':
          e.preventDefault();
          seekAudio(currentTime - 3, isPlaying, 'jump_back', currentSegmentId ?? undefined);
          break;
        case 'l':
          e.preventDefault();
          seekAudio(currentTime + 3, isPlaying, 'jump_forward', currentSegmentId ?? undefined);
          break;
        case 't':
          if (e.ctrlKey) {
            e.preventDefault();
            handleInsertMark();
          }
          break;
        case '1':
          if (e.ctrlKey) { e.preventDefault(); setSpeed(0.5); }
          break;
        case '2':
          if (e.ctrlKey) { e.preventDefault(); setSpeed(1.0); }
          break;
        case '3':
          if (e.ctrlKey) { e.preventDefault(); setSpeed(1.5); }
          break;
        case 'Enter':
          if (e.ctrlKey) {
            e.preventDefault();
            handleAcceptCandidate();
          }
          break;
        case 'k':
          if (e.ctrlKey) {
            e.preventDefault();
            handleAddToGlossary();
          }
          break;
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [
    currentSegmentId,
    currentTime,
    handleAcceptCandidate,
    handleAddToGlossary,
    handleInsertMark,
    isPlaying,
    seekAudio,
    segments,
    totalDuration,
  ]);

  useEffect(() => {
    const audio = audioRef.current;
    if (!audio) return;
    audio.playbackRate = speed;
  }, [speed, mediaSrc]);

  useEffect(() => {
    const audio = audioRef.current;
    if (!audio || !mediaSrc) return;

    if (isPlaying) {
      void audio.play().catch((error) => {
        setIsPlaying(false);
        setEditorStatus(errorToMessage(error, t));
      });
    } else {
      audio.pause();
    }
  }, [isPlaying, mediaSrc, t]);

  useEffect(() => {
    if (mediaSrc) return;
    if (!isPlaying) return;
    const interval = window.setInterval(() => {
      setCurrentTime(t => {
        const next = t + 0.1 * speed;
        if (next >= totalDuration) {
          setIsPlaying(false);
          return totalDuration;
        }
        return next;
      });
    }, 100);
    return () => window.clearInterval(interval);
  }, [isPlaying, mediaSrc, speed, totalDuration]);

  const playbackSegmentId = useMemo(() => {
    const currentMs = currentTime * 1000;
    const active = segments.find(s => currentMs >= s.startMs && currentMs < s.endMs);
    return active?.id ?? null;
  }, [currentTime, segments]);

  const activeSegmentId = playbackSegmentId ?? currentSegmentId;

  // ── Actions ─────────────────────────────────────────────────────────────
  const handleRenameSpeaker = (segmentId: string, newName: string) => {
    setEditingSpeaker(null);
    const trimmed = newName.trim();
    if (!trimmed) return;
    void saveSegmentChange(segmentId, { speaker: trimmed });
  };

  // ── Sidebar data ────────────────────────────────────────────────────────
  const lowConfidenceItems = useMemo(() => [...segments]
    .filter(segment => segment.lowConfidenceReasons.length > 0)
    .sort((a, b) => {
      const riskDelta = lowConfidenceRiskScore(b) - lowConfidenceRiskScore(a);
      return riskDelta !== 0 ? riskDelta : a.startMs - b.startMs;
    }), [segments]);
  const correctionItems = segments.filter(s => s.hasCorrection);
  const handleSpeakerLabelChange = useCallback(async (fromSpeaker: string, toSpeaker: string) => {
    const trimmed = toSpeaker.trim();
    if (!trimmed || trimmed === fromSpeaker) return;

    const previousSegments = segments;
    const isMerge = segments.some((segment) => segment.speaker === trimmed);
    setSegments((prev) => prev.map((segment) =>
      segment.speaker === fromSpeaker
        ? { ...segment, speaker: trimmed, hasCorrection: true }
        : segment
    ));
    setSpeakerEdits((prev) => {
      const next = { ...prev };
      delete next[fromSpeaker];
      return next;
    });
    setEditorStatus(isMerge ? t('editor.merging') : t('editor.renaming'));

    if (!jobId) {
      setEditorStatus(isMerge ? t('editor.mergedPreview') : t('editor.renamedPreview'));
      return;
    }

    try {
      const updated = await invokeTauri<TranscriptSegment[]>('cmd_update_speaker_label', {
        request: {
          jobId,
          fromSpeaker,
          toSpeaker: trimmed,
        },
      });
      setSegments(updated);
      setEditorStatus(isMerge
        ? t('editor.merged', { from: fromSpeaker, to: trimmed })
        : t('editor.renamed', { from: fromSpeaker, to: trimmed }));
    } catch (error) {
      setSegments(previousSegments);
      setEditorStatus(errorToMessage(error, t));
    }
  }, [jobId, segments, t]);
  const speakerCounts = useMemo(() => segments.reduce<Record<string, number>>((counts, segment) => {
    counts[segment.speaker] = (counts[segment.speaker] ?? 0) + 1;
    return counts;
  }, {}), [segments]);
  const speakers = useMemo(() => Object.keys(speakerCounts), [speakerCounts]);

  // ── Format time ─────────────────────────────────────────────────────────
  // ── Render ──────────────────────────────────────────────────────────────
  return (
    <div className="page editor-page">
      <div className="editor-toolbar">
        <div>
          <h2>{t('editor.title')}</h2>
          <p className={`editor-status editor-status-${loadState}`}>
            {editorStatus || t('editor.ready')}
          </p>
        </div>
        <form
          className="editor-search"
          onSubmit={(event) => {
            event.preventDefault();
            void handleSearch();
          }}
        >
          <input
            type="search"
            value={searchQuery}
            onChange={(event) => {
              const value = event.target.value;
              setSearchQuery(value);
              if (!value.trim()) {
                setSearchResults([]);
                setSearchIndex(0);
              }
            }}
            placeholder={t('editor.searchPlaceholder')}
            disabled={segments.length === 0 || searchBusy}
          />
          <button
            type="submit"
            className="btn-secondary btn-sm-inline"
            disabled={segments.length === 0 || searchBusy}
          >
            {searchBusy ? t('editor.searching') : t('editor.search')}
          </button>
          <button
            type="button"
            className="btn-secondary btn-sm-inline"
            disabled={searchResults.length === 0}
            onClick={() => handleSearchStep(-1)}
          >
            {t('editor.prev')}
          </button>
          <button
            type="button"
            className="btn-secondary btn-sm-inline"
            disabled={searchResults.length === 0}
            onClick={() => handleSearchStep(1)}
          >
            {t('editor.next')}
          </button>
          {searchResults.length > 0 ? (
            <span>{searchIndex + 1}/{searchResults.length}</span>
          ) : null}
        </form>
      </div>
      <div className="editor-layout">
        <div className="editor-main" id="transcript-scroll">
          <div className="transcript-area">
            {segments.length === 0 ? (
              <div className="empty-state editor-empty">
                <h2>{loadState === 'loading' ? t('editor.loadingTitle') : t('editor.noTranscript')}</h2>
                <p>{editorStatus || t('editor.openCompleted')}</p>
              </div>
            ) : segments.map((seg) => {
              const isActive = seg.id === activeSegmentId;
              const isEdited = seg.text !== seg.rawText || seg.hasCorrection;
              const isSearchHit = searchResultIds.has(seg.id);

              return (
                <div
                  key={seg.id}
                  id={`seg-${seg.id}`}
                  className={`transcript-segment ${isActive ? 'segment-active' : ''} ${isEdited ? 'segment-edited' : ''} ${isSearchHit ? 'segment-search-hit' : ''}`}
                  onClick={() => {
                    setCurrentSegmentId(seg.id);
                    seekAudio(seg.startMs / 1000, false, 'click_segment', seg.id);
                  }}
                >
                  <div className="segment-meta">
                    <span className="segment-speaker">
                      {editingSpeaker === seg.id ? (
                        <input
                          className="speaker-edit-input"
                          defaultValue={seg.speaker}
                          autoFocus
                          onBlur={(e) => handleRenameSpeaker(seg.id, e.target.value)}
                          onKeyDown={(e) => {
                            if (e.key === 'Enter') handleRenameSpeaker(seg.id, (e.target as HTMLInputElement).value);
                            if (e.key === 'Escape') setEditingSpeaker(null);
                          }}
                        />
                      ) : (
                        <span
                          className="speaker-badge"
                          onClick={(e) => { e.stopPropagation(); setEditingSpeaker(seg.id); }}
                          title={t('editor.clickRename')}
                        >
                          {seg.speaker}
                        </span>
                      )}
                    </span>
                    <span
                      className="segment-time"
                      onClick={(e) => {
                        e.stopPropagation();
                        setCurrentSegmentId(seg.id);
                        seekAudio(seg.startMs / 1000, true, 'click_segment', seg.id);
                      }}
                      title={t('editor.clickJump')}
                    >
                      {formatTime(seg.startMs)}
                    </span>
                    {seg.hasMark && (
                      <span className="segment-mark" title={t('editor.markTitle')}>
                        {t('editor.mark')} {seg.marks.length > 1 ? seg.marks.length : ''}
                      </span>
                    )}
                    {seg.confidence < 0.8 && (
                      <span className="segment-low-conf" title={t('editor.confidence', { percent: (seg.confidence * 100).toFixed(0) })}>{t('editor.low')}</span>
                    )}
                  </div>
                  <div className="segment-text" onClick={(event) => event.stopPropagation()}>
                    <textarea
                      value={seg.text}
                      onChange={(event) => {
                        const nextText = event.target.value;
                        setSegments((prev) => prev.map((item) =>
                          item.id === seg.id ? { ...item, text: nextText } : item
                        ));
                      }}
                      onFocus={() => setCurrentSegmentId(seg.id)}
                      onBlur={(event) => {
                        void saveSegmentChange(seg.id, { text: event.target.value });
                      }}
                      rows={Math.max(2, Math.ceil(seg.text.length / 56))}
                    />
                    {seg.hasCorrection && (
                      <span className="correction-indicator" title={t('editor.hasCorrections')}>{t('editor.edited')}</span>
                    )}
                    {seg.marks.length > 0 && (
                      <div className="segment-marks">
                        {seg.marks.map((mark) => (
                          <button
                            key={mark.id}
                            type="button"
                            onClick={() => seekAudio(mark.markMs / 1000, true, 'replay', seg.id)}
                          >
                            {formatTime(mark.markMs)}
                            {mark.label ? ` ${mark.label}` : ''}
                          </button>
                        ))}
                      </div>
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        </div>

        <aside className="editor-sidebar">
          <div className="sidebar-section">
            <h3>{t('editor.lowConfidence', { count: lowConfidenceItems.length })}</h3>
            {lowConfidenceItems.length === 0 ? (
              <p className="placeholder-text">{t('editor.noItems')}</p>
            ) : (
              <ul className="sidebar-list">
                {lowConfidenceItems.map(item => (
                  <li
                    key={item.id}
                    className="sidebar-item"
                    onClick={() => {
                      setCurrentSegmentId(item.id);
                      seekAudio(item.startMs / 1000, false, 'click_segment', item.id);
                    }}
                  >
                    <span className="item-time">{formatTime(item.startMs)}</span>
                    <span className={`risk-badge risk-${riskLevel(lowConfidenceRiskScore(item))}`}>
                      {riskLabel(riskLevel(lowConfidenceRiskScore(item)), t)}
                    </span>
                    <span className="item-reason">
                      {item.lowConfidenceReasons.map((reason) => reasonLabel(reason, t)).join(', ')}
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </div>

          <div className="sidebar-section">
            <h3>{t('editor.termCandidates', { count: correctionItems.length })}</h3>
            {correctionItems.length === 0 ? (
              <p className="placeholder-text">{t('editor.noCandidates')}</p>
            ) : (
              <ul className="sidebar-list">
                {correctionItems.map(item => {
                  const diff = segmentTextDiff(item);
                  const busy = termActionSegmentId === item.id;
                  return (
                    <li
                      key={item.id}
                      className="sidebar-item candidate-item"
                      onClick={() => {
                        setCurrentSegmentId(item.id);
                        seekAudio(item.startMs / 1000, false, 'click_segment', item.id);
                      }}
                    >
                      <div className="candidate-summary">
                        <span className="item-time">{formatTime(item.startMs)}</span>
                        <span className="item-diff">
                          <span className="diff-old">{(diff?.original ?? item.rawText).slice(0, 15)}</span>
                          {' -> '}
                          <span className="diff-new">{(diff?.replacement ?? item.text).slice(0, 15)}</span>
                        </span>
                      </div>
                      <div className="candidate-actions">
                        <button
                          type="button"
                          disabled={busy}
                          onClick={(event) => {
                            event.stopPropagation();
                            handleAcceptCandidate(item);
                          }}
                        >
                          {busy ? t('editor.accepting') : t('editor.acceptCandidate')}
                        </button>
                        <button
                          type="button"
                          disabled={busy || !diff}
                          onClick={(event) => {
                            event.stopPropagation();
                            handleAddToGlossary(item);
                          }}
                        >
                          {busy ? t('editor.addingGlossary') : t('editor.addToGlossary')}
                        </button>
                      </div>
                    </li>
                  );
                })}
              </ul>
            )}
          </div>

          <div className="sidebar-section">
            <h3>{t('editor.speakers', { count: speakers.length })}</h3>
            <ul className="sidebar-list">
              {speakers.map(sp => (
                <li key={sp} className="sidebar-item speaker-item">
                  <div className="speaker-row">
                    <span className="speaker-badge">{sp}</span>
                    <span className="speaker-count">{t('editor.segmentCount', { count: speakerCounts[sp] })}</span>
                  </div>
                  <div className="speaker-actions">
                    <input
                      className="speaker-label-input"
                      value={speakerEdits[sp] ?? sp}
                      onChange={(event) => setSpeakerEdits((prev) => ({
                        ...prev,
                        [sp]: event.target.value,
                      }))}
                      onKeyDown={(event) => {
                        if (event.key === 'Enter') {
                          void handleSpeakerLabelChange(sp, speakerEdits[sp] ?? sp);
                        }
                      }}
                    />
                    <button
                      type="button"
                      onClick={() => void handleSpeakerLabelChange(sp, speakerEdits[sp] ?? sp)}
                      disabled={(speakerEdits[sp] ?? sp).trim() === sp}
                    >
                      {t('editor.rename')}
                    </button>
                  </div>
                  {speakers.length > 1 && (
                    <select
                      className="speaker-merge-select"
                      value=""
                      onChange={(event) => {
                        if (event.target.value) {
                          void handleSpeakerLabelChange(sp, event.target.value);
                        }
                      }}
                    >
                      <option value="" disabled>{t('editor.merge')}</option>
                      {speakers.filter((target) => target !== sp).map((target) => (
                        <option key={target} value={target}>{target}</option>
                      ))}
                    </select>
                  )}
                </li>
              ))}
            </ul>
          </div>
        </aside>
      </div>

      <div className="playback-bar">
        <button
          title={isPlaying ? t('editor.pauseShortcut') : t('editor.playShortcut')}
          disabled={segments.length === 0}
          onClick={() => setIsPlaying(p => !p)}
        >
          {isPlaying ? t('editor.pause') : t('editor.play')}
        </button>
        {mediaSrc ? (
          <audio
            ref={audioRef}
            src={mediaSrc}
            preload="metadata"
            onTimeUpdate={(event) => setCurrentTime(event.currentTarget.currentTime)}
            onPlay={() => setIsPlaying(true)}
            onPause={() => setIsPlaying(false)}
            onEnded={() => setIsPlaying(false)}
            onLoadedMetadata={(event) => {
              event.currentTarget.playbackRate = speed;
            }}
          />
        ) : null}
        <div
          className="waveform-placeholder"
          onClick={(event) => {
            const rect = event.currentTarget.getBoundingClientRect();
            const ratio = rect.width > 0 ? (event.clientX - rect.left) / rect.width : 0;
            seekAudio(totalDuration * ratio, isPlaying, 'manual_scrub', currentSegmentId ?? undefined);
          }}
        >
          <div className="waveform-bars" aria-hidden="true">
            {displayedWaveformPeaks.map((peak, index) => (
              <span
                key={index}
                style={{ height: `${Math.round(Math.max(0.08, peak) * 100)}%` }}
              />
            ))}
          </div>
          <div
            className="waveform-progress"
            style={{ width: `${totalDuration > 0 ? (currentTime / totalDuration) * 100 : 0}%` }}
          />
        </div>
        <span className="time-display">
          {formatTime(Math.floor(currentTime * 1000))} / {formatTime(Math.floor(totalDuration * 1000))}
        </span>
        <select
          value={speed}
          onChange={(e) => setSpeed(parseFloat(e.target.value))}
        >
          <option value={0.5}>0.5x</option>
          <option value={1}>1x</option>
          <option value={1.5}>1.5x</option>
        </select>
        <span className="shortcuts-hint">
          {t('editor.shortcuts')}
        </span>
      </div>
    </div>
  );
}

// ── Export Page ────────────────────────────────────────────────────────────

function exportExtension(format: string): string {
  const normalized = format.trim().toLowerCase();
  if (normalized === 'markdown') return 'md';
  return normalized;
}

function ExportPage({ jobs }: { jobs: JobInfo[] }) {
  const { t } = useI18n();
  const completedJobs = jobs.filter((job) => job.state === 'completed');
  const [selectedJobId, setSelectedJobId] = useState('');
  const [includeTimestamps, setIncludeTimestamps] = useState(true);
  const [speakerFilter, setSpeakerFilter] = useState<'all' | 'namedOnly' | 'hidden'>('all');
  const [includeMarks, setIncludeMarks] = useState(true);
  const [exportStatus, setExportStatus] = useState<string | null>(null);
  const [exportingFormat, setExportingFormat] = useState<string | null>(null);
  const [copyingFormat, setCopyingFormat] = useState<string | null>(null);
  const effectiveJobId = selectedJobId || completedJobs[completedJobs.length - 1]?.jobId || '';
  const includeSpeakers = speakerFilter !== 'hidden';

  const handleExport = async (format: string) => {
    if (!effectiveJobId) {
      setExportStatus(t('export.noCompletedExport'));
      return;
    }

    setExportingFormat(format);
    setExportStatus(null);
    try {
      const extension = exportExtension(format);
      const selectedJob = completedJobs.find((job) => job.jobId === effectiveJobId);
      const targetPath = await saveDialog({
        title: t('export.saveTitle'),
        defaultPath: `${selectedJob?.fileName?.replace(/\.[^.]+$/, '') || `transcript-${effectiveJobId}`}.${extension}`,
        filters: [
          {
            name: format,
            extensions: [extension],
          },
        ],
      });
      if (!targetPath) {
        setExportStatus(t('export.cancelled'));
        return;
      }
      const path = await invokeTauri<string>('cmd_export_transcript', {
        jobId: effectiveJobId,
        format,
        includeSpeakers,
        includeTimestamps,
        includeMarks,
        speakerFilter,
        outputPath: targetPath,
      });
      setExportStatus(t('export.exportedTo', { path }));
    } catch (error) {
      setExportStatus(errorToMessage(error, t));
    } finally {
      setExportingFormat(null);
    }
  };

  const handleCopy = async (format: string) => {
    if (!effectiveJobId) {
      setExportStatus(t('export.noCompletedCopy'));
      return;
    }
    if (!navigator.clipboard?.writeText) {
      setExportStatus(t('export.clipboardUnavailable'));
      return;
    }

    setCopyingFormat(format);
    setExportStatus(null);
    try {
      const content = await invokeTauri<string>('cmd_render_transcript_export', {
        jobId: effectiveJobId,
        format,
        includeSpeakers,
        includeTimestamps,
        includeMarks,
        speakerFilter,
      });
      await navigator.clipboard.writeText(content);
      setExportStatus(t('export.copied', { format }));
    } catch (error) {
      setExportStatus(errorToMessage(error, t));
    } finally {
      setCopyingFormat(null);
    }
  };

  const formats = [
    { format: 'TXT', desc: t('export.descTxt'), enabled: true, exportable: true, copyable: true },
    { format: 'Markdown', desc: t('export.descMarkdown'), enabled: true, exportable: true, copyable: true },
    { format: 'SRT', desc: t('export.descSrt'), enabled: true, exportable: true, copyable: true },
    { format: 'VTT', desc: t('export.descVtt'), enabled: true, exportable: true, copyable: true },
    { format: 'JSON', desc: t('export.descJson'), enabled: true, exportable: true, copyable: true },
    { format: 'DOCX', desc: t('export.descDocx'), enabled: true, exportable: true, copyable: false },
    { format: 'Obsidian', desc: t('export.descObsidian'), enabled: true, exportable: false, copyable: true },
    { format: 'Notion', desc: t('export.descNotion'), enabled: true, exportable: false, copyable: true },
  ];

  return (
    <div className="page export-page">
      <h2>{t('export.title')}</h2>
      <div className="export-toolbar">
        <label>
          {t('export.job')}
          <select
            value={effectiveJobId}
            onChange={(event) => setSelectedJobId(event.target.value)}
            disabled={completedJobs.length === 0}
          >
            {completedJobs.length === 0 ? (
              <option value="">{t('export.noCompletedJobs')}</option>
            ) : (
              completedJobs.map((job) => (
                <option key={job.jobId} value={job.jobId}>
                  {job.fileName}
                </option>
              ))
            )}
          </select>
        </label>
        <label><input type="checkbox" checked={includeTimestamps} onChange={(event) => setIncludeTimestamps(event.target.checked)} /> {t('export.timestamps')}</label>
        <label>
          {t('export.speakers')}
          <select
            value={speakerFilter}
            onChange={(event) => setSpeakerFilter(event.target.value as 'all' | 'namedOnly' | 'hidden')}
          >
            <option value="all">{t('export.speakersAll')}</option>
            <option value="namedOnly">{t('export.speakersNamed')}</option>
            <option value="hidden">{t('export.speakersHidden')}</option>
          </select>
        </label>
        <label><input type="checkbox" checked={includeMarks} onChange={(event) => setIncludeMarks(event.target.checked)} /> {t('export.marks')}</label>
      </div>
      {exportStatus && <p className="export-status">{exportStatus}</p>}
      <div className="export-grid">
        {formats.map(({ format, desc, enabled, exportable, copyable }) => (
          <div key={format} className="export-card">
            <h3>.{format.toLowerCase()}</h3>
            <p>{desc}</p>
            <div className="export-card-actions">
              {exportable && (
                <button
                  className="btn-primary btn-sm"
                  disabled={!enabled || completedJobs.length === 0 || exportingFormat === format}
                  onClick={() => handleExport(format)}
                >
                  {exportingFormat === format ? t('export.exporting') : t('export.export')}
                </button>
              )}
              {copyable && (
                <button
                  className="btn-secondary btn-sm"
                  disabled={!enabled || completedJobs.length === 0 || copyingFormat === format}
                  onClick={() => handleCopy(format)}
                >
                  {copyingFormat === format ? t('export.copying') : t('export.copy')}
                </button>
              )}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

// ── Settings Page ──────────────────────────────────────────────────────────

function SettingsPage({
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
  const [runtimeHealthRefreshing, setRuntimeHealthRefreshing] = useState(false);
  const [runtimeRepairAction, setRuntimeRepairAction] = useState<string | null>(null);
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

  const handleRepairRuntimeDependency = async (item: RuntimeDependency) => {
    setRuntimeRepairAction(item.id);
    setRuntimeHealthStatus(t('runtime.repairingItem', { item: runtimeDependencyLabel(item.id, t) }));
    try {
      const result = await invokeTauri<RuntimeRepairResult>('cmd_repair_runtime_dependency', {
        id: item.id,
      });
      setRuntimeHealth(result.health);
      setRuntimeHealthStatus(result.message);
      onRefreshModelSettings();
      refreshDiagnosticsPreview();
    } catch (error) {
      setRuntimeHealthStatus(errorToMessage(error, t));
      void refreshRuntimeHealth();
    } finally {
      setRuntimeRepairAction(null);
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
  }, [refreshDiagnosticsPreview, refreshGlossaryEntries, refreshModelCatalog, refreshRuntimeHealth, onRefreshModelSettings, t, telemetryConsent]);

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

export default App;
