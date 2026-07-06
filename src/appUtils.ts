import type {
  AsrEngine,
  AudioMode,
  BackendJobStatus,
  EffectiveTranscriptionPlan,
  JobInfo,
  JobLogEntry,
  MediaFileInfo,
  ModelInfo,
  PersistedJobSummary,
  RuntimeDependencyKind,
  RuntimeDependencyStatus,
  TelemetryConsentState,
  TelemetryEventPayload,
} from './types';
import { hasTauriRuntime, invokeTauri, type Translate } from './tauri';

const TELEMETRY_CONSENT_STORAGE_KEY = 'audraflow.telemetryConsent';

export function createTelemetryConsentState(enabled: boolean): TelemetryConsentState {
  return {
    enabled,
    decided: true,
    updatedAtMs: Date.now(),
  };
}

export function readBrowserTelemetryConsent(): TelemetryConsentState | null {
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

export function writeBrowserTelemetryConsent(enabled: boolean): TelemetryConsentState {
  const state = createTelemetryConsentState(enabled);

  if (typeof window !== 'undefined') {
    window.localStorage.setItem(TELEMETRY_CONSENT_STORAGE_KEY, JSON.stringify(state));
  }

  return state;
}

export type TauriDroppedFile = File & { path?: string };

export function filePathFromDrop(file: File): string {
  return (file as TauriDroppedFile).path || file.name;
}

export function fileNameFromSource(source: string): string {
  try {
    const url = new URL(source);
    const name = url.pathname.split('/').filter(Boolean).pop();
    return name || url.hostname;
  } catch {
    return source.split(/[\\/]/).pop() || source;
  }
}

export function createLog(level: JobLogEntry['level'], message: string): JobLogEntry {
  return {
    id: `${Date.now()}-${Math.random().toString(16).slice(2)}`,
    time: new Date().toLocaleTimeString(),
    level,
    message,
  };
}

export function parseTimeToSeconds(value: string): number | null {
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

export function formatTimeInput(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return '0';
  const rounded = Math.max(0, Math.round(seconds));
  const minutes = Math.floor(rounded / 60);
  const remainingSeconds = rounded % 60;
  return `${minutes}:${String(remainingSeconds).padStart(2, '0')}`;
}

export function formatTime(ms: number): string {
  const s = Math.floor(ms / 1000);
  const m = Math.floor(s / 60);
  return `${m.toString().padStart(2, '0')}:${(s % 60).toString().padStart(2, '0')}`;
}

export function stateLabel(state: JobInfo['state'], t: Translate) {
  return t(`state.${state}`);
}

export function runtimeDependencyLabel(id: string, t: Translate): string {
  return t(`runtime.item.${id}`);
}

export function runtimeDependencyHint(id: string, t: Translate): string {
  return t(`runtime.hint.${id}`);
}

export function runtimeDependencyFix(id: string, t: Translate): string {
  return t(`runtime.fix.${id}`);
}

export function runtimeComponentLabel(id: string, t: Translate): string {
  return t(`runtime.component.${id}`);
}

export function runtimeComponentHint(id: string, t: Translate): string {
  return t(`runtime.componentHint.${id}`);
}

export function runtimeStatusLabel(status: RuntimeDependencyStatus, t: Translate): string {
  return t(`runtime.status.${status}`);
}

export function runtimeKindLabel(kind: RuntimeDependencyKind, t: Translate): string {
  return t(`runtime.kind.${kind}`);
}

export function jobStatusMessage(status: BackendJobStatus, t: Translate): string {
  if (status.state === 'completed') {
    return status.message && status.message !== 'Completed'
      ? status.message
      : t('state.transcriptReady');
  }
  return status.message || stateLabel(status.state, t);
}

export function jobPhaseFromStatus(status: BackendJobStatus, previousPhase?: string): string | undefined {
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

export function jobProgressFromStatus(status: BackendJobStatus): number {
  if (status.state === 'completed') return 100;
  return Math.max(0, Math.min(100, status.progressPct));
}

export function normalizeJobState(value: string): JobInfo['state'] {
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

export function parseSqliteTimestampMs(value?: string | null): number | undefined {
  if (!value) return undefined;
  const normalized = value.includes('T') ? value : `${value.replace(' ', 'T')}Z`;
  const ms = Date.parse(normalized);
  return Number.isFinite(ms) ? ms : undefined;
}

export function persistedJobToJobInfo(job: PersistedJobSummary, t: Translate): JobInfo {
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

export function mergePersistedJobInfo(existing: JobInfo, restored: JobInfo): JobInfo {
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

export function mergePersistedJobs(current: JobInfo[], restored: JobInfo[]): JobInfo[] {
  const currentById = new Map(current.map((job) => [job.jobId, job]));
  const restoredIds = new Set(restored.map((job) => job.jobId));
  const mergedRestored = restored.map((job) => {
    const existing = currentById.get(job.jobId);
    return existing ? mergePersistedJobInfo(existing, job) : job;
  });
  const unsavedJobs = current.filter((job) => !restoredIds.has(job.jobId));
  return [...mergedRestored, ...unsavedJobs];
}

export interface BatchQueueReport {
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

export function isTerminalJob(state: JobInfo['state']): boolean {
  return ['completed', 'cancelled', 'failed', 'notFound'].includes(state);
}

export function buildBatchQueueReport(jobs: JobInfo[]): BatchQueueReport {
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

export function recordTelemetry(payload: TelemetryEventPayload): void {
  if (!hasTauriRuntime()) return;
  void invokeTauri<void>('cmd_record_telemetry_event', { request: payload }).catch(() => {});
}

export function formatPercent(value: number): string {
  return `${value.toFixed(value >= 10 ? 0 : 1)}%`;
}

export function formatRtf(value: number | null): string {
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

export function isSupportedLocalMediaPath(path: string): boolean {
  const ext = path.split(/[\\/]/).pop()?.split('.').pop()?.toLowerCase();
  return !!ext && LOCAL_MEDIA_EXTENSIONS.has(ext);
}

export function formatDurationSeconds(seconds?: number | null): string {
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

export function formatFileSize(bytes?: number): string {
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

export function whisperModelNameMatchesPreference(name: string, preference: string): boolean {
  const normalized = name.trim().toLowerCase();
  if (preference === 'large' || preference === 'medium') {
    return normalized.startsWith(preference);
  }
  return normalized === preference || normalized.startsWith(`${preference}-`);
}

export function preferredLyricsWhisperModel(
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

export function buildEffectiveTranscriptionPlan(
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

export async function inspectLocalMediaFiles(files: string[]): Promise<MediaFileInfo[]> {
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

export async function runWithConcurrency<T>(
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
