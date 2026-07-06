//! AudraFlow IPC Message Types
//!
//! Defines all messages exchanged between UI ↔ Orchestrator ↔ ASR Runtime.
//! Based on PRD §13.4 IPC Message Contract.
//!
//! Transport: JSON over Named Pipe (Windows) / Unix Domain Socket (macOS later).
//! Every message carries `message_id` (UUID v4) and `timestamp` (Unix ms).

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Transcript Schema (PRD §13.2) ──────────────────────────────────────────

/// A time-stamped segment of transcribed text.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub segment_id: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub speaker_id: Option<String>,
    pub text: String,
    pub raw_text: String,
    pub confidence: f64,
    pub low_confidence_reasons: Vec<String>,
    pub corrections: Vec<Correction>,
    pub marks: Vec<TimestampMark>,
}

/// A correction applied to a segment (by post-processor or user).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Correction {
    pub field: String,
    pub old_value: String,
    pub new_value: String,
    pub source: CorrectionSource,
    pub auto_applied: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CorrectionSource {
    Lexicon,
    User,
    Merge,
}

/// A timestamp mark inserted by the user (Ctrl+T).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimestampMark {
    pub mark_ms: i64,
    pub label: Option<String>,
    pub note: Option<String>,
}

// ── Core IPC Messages ──────────────────────────────────────────────────────

/// Every IPC message wraps a payload with routing metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcEnvelope {
    pub message_id: String,
    pub timestamp_ms: i64,
    #[serde(flatten)]
    pub payload: IpcMessage,
}

impl IpcEnvelope {
    pub fn new(payload: IpcMessage) -> Self {
        Self {
            message_id: Uuid::new_v4().to_string(),
            timestamp_ms: Utc::now().timestamp_millis(),
            payload,
        }
    }
}

/// All IPC message variants (PRD §13.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum IpcMessage {
    // ── Job lifecycle ──
    JobCreate(JobCreate),
    JobStatus(JobStatus),
    JobCancel(JobControl),
    JobPause(JobControl),
    JobResume(JobControl),
    JobRetry(JobControl),
    JobSkip(JobControl),

    // ── Streaming ──
    SegmentStream(SegmentStream),

    // ── User edits ──
    SegmentUpdate(SegmentUpdate),

    // ── Post-processing ──
    CorrectionApply(CorrectionApply),

    // ── Diarization ──
    DiarizationResult(DiarizationResult),

    // ── Errors ──
    ErrorReport(ErrorReport),

    // ── Checkpoint ──
    CheckpointSave(CheckpointEvent),
    CheckpointRestore(CheckpointEvent),

    // ── Export ──
    ExportRequest(ExportRequest),
    ExportComplete(ExportComplete),

    // ── Diagnostics ──
    DiagnosticsRequest(DiagnosticsRequest),

    // ── Scheduler plan ──
    JobPlan(JobPlan),
}

// ── Job Messages ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobCreate {
    pub job_id: String,
    pub file_path: String,
    pub file_hash: String,
    pub extreme_accuracy: bool,
    pub export_formats: Vec<String>,
    /// Optional ASR engine. Use "sensevoice" or "whisper".
    pub asr_engine: Option<String>,
    /// Optional selected ASR model path for this job.
    pub model_path: Option<String>,
    /// Optional selected ASR model name.
    pub model_name: Option<String>,
    /// Optional selected ASR model version.
    pub model_version: Option<String>,
    /// Optional language hint passed to whisper.cpp, e.g. zh, en, or auto.
    pub language: Option<String>,
    /// Optional processing mode. Use "music" for lyrics/strong-background music.
    pub audio_mode: Option<String>,
    /// Optional vocal separation mode. Use "demucs" to isolate vocals before ASR.
    pub vocal_separation: Option<String>,
    /// Optional: audio duration in seconds (from pre-scan).
    pub audio_duration_s: Option<f64>,
    /// Optional: SNR estimate from pre-scan.
    pub snr_db: Option<f64>,
    /// Optional: estimated speaker count.
    pub estimated_speakers: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobStatus {
    pub job_id: String,
    pub state: JobState,
    pub progress_pct: f64,
    pub message: Option<String>,
    pub estimated_remaining_s: Option<f64>,
    pub rtf_current: Option<f64>,
    pub ttfv_s: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobState {
    Pending,
    Running,
    Paused,
    Completed,
    Cancelled,
    Failed,
    NotFound,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobControl {
    pub job_id: String,
    pub reason: Option<String>,
}

// ── Segment Streaming ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentStream {
    pub job_id: String,
    pub segments: Vec<Segment>,
    pub is_partial: bool,
}

// ── User Edits ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentUpdate {
    pub segment_id: String,
    pub field: String,
    pub old_value: String,
    pub new_value: String,
    pub source: CorrectionSource,
}

// ── Post-Processing ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CorrectionApply {
    pub segment_id: String,
    pub corrections: Vec<Correction>,
}

// ── Diarization ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiarizationResult {
    pub job_id: String,
    pub speaker_count_estimate: u32,
    pub segments: Vec<SpeakerSegment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeakerSegment {
    pub segment_id: String,
    pub speaker_id: String,
    pub confidence: f64,
}

// ── Error Reporting ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorReport {
    pub job_id: String,
    pub error_code: u16,
    pub error_message: String,
    pub recoverable: bool,
    pub fallback_action: Option<String>,
}

/// Error code ranges (PRD §13.4).
pub mod error_codes {
    pub const FILE_NOT_READABLE: u16 = 1001;
    pub const FORMAT_NOT_SUPPORTED: u16 = 1002;
    pub const DISK_SPACE_LOW: u16 = 1003;

    pub const MODEL_NOT_DOWNLOADED: u16 = 2001;
    pub const GPU_OOM: u16 = 2002;
    pub const INFERENCE_TIMEOUT: u16 = 2003;

    pub const LEXICON_INDEX_CORRUPT: u16 = 3001;
    pub const PUNCTUATION_MODEL_FAILED: u16 = 3002;

    pub const VAD_FAILED: u16 = 4001;
    pub const CLUSTERING_TIMEOUT: u16 = 4002;

    pub const RUNTIME_UNRESPONSIVE: u16 = 5001;
    pub const MESSAGE_FORMAT_ERROR: u16 = 5002;

    pub const UNKNOWN_ERROR: u16 = 9001;
    pub const CRASH_RECOVERY_FAILED: u16 = 9999;
}

// ── Checkpoint ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointEvent {
    pub job_id: String,
    pub checkpoint_id: String,
    pub last_segment_id: String,
    pub timestamp: i64,
}

// ── Export ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    pub job_id: String,
    pub format: ExportFormat,
    pub options: ExportOptions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOptions {
    pub include_speakers: bool,
    pub include_timestamps: bool,
    pub include_marks: bool,
    pub speaker_filter: SpeakerFilter,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SpeakerFilter {
    All,
    NamedOnly,
    Hidden,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportFormat {
    Txt,
    Markdown,
    Srt,
    Vtt,
    Json,
    Docx,
    ClipboardObsidian,
    ClipboardNotion,
    StdoutJson,
    StdoutMarkdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportComplete {
    pub job_id: String,
    pub format: ExportFormat,
    pub file_path: Option<String>,
    pub stdout_ready: bool,
    pub exported_segments_count: u32,
}

// ── Diagnostics ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsRequest {
    pub include_logs: bool,
    pub include_config: bool,
    pub include_device_info: bool,
}

// ── Scheduler Plan ─────────────────────────────────────────────────────────

/// Result of running the adaptive scheduler for a job.
/// Returned to the UI so the user sees the estimated processing time.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobPlan {
    pub job_id: String,
    pub plan_id: String,
    pub model_size: String,
    pub estimated_seconds: f64,
    pub explanation: String,
    pub fallback_reason: Option<String>,
}
