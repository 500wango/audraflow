export type Page = 'import' | 'queue' | 'editor' | 'export' | 'settings';
export type WindowControlAction = 'close' | 'minimize' | 'zoom';

// ── Types ─────────────────────────────────────────────────────────────────

export interface JobInfo {
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

export type QueueAction = 'pause' | 'resume' | 'cancel' | 'retry' | 'skip' | 'open';

export interface BackendJobStatus {
  jobId: string;
  state: JobInfo['state'];
  progressPct: number;
  message?: string | null;
  estimatedRemainingS?: number | null;
  rtfCurrent?: number | null;
  ttfvS?: number | null;
}

export interface JobLogEntry {
  id: string;
  time: string;
  level: 'info' | 'warn' | 'error';
  message: string;
}

export interface BackendJobLogEvent {
  jobId: string;
  level: JobLogEntry['level'];
  message: string;
}

export interface BackendJobProgressEvent {
  jobId: string;
  phase: string;
  progressPct: number;
  message: string;
}

export interface ModelDownloadProgressEvent {
  id: string;
  downloadedBytes: number;
  totalBytes: number;
  progressPct: number;
  message: string;
}

export interface RuntimeComponentProgressEvent {
  id: string;
  downloadedBytes: number;
  totalBytes: number;
  progressPct: number;
  message: string;
}

export interface UrlPreviewResponse {
  filePath: string;
  previewSeconds: number;
  source: string;
  message: string;
}

export interface MediaFileInfo {
  filePath: string;
  fileName: string;
  format: string;
  sizeBytes: number;
  durationSeconds?: number | null;
}

export type AudioMode = 'speech' | 'music';
export type AsrEngine = 'auto' | 'sensevoice' | 'whisper' | 'funasr';
export type TranscriptionLanguage = 'auto' | 'zh' | 'en';
export type VocalSeparationMode = 'off' | 'demucs';

export interface PersistedJobSummary {
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

export interface TranscriptResponse {
  jobId: string;
  filePath: string;
  mediaSrcPath: string;
  segments: TranscriptSegment[];
}

export interface PlatformDownloadOptions {
  audioQuality: 'auto' | 'small' | 'medium' | 'best';
  audioFormat: 'source' | 'mp3' | 'm4a' | 'wav';
  skipStartSeconds: number;
  asrEngine: AsrEngine;
  language: TranscriptionLanguage;
  audioMode: AudioMode;
  vocalSeparation: VocalSeparationMode;
}

export interface TelemetryConsentState {
  enabled: boolean;
  decided: boolean;
  updatedAtMs?: number | null;
}

export interface PrivacyActionResult {
  message: string;
  bytesFreed: number;
  itemsAffected: number;
}

export interface DiagnosticsPreview {
  fields: string[];
  localHistoryBytes: number;
  telemetryEventsBytes: number;
  modelCacheBytes: number;
  modelCacheItems: number;
  telemetryEnabled: boolean;
}

export interface DeviceDiagnostics {
  cpuCores: number;
  cudaAvailable: boolean;
  vramGb?: number | null;
  gpuModel?: string | null;
  cudaVersion?: string | null;
  driverVersion?: string | null;
  deviceTier: string;
  fallbackMessage?: string | null;
}

export type RuntimeDependencyStatus = 'ready' | 'missing' | 'warning';
export type RuntimeDependencyKind = 'required' | 'recommended' | 'optional' | 'experimental';

export interface RuntimeDependency {
  id: string;
  status: RuntimeDependencyStatus;
  kind: RuntimeDependencyKind;
  path?: string | null;
  version?: string | null;
  detail?: string | null;
  repairable: boolean;
}

export interface RuntimeHealth {
  generatedAtMs: number;
  blockingCount: number;
  warningCount: number;
  items: RuntimeDependency[];
}

export interface RuntimeRepairResult {
  id: string;
  message: string;
  health: RuntimeHealth;
}

export interface RuntimeComponent {
  id: string;
  status: RuntimeDependencyStatus;
  kind: RuntimeDependencyKind;
  installDir: string;
  downloadUrl?: string | null;
  downloadSizeBytes: number;
  installedSizeBytes: number;
  requiredFiles: string[];
  detail?: string | null;
  installable: boolean;
}

export interface RuntimeComponentActionResult {
  id: string;
  message: string;
  components: RuntimeComponent[];
  health: RuntimeHealth;
}

export interface ModelInfo {
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

export interface ModelCatalogEntry {
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

export interface ModelSettings {
  modelsDir: string;
  selectedModel?: ModelInfo | null;
  installedModels: ModelInfo[];
}

export interface ModelActionResult {
  message: string;
  bytesFreed: number;
  itemsAffected: number;
  settings: ModelSettings;
}

export interface EffectiveTranscriptionPlan {
  engine: AsrEngine;
  model?: ModelInfo | null;
  ready: boolean;
  reasonKey: string;
}

export interface GlossaryAlias {
  id: number;
  alias: string;
  pinyin?: string | null;
}

export interface GlossaryEntry {
  id: number;
  canonical: string;
  category?: string | null;
  enabled: boolean;
  createdAt: string;
  aliases: GlossaryAlias[];
}

export type TelemetryEventPayload = Record<string, string | number | boolean | null | undefined>;

export interface TranscriptSegment {
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

export interface GlossaryApplyResult {
  updatedSegments: TranscriptSegment[];
  updatedCount: number;
  entry: GlossaryEntry;
}

export interface TimestampMark {
  id: number;
  segmentId: string;
  markMs: number;
  label?: string | null;
  note?: string | null;
}
