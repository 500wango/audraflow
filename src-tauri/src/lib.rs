//! AudraFlow Tauri Application
//!
//! Desktop app providing the UI layer for the AudraFlow product.
//! Communicates with the Orchestrator and ASR Runtime via local IPC.

use audraflow_ipc::{
    Correction, CorrectionSource, IpcEnvelope, IpcMessage, JobControl, JobCreate, JobPlan,
    JobState, JobStatus, Segment, TimestampMark,
};
use audraflow_licensing::{LicenseManager, LicenseState};
use audraflow_scheduler::{DeviceTier, Scheduler, SchedulerInput};
use audraflow_storage::SegmentRow;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
#[allow(unused_imports)]
use tauri::{Emitter, Manager};

#[cfg(target_os = "windows")]
const ORCHESTRATOR_PIPE: &str = r"\\.\pipe\audraflow-orchestrator";
const MAX_REMOTE_MEDIA_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const REMOTE_MEDIA_TIMEOUT_SECS: u64 = 300;
const PLATFORM_DOWNLOAD_TIMEOUT_SECS: u64 = 900;
const ORCHESTRATOR_STARTUP_TIMEOUT_SECS: u64 = 8;
const MAX_SKIP_START_SECONDS: f64 = 12.0 * 60.0 * 60.0;
const DEFAULT_URL_PREVIEW_SECONDS: f64 = 120.0;
const MAX_URL_PREVIEW_SECONDS: f64 = 300.0;
const URL_PREVIEW_TIMEOUT_SECS: u64 = 240;
const WHISPER_CPP_MODEL_COMMIT: &str = "5359861c739e955e79d9a303bcbc70fb988958b1";
const WHISPER_CPP_MODEL_BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve";
const BUNDLED_DEFAULT_MODEL_RESOURCE: &str = "default-models/ggml-base.bin";
const DEFAULT_WHISPER_MODEL_NAME: &str = "base";
const DEFAULT_WHISPER_MODEL_SIZE_BYTES: u64 = 147_951_465;
const DEFAULT_WHISPER_MODEL_SHA256: &str =
    "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct JobLogEvent {
    job_id: String,
    level: &'static str,
    message: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct JobProgressEvent {
    job_id: String,
    phase: &'static str,
    progress_pct: f64,
    message: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelDownloadProgressEvent {
    id: String,
    downloaded_bytes: u64,
    total_bytes: u64,
    progress_pct: f64,
    message: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateJobFromUrlRequest {
    client_job_id: String,
    url: String,
    audio_quality: Option<String>,
    audio_format: Option<String>,
    skip_start_seconds: Option<f64>,
    asr_engine: Option<String>,
    language: Option<String>,
    audio_mode: Option<String>,
    vocal_separation: Option<String>,
    extreme_accuracy: bool,
    export_formats: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UrlPreviewRequest {
    url: String,
    preview_seconds: Option<f64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UrlPreviewResponse {
    file_path: String,
    preview_seconds: f64,
    source: &'static str,
    message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MediaFileInfo {
    file_path: String,
    file_name: String,
    format: String,
    size_bytes: u64,
    duration_seconds: Option<f64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JobSummaryDto {
    job_id: String,
    file_path: String,
    file_name: String,
    format: String,
    size_bytes: u64,
    duration_seconds: Option<f64>,
    state: String,
    extreme_accuracy: bool,
    segment_count: u32,
    created_at: String,
    completed_at: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TranscriptSegmentDto {
    id: String,
    start_ms: i64,
    end_ms: i64,
    speaker: String,
    text: String,
    raw_text: String,
    confidence: f64,
    low_confidence_reasons: Vec<String>,
    has_correction: bool,
    has_mark: bool,
    marks: Vec<TimestampMarkDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TimestampMarkDto {
    id: i64,
    segment_id: String,
    mark_ms: i64,
    label: Option<String>,
    note: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TranscriptResponse {
    job_id: String,
    file_path: String,
    media_src_path: String,
    segments: Vec<TranscriptSegmentDto>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateSegmentRequest {
    segment_id: String,
    text: Option<String>,
    speaker: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateSpeakerLabelRequest {
    job_id: String,
    from_speaker: String,
    to_speaker: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AcceptTermCandidateRequest {
    segment_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddGlossaryEntryRequest {
    job_id: Option<String>,
    canonical: String,
    aliases: Vec<String>,
    category: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveGlossaryEntryRequest {
    id: Option<i64>,
    canonical: String,
    aliases: Vec<String>,
    category: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GlossaryAliasDto {
    id: i64,
    alias: String,
    pinyin: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GlossaryEntryDto {
    id: i64,
    canonical: String,
    category: Option<String>,
    enabled: bool,
    created_at: String,
    aliases: Vec<GlossaryAliasDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GlossaryApplyResult {
    entry: GlossaryEntryDto,
    updated_segments: Vec<TranscriptSegmentDto>,
    updated_count: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddTimestampMarkRequest {
    segment_id: String,
    mark_ms: i64,
    label: Option<String>,
    note: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TelemetryEventRequest {
    event_type: String,
    job_id: Option<String>,
    segment_id: Option<String>,
    audio_hours: Option<f64>,
    transcript_chars: Option<u32>,
    active_seconds: Option<f64>,
    inactive_seconds: Option<f64>,
    completed_ratio: Option<f64>,
    op_type: Option<String>,
    chars_before: Option<u32>,
    chars_after: Option<u32>,
    source: Option<String>,
    from_ms: Option<i64>,
    to_ms: Option<i64>,
    trigger: Option<String>,
    mark_ms: Option<i64>,
    label_type: Option<String>,
    format: Option<String>,
    include_timestamps: Option<bool>,
    include_speakers: Option<bool>,
    include_marks: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TelemetryConsentState {
    enabled: bool,
    decided: bool,
    updated_at_ms: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetTelemetryConsentRequest {
    enabled: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PrivacyActionResult {
    message: String,
    bytes_freed: u64,
    items_affected: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticsPreview {
    fields: Vec<String>,
    local_history_bytes: u64,
    telemetry_events_bytes: u64,
    model_cache_bytes: u64,
    model_cache_items: u64,
    telemetry_enabled: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceDiagnosticsDto {
    cpu_cores: u32,
    cuda_available: bool,
    vram_gb: Option<f64>,
    gpu_model: Option<String>,
    cuda_version: Option<String>,
    driver_version: Option<String>,
    device_tier: String,
    fallback_message: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeHealthDto {
    generated_at_ms: i64,
    blocking_count: u32,
    warning_count: u32,
    items: Vec<RuntimeDependencyDto>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeDependencyDto {
    id: String,
    status: String,
    kind: String,
    path: Option<String>,
    version: Option<String>,
    detail: Option<String>,
    repairable: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeRepairResultDto {
    id: String,
    message: String,
    health: RuntimeHealthDto,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportLocalModelRequest {
    file_path: String,
    name: Option<String>,
    version: Option<String>,
    language: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DownloadModelRequest {
    url: String,
    sha256: String,
    size_bytes: u64,
    name: Option<String>,
    version: Option<String>,
    language: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SelectModelRequest {
    name: String,
    version: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteModelRequest {
    name: String,
    version: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelInfoDto {
    name: String,
    version: String,
    language: String,
    size_bytes: u64,
    sha256: String,
    path: String,
    installed_at_ms: i64,
    selected: bool,
    bundled: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelSettingsDto {
    models_dir: String,
    selected_model: Option<ModelInfoDto>,
    installed_models: Vec<ModelInfoDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelActionResult {
    message: String,
    bytes_freed: u64,
    items_affected: u64,
    settings: ModelSettingsDto,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelCatalogEntryDto {
    name: String,
    version: String,
    language: String,
    size_bytes: u64,
    sha256: String,
    download_url: String,
    description: String,
    recommended: bool,
    installed: bool,
    selected: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalTelemetryRecord {
    event_type: String,
    timestamp_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    job_id_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    segment_id_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    audio_hours: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transcript_chars: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    active_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inactive_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    completed_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    op_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chars_before: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chars_after: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    from_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    to_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    trigger: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mark_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    include_timestamps: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    include_speakers: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    include_marks: Option<bool>,
}

fn emit_job_log(
    app_handle: &tauri::AppHandle,
    job_id: &str,
    level: &'static str,
    message: impl Into<String>,
) {
    let message = message.into();
    log::info!("[job:{job_id}] {message}");
    let _ = app_handle.emit(
        "job://log",
        JobLogEvent {
            job_id: job_id.to_string(),
            level,
            message,
        },
    );
}

fn emit_job_progress(
    app_handle: &tauri::AppHandle,
    job_id: &str,
    phase: &'static str,
    progress_pct: f64,
    message: impl Into<String>,
) {
    let _ = app_handle.emit(
        "job://progress",
        JobProgressEvent {
            job_id: job_id.to_string(),
            phase,
            progress_pct: progress_pct.clamp(0.0, 100.0),
            message: message.into(),
        },
    );
}

fn emit_model_download_progress(
    app_handle: &tauri::AppHandle,
    id: &str,
    downloaded_bytes: u64,
    total_bytes: u64,
    message: impl Into<String>,
) {
    let progress_pct = if total_bytes > 0 {
        downloaded_bytes as f64 / total_bytes as f64 * 100.0
    } else {
        0.0
    };
    let _ = app_handle.emit(
        "model://download-progress",
        ModelDownloadProgressEvent {
            id: id.to_string(),
            downloaded_bytes,
            total_bytes,
            progress_pct: progress_pct.clamp(0.0, 100.0),
            message: message.into(),
        },
    );
}

fn expect_job_status(message: IpcMessage) -> Result<JobStatus, String> {
    match message {
        IpcMessage::JobStatus(status) => Ok(status),
        other => Err(format!("Unexpected orchestrator response: {other:?}")),
    }
}

fn storage_db_path() -> Result<PathBuf, String> {
    let dir = app_data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("audraflow.db"))
}

fn app_data_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("AudraFlow");
    }

    #[cfg(not(target_os = "windows"))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
            .unwrap_or_else(|| PathBuf::from("."))
            .join("com.audraflow.app")
    }
}

fn segment_to_dto(
    storage: &audraflow_storage::Storage,
    segment: SegmentRow,
) -> Result<TranscriptSegmentDto, String> {
    let mut low_confidence_reasons = storage
        .get_low_confidence_reasons(&segment.segment_id)
        .map_err(|e| e.to_string())?;
    if low_confidence_reasons.is_empty() && segment.confidence < 0.8 {
        low_confidence_reasons.push("low_confidence".into());
    }

    let has_recorded_correction = storage
        .segment_has_corrections(&segment.segment_id)
        .map_err(|e| e.to_string())?;
    let has_mark = storage
        .segment_has_marks(&segment.segment_id)
        .map_err(|e| e.to_string())?;
    let marks = storage
        .get_marks(&segment.segment_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|mark| TimestampMarkDto {
            id: mark.id,
            segment_id: mark.segment_id,
            mark_ms: mark.mark_ms,
            label: mark.label,
            note: mark.note,
        })
        .collect();
    let has_correction = has_recorded_correction || segment.text != segment.raw_text;

    Ok(TranscriptSegmentDto {
        id: segment.segment_id,
        start_ms: segment.start_ms,
        end_ms: segment.end_ms,
        speaker: segment.speaker_id.unwrap_or_else(|| "Speaker".into()),
        text: segment.text,
        raw_text: segment.raw_text,
        confidence: segment.confidence,
        low_confidence_reasons,
        has_correction,
        has_mark,
        marks,
    })
}

fn normalize_fts_query(query: &str) -> String {
    let escaped = query.trim().replace('"', "\"\"");
    if escaped.is_empty() {
        return String::new();
    }
    format!("\"{escaped}\"")
}

fn filter_segments_by_text(segments: &[SegmentRow], query: &str) -> Vec<SegmentRow> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }

    segments
        .iter()
        .filter(|segment| {
            segment.text.to_lowercase().contains(&needle)
                || segment.raw_text.to_lowercase().contains(&needle)
                || segment
                    .speaker_id
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&needle)
        })
        .cloned()
        .collect()
}

fn search_transcript_segments(
    storage: &audraflow_storage::Storage,
    job_id: &str,
    query: &str,
) -> Result<Vec<TranscriptSegmentDto>, String> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    let all_segments = storage.get_segments(job_id).map_err(|e| e.to_string())?;
    let fts_query = normalize_fts_query(trimmed);
    let fts_matches = if fts_query.is_empty() {
        Vec::new()
    } else {
        storage
            .search_segments(job_id, &fts_query)
            .unwrap_or_default()
    };

    let matched_segments = if fts_matches.is_empty() {
        filter_segments_by_text(&all_segments, trimmed)
    } else {
        fts_matches
            .iter()
            .filter_map(|segment_id| {
                all_segments
                    .iter()
                    .find(|segment| &segment.segment_id == segment_id)
                    .cloned()
            })
            .collect()
    };

    matched_segments
        .into_iter()
        .map(|segment| segment_to_dto(storage, segment))
        .collect()
}

fn glossary_entry_to_dto(entry: audraflow_storage::GlossaryEntryRow) -> GlossaryEntryDto {
    GlossaryEntryDto {
        id: entry.id,
        canonical: entry.canonical,
        category: entry.category,
        enabled: entry.enabled,
        created_at: entry.created_at,
        aliases: entry
            .aliases
            .into_iter()
            .map(|alias| GlossaryAliasDto {
                id: alias.id,
                alias: alias.alias,
                pinyin: alias.pinyin,
            })
            .collect(),
    }
}

fn glossary_entry_to_processor(
    entry: &audraflow_storage::GlossaryEntryRow,
) -> audraflow_post_processor::GlossaryEntry {
    audraflow_post_processor::GlossaryEntry {
        canonical: entry.canonical.clone(),
        aliases: entry
            .aliases
            .iter()
            .map(|alias| alias.alias.clone())
            .collect(),
        pinyin_forms: entry
            .aliases
            .iter()
            .filter_map(|alias| alias.pinyin.clone())
            .collect(),
        category: entry.category.clone(),
        enabled: entry.enabled,
    }
}

fn sanitize_glossary_aliases(canonical: &str, aliases: Vec<String>) -> Vec<String> {
    let mut cleaned = Vec::new();
    for alias in aliases {
        let alias = alias.trim();
        if alias.is_empty() || alias == canonical || cleaned.iter().any(|item| item == alias) {
            continue;
        }
        cleaned.push(alias.to_string());
    }
    cleaned
}

fn apply_glossary_entry_to_job(
    storage: &audraflow_storage::Storage,
    job_id: &str,
    entry: &audraflow_storage::GlossaryEntryRow,
) -> Result<u32, String> {
    let processor_entry = glossary_entry_to_processor(entry);
    if processor_entry.aliases.is_empty() {
        return Ok(0);
    }

    let processor = audraflow_post_processor::PostProcessor::new(vec![processor_entry]);
    let segments = storage.get_segments(job_id).map_err(|e| e.to_string())?;
    let mut updated_count = 0_u32;

    for segment in segments {
        let ipc_segment = storage_segment_to_ipc_segment(storage, &segment, false)?;
        let corrected = processor.apply_to_segment(&ipc_segment);
        if corrected.corrected_text == segment.text {
            continue;
        }

        storage
            .record_correction(
                &segment.segment_id,
                "text",
                &segment.text,
                &corrected.corrected_text,
                &CorrectionSource::Lexicon,
                true,
            )
            .map_err(|e| e.to_string())?;
        storage
            .update_segment(&segment.segment_id, Some(&corrected.corrected_text), None)
            .map_err(|e| e.to_string())?;
        storage
            .remove_low_confidence_reason(&segment.segment_id, "term_conflict")
            .map_err(|e| e.to_string())?;
        updated_count += 1;
    }

    Ok(updated_count)
}

#[cfg(target_os = "windows")]
fn send_orchestrator_message(message: IpcMessage) -> Result<IpcMessage, String> {
    use std::io::{Read, Write};

    let envelope = IpcEnvelope::new(message);
    let payload = serde_json::to_vec(&envelope).map_err(|e| e.to_string())?;

    let deadline = Instant::now() + Duration::from_secs(ORCHESTRATOR_STARTUP_TIMEOUT_SECS);
    let mut pipe = loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(ORCHESTRATOR_PIPE)
        {
            Ok(pipe) => break pipe,
            Err(e) if Instant::now() < deadline => {
                log::debug!("Waiting for orchestrator pipe: {}", e);
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => return Err(format!("Orchestrator is not available: {e}")),
        }
    };

    pipe.write_all(&payload).map_err(|e| e.to_string())?;

    let mut buf = vec![0u8; 65536];
    let n = pipe.read(&mut buf).map_err(|e| e.to_string())?;
    if n == 0 {
        return Err("Orchestrator closed the connection without a response".into());
    }

    let reply: IpcEnvelope = serde_json::from_slice(&buf[..n]).map_err(|e| e.to_string())?;
    Ok(reply.payload)
}

#[cfg(not(target_os = "windows"))]
fn send_orchestrator_message(message: IpcMessage) -> Result<IpcMessage, String> {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    let envelope = IpcEnvelope::new(message);
    let payload = serde_json::to_vec(&envelope).map_err(|e| e.to_string())?;
    let socket_path = orchestrator_socket_path();
    let deadline = Instant::now() + Duration::from_secs(ORCHESTRATOR_STARTUP_TIMEOUT_SECS);
    let mut stream = loop {
        match UnixStream::connect(&socket_path) {
            Ok(stream) => break stream,
            Err(e) if Instant::now() < deadline => {
                log::debug!(
                    "Waiting for orchestrator socket {}: {}",
                    socket_path.display(),
                    e
                );
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => {
                return Err(format!(
                    "Orchestrator is not available at {}: {e}",
                    socket_path.display()
                ));
            }
        }
    };

    stream.write_all(&payload).map_err(|e| e.to_string())?;
    stream
        .shutdown(std::net::Shutdown::Write)
        .map_err(|e| e.to_string())?;

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    if buf.is_empty() {
        return Err("Orchestrator closed the connection without a response".into());
    }

    let reply: IpcEnvelope = serde_json::from_slice(&buf).map_err(|e| e.to_string())?;
    Ok(reply.payload)
}

fn hash_file_sha256(path: &str) -> Result<String, String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path).map_err(|e| format!("Failed to open file: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];

    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("Failed to read file: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

fn default_telemetry_consent() -> TelemetryConsentState {
    TelemetryConsentState {
        enabled: false,
        decided: false,
        updated_at_ms: None,
    }
}

fn telemetry_consent_path(app_handle: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(app_handle
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("privacy")
        .join("telemetry-consent.json"))
}

fn read_telemetry_consent_from_path(path: &Path) -> TelemetryConsentState {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return default_telemetry_consent();
    };
    serde_json::from_str(&raw).unwrap_or_else(|_| default_telemetry_consent())
}

fn write_telemetry_consent_to_path(
    path: &Path,
    enabled: bool,
) -> Result<TelemetryConsentState, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let state = TelemetryConsentState {
        enabled,
        decided: true,
        updated_at_ms: Some(now_unix_ms()),
    };
    let json = serde_json::to_vec_pretty(&state).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(state)
}

fn read_telemetry_consent(app_handle: &tauri::AppHandle) -> Result<TelemetryConsentState, String> {
    Ok(read_telemetry_consent_from_path(&telemetry_consent_path(
        app_handle,
    )?))
}

fn write_telemetry_consent(
    app_handle: &tauri::AppHandle,
    enabled: bool,
) -> Result<TelemetryConsentState, String> {
    write_telemetry_consent_to_path(&telemetry_consent_path(app_handle)?, enabled)
}

fn hash_telemetry_id(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut hasher = Sha256::new();
    hasher.update(trimmed.as_bytes());
    Some(format!("{:x}", hasher.finalize()))
}

fn sanitize_telemetry_token(value: Option<String>) -> Option<String> {
    value
        .map(|raw| {
            raw.chars()
                .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
                .take(48)
                .collect::<String>()
                .to_ascii_lowercase()
        })
        .filter(|value| !value.is_empty())
}

fn normalize_telemetry_event_type(event_type: &str) -> Result<String, String> {
    let normalized = event_type.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "proofread_session_start"
        | "proofread_session_end"
        | "correction_event"
        | "playback_seek"
        | "timestamp_mark"
        | "export_completed" => Ok(normalized),
        _ => Err(format!("Unsupported telemetry event type: {event_type}")),
    }
}

fn telemetry_request_to_record(
    request: TelemetryEventRequest,
) -> Result<LocalTelemetryRecord, String> {
    let event_type = normalize_telemetry_event_type(&request.event_type)?;
    Ok(LocalTelemetryRecord {
        event_type,
        timestamp_ms: now_unix_ms(),
        job_id_hash: request.job_id.as_deref().and_then(hash_telemetry_id),
        segment_id_hash: request.segment_id.as_deref().and_then(hash_telemetry_id),
        audio_hours: request.audio_hours.map(|value| value.max(0.0)),
        transcript_chars: request.transcript_chars,
        active_seconds: request.active_seconds.map(|value| value.max(0.0)),
        inactive_seconds: request.inactive_seconds.map(|value| value.max(0.0)),
        completed_ratio: request.completed_ratio.map(|value| value.clamp(0.0, 1.0)),
        op_type: sanitize_telemetry_token(request.op_type),
        chars_before: request.chars_before,
        chars_after: request.chars_after,
        source: sanitize_telemetry_token(request.source),
        from_ms: request.from_ms,
        to_ms: request.to_ms,
        trigger: sanitize_telemetry_token(request.trigger),
        mark_ms: request.mark_ms,
        label_type: sanitize_telemetry_token(request.label_type),
        format: sanitize_telemetry_token(request.format),
        include_timestamps: request.include_timestamps,
        include_speakers: request.include_speakers,
        include_marks: request.include_marks,
    })
}

fn append_local_telemetry(
    app_handle: &tauri::AppHandle,
    record: &LocalTelemetryRecord,
) -> Result<(), String> {
    use std::io::Write;

    let dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("telemetry");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("events.jsonl");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut file, record).map_err(|e| e.to_string())?;
    file.write_all(b"\n").map_err(|e| e.to_string())
}

fn telemetry_events_path(app_handle: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(app_handle
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("telemetry")
        .join("events.jsonl"))
}

fn model_cache_dir(app_handle: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(app_handle
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("models"))
}

fn model_manager(
    app_handle: &tauri::AppHandle,
) -> Result<audraflow_model_manager::ModelManager, String> {
    let manager = audraflow_model_manager::ModelManager::new(model_cache_dir(app_handle)?);
    manager.init().map_err(|e| e.to_string())?;
    Ok(manager)
}

fn whisper_cpp_model_version() -> String {
    format!("whisper.cpp-{WHISPER_CPP_MODEL_COMMIT}")
}

fn bundled_default_model_info() -> audraflow_model_manager::ModelInfo {
    audraflow_model_manager::ModelInfo {
        name: DEFAULT_WHISPER_MODEL_NAME.into(),
        version: whisper_cpp_model_version(),
        language: "auto".into(),
        size_bytes: DEFAULT_WHISPER_MODEL_SIZE_BYTES,
        sha256: DEFAULT_WHISPER_MODEL_SHA256.into(),
        download_url: "bundled".into(),
        model_type: audraflow_model_manager::ModelType::WhisperCpp,
    }
}

fn is_bundled_default_model_info(info: &audraflow_model_manager::ModelInfo) -> bool {
    info.name == DEFAULT_WHISPER_MODEL_NAME && info.version == whisper_cpp_model_version()
}

fn ensure_bundled_default_model(
    app_handle: &tauri::AppHandle,
    manager: &audraflow_model_manager::ModelManager,
) -> Result<Option<audraflow_model_manager::InstalledModel>, String> {
    let Some(source) = find_bundled_default_model(app_handle) else {
        return Ok(None);
    };

    let info = bundled_default_model_info();
    if let Some(installed) = manager
        .list_installed_models()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|model| is_bundled_default_model_info(&model.info) && model.path.is_file())
    {
        if manager
            .selected_model()
            .map_err(|e| e.to_string())?
            .is_none()
        {
            manager
                .select_model(&installed.info.name, &installed.info.version)
                .map_err(|e| e.to_string())?;
        }
        return Ok(Some(installed));
    }

    let destination = manager.model_path(&info);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp_path = destination.with_extension("bin.tmp");
    let _ = std::fs::remove_file(&tmp_path);
    std::fs::copy(&source, &tmp_path).map_err(|e| {
        format!(
            "Failed to copy bundled default model from {}: {e}",
            source.display()
        )
    })?;
    let _ = std::fs::remove_file(&destination);
    std::fs::rename(&tmp_path, &destination)
        .map_err(|e| format!("Failed to install bundled default model: {e}"))?;

    let installed = manager
        .register_installed_model(&info)
        .map_err(|e| format!("Bundled default model failed validation: {e}"))?;
    if manager
        .selected_model()
        .map_err(|e| e.to_string())?
        .is_none()
    {
        manager
            .select_model(&info.name, &info.version)
            .map_err(|e| e.to_string())?;
    }

    Ok(Some(installed))
}

fn find_bundled_default_model(app_handle: &tauri::AppHandle) -> Option<PathBuf> {
    if let Some(path) = command_env_override("AUDRAFLOW_DEFAULT_MODEL_BIN") {
        if path.is_file() {
            return Some(path);
        }
    }

    if let Ok(resource_dir) = app_handle.path().resource_dir() {
        for candidate in [
            resource_dir.join(BUNDLED_DEFAULT_MODEL_RESOURCE),
            resource_dir
                .join("resources")
                .join(BUNDLED_DEFAULT_MODEL_RESOURCE),
        ] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    runtime_search_roots()
        .into_iter()
        .flat_map(|root| {
            [
                root.join(BUNDLED_DEFAULT_MODEL_RESOURCE),
                root.join("resources").join(BUNDLED_DEFAULT_MODEL_RESOURCE),
                root.join("release")
                    .join("default-models")
                    .join("ggml-base.bin"),
            ]
        })
        .find(|path| path.is_file())
}

fn installed_model_to_dto(
    model: audraflow_model_manager::InstalledModel,
    selected: Option<&audraflow_model_manager::InstalledModel>,
) -> ModelInfoDto {
    let is_selected = selected.is_some_and(|selected| {
        selected.info.name == model.info.name && selected.info.version == model.info.version
    });
    ModelInfoDto {
        bundled: is_bundled_default_model_info(&model.info),
        name: model.info.name,
        version: model.info.version,
        language: model.info.language,
        size_bytes: model.info.size_bytes,
        sha256: model.info.sha256,
        path: model.path.to_string_lossy().into_owned(),
        installed_at_ms: model.installed_at_ms,
        selected: is_selected,
    }
}

fn model_settings(app_handle: &tauri::AppHandle) -> Result<ModelSettingsDto, String> {
    let manager = model_manager(app_handle)?;
    ensure_bundled_default_model(app_handle, &manager)?;
    let selected = manager.selected_model().map_err(|e| e.to_string())?;
    let installed_models = manager
        .list_installed_models()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|model| installed_model_to_dto(model, selected.as_ref()))
        .collect();
    let selected_model = selected
        .clone()
        .map(|model| installed_model_to_dto(model, selected.as_ref()));

    Ok(ModelSettingsDto {
        models_dir: manager.models_dir().to_string_lossy().into_owned(),
        selected_model,
        installed_models,
    })
}

fn builtin_model_catalog(
    app_handle: &tauri::AppHandle,
) -> Result<Vec<ModelCatalogEntryDto>, String> {
    let manager = model_manager(app_handle)?;
    ensure_bundled_default_model(app_handle, &manager)?;
    let installed = manager.list_installed_models().map_err(|e| e.to_string())?;
    let selected = manager.selected_model().map_err(|e| e.to_string())?;

    let catalog = [
        (
            "tiny",
            77_691_713_u64,
            "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
            "Fast smoke tests and short drafts.",
            false,
        ),
        (
            "base",
            DEFAULT_WHISPER_MODEL_SIZE_BYTES,
            DEFAULT_WHISPER_MODEL_SHA256,
            "Balanced multilingual local transcription.",
            true,
        ),
        (
            "small",
            487_601_967_u64,
            "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
            "Higher accuracy for longer or noisier recordings.",
            false,
        ),
        (
            "large-v3-turbo-q8_0",
            874_188_075_u64,
            "317eb69c11673c9de1e1f0d459b253999804ec71ac4c23c17ecf5fbe24e259a1",
            "High-accuracy multilingual turbo model; slower and larger than small.",
            false,
        ),
    ];

    Ok(catalog
        .into_iter()
        .map(|(name, size_bytes, sha256, description, recommended)| {
            let version = whisper_cpp_model_version();
            let is_installed = installed
                .iter()
                .any(|model| model.info.name == name && model.info.version == version);
            let is_selected = selected
                .as_ref()
                .is_some_and(|model| model.info.name == name && model.info.version == version);
            ModelCatalogEntryDto {
                name: name.into(),
                version,
                language: "auto".into(),
                size_bytes,
                sha256: sha256.into(),
                download_url: format!(
                    "{WHISPER_CPP_MODEL_BASE_URL}/{WHISPER_CPP_MODEL_COMMIT}/ggml-{name}.bin"
                ),
                description: description.into(),
                recommended,
                installed: is_installed,
                selected: is_selected,
            }
        })
        .collect())
}

fn normalize_model_component(value: &str, default: &str) -> String {
    let normalized = value
        .trim()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        .collect::<String>();
    if normalized.is_empty() {
        default.into()
    } else {
        normalized
    }
}

fn default_transcription_language(model_language: &str) -> String {
    let normalized = model_language.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "" | "auto" | "multilingual" | "multi" => "auto".into(),
        _ => normalized,
    }
}

fn normalize_transcription_language(
    language: Option<&str>,
    selected_model: Option<&audraflow_model_manager::InstalledModel>,
) -> String {
    match language.unwrap_or("").trim().to_ascii_lowercase().as_str() {
        "" | "auto" | "detect" | "auto_detect" => "auto".into(),
        "zh" | "cn" | "chinese" | "mandarin" | "中文" | "汉语" | "普通话" => "zh".into(),
        "en" | "eng" | "english" | "英文" | "英语" => "en".into(),
        _ => selected_model
            .map(|model| default_transcription_language(&model.info.language))
            .unwrap_or_else(|| "auto".into()),
    }
}

fn normalize_audio_mode(audio_mode: Option<&str>) -> String {
    match audio_mode
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "music" | "lyrics" | "lyric" => "music".into(),
        _ => "speech".into(),
    }
}

fn normalize_asr_engine(asr_engine: Option<&str>) -> String {
    match asr_engine
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "" | "auto" | "automatic" => "auto".into(),
        "whisper" | "whispercpp" | "whisper.cpp" => "whisper".into(),
        "sensevoice" | "sense_voice" => "sensevoice".into(),
        "funasr" | "fun-asr" | "fun_asr" | "funasr-nano" | "fun-asr-nano" => "funasr".into(),
        _ => "auto".into(),
    }
}

fn resolve_asr_engine(
    requested_engine: &str,
    _audio_mode: &str,
    has_whisper_model: bool,
) -> String {
    match requested_engine {
        "whisper" => "whisper".into(),
        "sensevoice" => "sensevoice".into(),
        "funasr" => "funasr".into(),
        _ if has_whisper_model => "whisper".into(),
        _ => "sensevoice".into(),
    }
}

fn is_whisper_cpp_model(model: &audraflow_model_manager::InstalledModel) -> bool {
    matches!(
        &model.info.model_type,
        audraflow_model_manager::ModelType::WhisperCpp
    )
}

fn preferred_lyrics_whisper_model(
    installed_models: &[audraflow_model_manager::InstalledModel],
    selected_model: Option<&audraflow_model_manager::InstalledModel>,
    extreme_accuracy: bool,
) -> Option<audraflow_model_manager::InstalledModel> {
    if let Some(model) = selected_model.filter(|model| is_whisper_cpp_model(model)) {
        return Some(model.clone());
    }

    let preferences = if extreme_accuracy {
        [
            "large-v3-turbo",
            "large-v3",
            "large",
            "medium",
            "small",
            "base",
        ]
        .as_slice()
    } else {
        [
            "small",
            "medium",
            "large-v3-turbo",
            "large-v3",
            "large",
            "base",
        ]
        .as_slice()
    };

    preferences
        .iter()
        .find_map(|preference| find_whisper_model_by_preference(installed_models, preference))
        .or_else(|| {
            installed_models
                .iter()
                .find(|model| is_whisper_cpp_model(model))
                .cloned()
        })
}

fn find_whisper_model_by_preference(
    installed_models: &[audraflow_model_manager::InstalledModel],
    preference: &str,
) -> Option<audraflow_model_manager::InstalledModel> {
    installed_models
        .iter()
        .find(|model| {
            is_whisper_cpp_model(model)
                && whisper_model_name_matches_preference(&model.info.name, preference)
        })
        .cloned()
}

fn whisper_model_name_matches_preference(name: &str, preference: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    match preference {
        "large" => name.starts_with("large"),
        "medium" => name.starts_with("medium"),
        other => name == other || name.starts_with(&format!("{other}-")),
    }
}

fn resolve_whisper_model_for_job(
    asr_engine: &str,
    audio_mode: &str,
    selected_model: Option<audraflow_model_manager::InstalledModel>,
    installed_models: &[audraflow_model_manager::InstalledModel],
    extreme_accuracy: bool,
) -> Option<audraflow_model_manager::InstalledModel> {
    if asr_engine != "whisper" {
        return selected_model;
    }

    if audio_mode == "music" {
        return preferred_lyrics_whisper_model(
            installed_models,
            selected_model.as_ref(),
            extreme_accuracy,
        );
    }

    selected_model
}

fn normalize_vocal_separation(vocal_separation: Option<&str>, audio_mode: &str) -> String {
    if audio_mode != "music" {
        return "off".into();
    }

    match vocal_separation
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "demucs" | "vocal" | "vocals" | "on" | "true" => "demucs".into(),
        _ => "off".into(),
    }
}

fn cross_platform_file_stem(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let file_name = raw
        .rsplit(['/', '\\'])
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or(raw.as_ref());

    Path::new(file_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(file_name)
        .to_string()
}

fn infer_model_name(path: &Path, requested: Option<String>) -> String {
    requested
        .map(|value| normalize_model_component(&value, "whisper-local"))
        .unwrap_or_else(|| {
            let stem = cross_platform_file_stem(path);
            let stripped = stem.strip_prefix("ggml-").unwrap_or(&stem);
            normalize_model_component(stripped, "whisper-local")
        })
}

fn infer_model_version(requested: Option<String>) -> String {
    requested
        .map(|value| normalize_model_component(&value, "local"))
        .unwrap_or_else(|| "local".into())
}

fn infer_model_name_from_url(url: &str, requested: Option<String>) -> String {
    requested
        .map(|value| normalize_model_component(&value, "whisper-remote"))
        .unwrap_or_else(|| {
            let stem = reqwest::Url::parse(url)
                .ok()
                .and_then(|parsed| {
                    parsed
                        .path_segments()
                        .and_then(|mut segments| segments.next_back().map(str::to_string))
                })
                .and_then(|filename| {
                    Path::new(&filename)
                        .file_stem()
                        .and_then(|value| value.to_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "whisper-remote".into());
            let stripped = stem.strip_prefix("ggml-").unwrap_or(&stem);
            normalize_model_component(stripped, "whisper-remote")
        })
}

fn import_local_model(
    app_handle: &tauri::AppHandle,
    request: ImportLocalModelRequest,
) -> Result<ModelSettingsDto, String> {
    let source = PathBuf::from(request.file_path.trim());
    if !source.is_file() {
        return Err(format!("Model file not found: {}", source.display()));
    }
    let size_bytes = std::fs::metadata(&source)
        .map_err(|e| format!("Failed to inspect model file: {e}"))?
        .len();
    if size_bytes == 0 {
        return Err("Model file is empty".into());
    }

    let manager = model_manager(app_handle)?;
    let sha256 = hash_file_sha256(&source.to_string_lossy())?;
    let info = audraflow_model_manager::ModelInfo {
        name: infer_model_name(&source, request.name),
        version: infer_model_version(request.version),
        language: request
            .language
            .map(|value| normalize_model_component(&value, "zh"))
            .unwrap_or_else(|| "zh".into()),
        size_bytes,
        sha256,
        download_url: "local".into(),
        model_type: audraflow_model_manager::ModelType::WhisperCpp,
    };
    let dest = manager.model_path(&info);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::copy(&source, &dest).map_err(|e| format!("Failed to import model file: {e}"))?;
    manager
        .register_installed_model(&info)
        .map_err(|e| e.to_string())?;
    manager
        .select_model(&info.name, &info.version)
        .map_err(|e| e.to_string())?;

    model_settings(app_handle)
}

async fn download_model(
    app_handle: tauri::AppHandle,
    request: DownloadModelRequest,
) -> Result<ModelActionResult, String> {
    let url = request.url.trim().to_string();
    let parsed = reqwest::Url::parse(&url).map_err(|e| format!("Invalid model URL: {e}"))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err("Only http and https model URLs are supported.".into());
    }
    let sha256 = request.sha256.trim().to_ascii_lowercase();
    if sha256.len() != 64 || !sha256.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err("Enter a 64 character SHA256 checksum for the model file.".into());
    }
    if request.size_bytes == 0 {
        return Err("Enter the expected model size in bytes.".into());
    }

    let info = audraflow_model_manager::ModelInfo {
        name: infer_model_name_from_url(&url, request.name),
        version: infer_model_version(request.version),
        language: request
            .language
            .map(|value| normalize_model_component(&value, "zh"))
            .unwrap_or_else(|| "zh".into()),
        size_bytes: request.size_bytes,
        sha256,
        download_url: url,
        model_type: audraflow_model_manager::ModelType::WhisperCpp,
    };
    let download_id = format!("{}:{}", info.name, info.version);
    emit_model_download_progress(
        &app_handle,
        &download_id,
        0,
        info.size_bytes,
        "Starting model download",
    );

    let manager = model_manager(&app_handle)?;
    let app_for_progress = app_handle.clone();
    let info_for_download = info.clone();
    tokio::task::spawn_blocking(move || {
        manager.download(&info_for_download, |downloaded, total| {
            emit_model_download_progress(
                &app_for_progress,
                &download_id,
                downloaded,
                total,
                format!(
                    "Downloaded {} / {}",
                    format_file_size(downloaded),
                    format_file_size(total)
                ),
            );
        })?;
        manager.select_model(&info_for_download.name, &info_for_download.version)?;
        anyhow::Ok(())
    })
    .await
    .map_err(|e| format!("Model download task failed: {e}"))?
    .map_err(|e| e.to_string())?;

    Ok(ModelActionResult {
        message: format!("Downloaded and selected {} v{}.", info.name, info.version),
        bytes_freed: 0,
        items_affected: 1,
        settings: model_settings(&app_handle)?,
    })
}

fn file_size_or_zero(path: &Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

fn format_file_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let value = bytes as f64;
    if value >= GIB {
        format!("{:.2} GB", value / GIB)
    } else if value >= MIB {
        format!("{:.1} MB", value / MIB)
    } else if value >= KIB {
        format!("{:.1} KB", value / KIB)
    } else {
        format!("{bytes} B")
    }
}

fn detect_nvidia_device() -> Option<(String, f64, String, String)> {
    let output = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,memory.total,cuda_version,driver_version",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let line = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?
        .to_string();
    let parts = line
        .split(',')
        .map(str::trim)
        .map(str::to_string)
        .collect::<Vec<_>>();
    if parts.len() < 4 {
        return None;
    }
    let vram_gb = parts[1].parse::<f64>().ok()? / 1024.0;
    Some((
        parts[0].clone(),
        vram_gb,
        parts[2].clone(),
        parts[3].clone(),
    ))
}

fn detect_device_diagnostics() -> DeviceDiagnosticsDto {
    let cpu_cores = num_cpus::get() as u32;
    let nvidia = detect_nvidia_device();
    let cuda_available = nvidia.is_some();
    let vram_gb = nvidia.as_ref().map(|(_, vram, _, _)| *vram);
    let device_tier = DeviceTier::classify(cuda_available, vram_gb, cpu_cores);
    let fallback_message = if cuda_available {
        None
    } else {
        Some("CUDA GPU was not detected; transcription will use CPU fallback.".into())
    };

    DeviceDiagnosticsDto {
        cpu_cores,
        cuda_available,
        vram_gb,
        gpu_model: nvidia.as_ref().map(|(name, _, _, _)| name.clone()),
        cuda_version: nvidia.as_ref().map(|(_, _, cuda, _)| cuda.clone()),
        driver_version: nvidia.as_ref().map(|(_, _, _, driver)| driver.clone()),
        device_tier: format!("{:?}", device_tier),
        fallback_message,
    }
}

async fn runtime_health(app_handle: &tauri::AppHandle) -> RuntimeHealthDto {
    let mut items = vec![
        probe_default_whisper_model(app_handle),
        probe_runtime_command(
            "whisperCli",
            "required",
            whisper_cli_command(),
            &["--help"],
            None,
            5,
        )
        .await,
        probe_runtime_command(
            "ffmpeg",
            "required",
            ffmpeg_command(),
            &["-version"],
            None,
            5,
        )
        .await,
        probe_runtime_command(
            "ffprobe",
            "required",
            ffprobe_command(),
            &["-version"],
            None,
            5,
        )
        .await,
        probe_sensevoice_python().await,
        probe_runtime_command(
            "ytDlp",
            "optional",
            yt_dlp_command(),
            &["--version"],
            None,
            5,
        )
        .await,
        probe_demucs().await,
        probe_runtime_command(
            "funasrCli",
            "experimental",
            funasr_cli_command(),
            &["--help"],
            None,
            5,
        )
        .await,
        probe_funasr_models(app_handle),
    ];

    items.sort_by_key(|item| runtime_dependency_sort_key(&item.id));

    let blocking_count = items
        .iter()
        .filter(|item| item.kind == "required" && item.status != "ready")
        .count() as u32;
    let warning_count = items
        .iter()
        .filter(|item| item.kind != "required" && item.status != "ready")
        .count() as u32;

    RuntimeHealthDto {
        generated_at_ms: now_unix_ms(),
        blocking_count,
        warning_count,
        items,
    }
}

fn runtime_dependency_sort_key(id: &str) -> u8 {
    match id {
        "defaultWhisperModel" => 0,
        "whisperCli" => 1,
        "ffmpeg" => 2,
        "ffprobe" => 3,
        "sensevoicePython" => 4,
        "ytDlp" => 5,
        "demucs" => 6,
        "funasrCli" => 7,
        "funasrModels" => 8,
        _ => u8::MAX,
    }
}

fn runtime_dependency_repairable(id: &str) -> bool {
    matches!(
        id,
        "defaultWhisperModel" | "ytDlp" | "sensevoicePython" | "demucs"
    )
}

fn probe_default_whisper_model(app_handle: &tauri::AppHandle) -> RuntimeDependencyDto {
    let id = "defaultWhisperModel";
    let repairable = runtime_dependency_repairable(id);
    let manager = match model_manager(app_handle) {
        Ok(manager) => manager,
        Err(error) => {
            return RuntimeDependencyDto {
                id: id.into(),
                status: "missing".into(),
                kind: "required".into(),
                path: None,
                version: None,
                detail: Some(error),
                repairable,
            };
        }
    };

    if let Err(error) = ensure_bundled_default_model(app_handle, &manager) {
        return RuntimeDependencyDto {
            id: id.into(),
            status: "warning".into(),
            kind: "required".into(),
            path: None,
            version: None,
            detail: Some(error),
            repairable,
        };
    }

    match manager.selected_model() {
        Ok(Some(model)) if is_whisper_cpp_model(&model) && model.path.is_file() => {
            RuntimeDependencyDto {
                id: id.into(),
                status: "ready".into(),
                kind: "required".into(),
                path: Some(model.path.to_string_lossy().into_owned()),
                version: Some(format!("{} {}", model.info.name, model.info.version)),
                detail: None,
                repairable,
            }
        }
        Ok(_) => RuntimeDependencyDto {
            id: id.into(),
            status: "missing".into(),
            kind: "required".into(),
            path: None,
            version: None,
            detail: Some("No selected local Whisper model was found.".into()),
            repairable,
        },
        Err(error) => RuntimeDependencyDto {
            id: id.into(),
            status: "warning".into(),
            kind: "required".into(),
            path: None,
            version: None,
            detail: Some(error.to_string()),
            repairable,
        },
    }
}

async fn probe_runtime_command(
    id: &str,
    kind: &str,
    program: PathBuf,
    args: &[&str],
    display_path: Option<String>,
    timeout_secs: u64,
) -> RuntimeDependencyDto {
    let display_path = display_path.unwrap_or_else(|| command_display_path(&program));
    let mut command = tokio::process::Command::new(&program);
    command.args(args);

    match tokio::time::timeout(Duration::from_secs(timeout_secs), command.output()).await {
        Ok(Ok(output)) if output.status.success() => RuntimeDependencyDto {
            id: id.into(),
            status: "ready".into(),
            kind: kind.into(),
            path: Some(display_path),
            version: first_output_line(&output.stdout)
                .or_else(|| first_output_line(&output.stderr)),
            detail: None,
            repairable: runtime_dependency_repairable(id),
        },
        Ok(Ok(output)) => RuntimeDependencyDto {
            id: id.into(),
            status: "warning".into(),
            kind: kind.into(),
            path: Some(display_path),
            version: first_output_line(&output.stdout)
                .or_else(|| first_output_line(&output.stderr)),
            detail: Some(format!(
                "Probe exited with {}. {}",
                output.status,
                short_output(&output.stderr)
                    .or_else(|| short_output(&output.stdout))
                    .unwrap_or_else(|| "No output.".into())
            )),
            repairable: runtime_dependency_repairable(id),
        },
        Ok(Err(error)) => RuntimeDependencyDto {
            id: id.into(),
            status: "missing".into(),
            kind: kind.into(),
            path: None,
            version: None,
            detail: Some(error.to_string()),
            repairable: runtime_dependency_repairable(id),
        },
        Err(_) => RuntimeDependencyDto {
            id: id.into(),
            status: "warning".into(),
            kind: kind.into(),
            path: Some(display_path),
            version: None,
            detail: Some(format!("Probe timed out after {timeout_secs}s.")),
            repairable: runtime_dependency_repairable(id),
        },
    }
}

async fn probe_sensevoice_python() -> RuntimeDependencyDto {
    let Some(invocation) = resolve_python_invocation() else {
        return RuntimeDependencyDto {
            id: "sensevoicePython".into(),
            status: "missing".into(),
            kind: "optional".into(),
            path: None,
            version: None,
            detail: Some("Python was not found.".into()),
            repairable: runtime_dependency_repairable("sensevoicePython"),
        };
    };

    let script = "import funasr, modelscope; print('funasr/modelscope ready')";
    let mut args = invocation.base_args;
    args.push("-c".into());
    args.push(script.into());
    let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();

    probe_runtime_command(
        "sensevoicePython",
        "optional",
        invocation.program,
        &arg_refs,
        Some(invocation.display),
        15,
    )
    .await
}

async fn probe_demucs() -> RuntimeDependencyDto {
    let Some(invocation) = resolve_demucs_invocation_for_health() else {
        return RuntimeDependencyDto {
            id: "demucs".into(),
            status: "missing".into(),
            kind: "optional".into(),
            path: None,
            version: None,
            detail: Some("Demucs was not found.".into()),
            repairable: runtime_dependency_repairable("demucs"),
        };
    };

    let mut args = invocation.base_args;
    args.push("--help".into());
    let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();

    probe_runtime_command(
        "demucs",
        "optional",
        invocation.program,
        &arg_refs,
        Some(invocation.display),
        8,
    )
    .await
}

fn probe_funasr_models(app_handle: &tauri::AppHandle) -> RuntimeDependencyDto {
    match resolve_funasr_model_paths(app_handle) {
        Ok(paths) => RuntimeDependencyDto {
            id: "funasrModels".into(),
            status: "ready".into(),
            kind: "experimental".into(),
            path: paths
                .model_dir
                .map(|path| path.to_string_lossy().into_owned()),
            version: None,
            detail: Some(format!(
                "encoder={}, llm={}, vad={}",
                paths.encoder_path.display(),
                paths.llm_path.display(),
                paths
                    .vad_path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "not found".into())
            )),
            repairable: runtime_dependency_repairable("funasrModels"),
        },
        Err(error) => RuntimeDependencyDto {
            id: "funasrModels".into(),
            status: "missing".into(),
            kind: "experimental".into(),
            path: None,
            version: None,
            detail: Some(error),
            repairable: runtime_dependency_repairable("funasrModels"),
        },
    }
}

async fn repair_runtime_dependency(
    app_handle: tauri::AppHandle,
    id: &str,
) -> Result<RuntimeRepairResultDto, String> {
    let normalized = id.trim();
    let message = match normalized {
        "defaultWhisperModel" => repair_default_whisper_model(&app_handle).await?,
        "ytDlp" => repair_yt_dlp().await?,
        "sensevoicePython" => {
            repair_python_packages(
                "SenseVoice Python packages",
                &["funasr", "modelscope"],
                Duration::from_secs(30 * 60),
            )
            .await?
        }
        "demucs" => {
            repair_python_packages(
                "Demucs",
                &["demucs", "torchcodec"],
                Duration::from_secs(30 * 60),
            )
            .await?
        }
        _ => {
            return Err(format!(
                "Runtime dependency cannot be repaired automatically: {id}"
            ))
        }
    };

    Ok(RuntimeRepairResultDto {
        id: normalized.into(),
        message,
        health: runtime_health(&app_handle).await,
    })
}

async fn repair_default_whisper_model(app_handle: &tauri::AppHandle) -> Result<String, String> {
    let manager = model_manager(app_handle)?;
    if let Some(installed) = ensure_bundled_default_model(app_handle, &manager)? {
        match manager.selected_model() {
            Ok(Some(model)) if is_whisper_cpp_model(&model) && model.path.is_file() => {
                return Ok(format!("Whisper model is ready: {}", model.path.display()));
            }
            Ok(_) => {
                manager
                    .select_model(&installed.info.name, &installed.info.version)
                    .map_err(|e| e.to_string())?;
                return Ok(format!(
                    "Default Whisper model was selected: {}",
                    installed.path.display()
                ));
            }
            Err(error) => {
                manager
                    .select_model(&installed.info.name, &installed.info.version)
                    .map_err(|e| e.to_string())?;
                return Ok(format!(
                    "Default Whisper model was selected after repairing the previous selection ({error}): {}",
                    installed.path.display(),
                ));
            }
        }
    }

    let mut info = bundled_default_model_info();
    info.download_url = format!(
        "{WHISPER_CPP_MODEL_BASE_URL}/{WHISPER_CPP_MODEL_COMMIT}/ggml-{DEFAULT_WHISPER_MODEL_NAME}.bin"
    );
    let download_id = "defaultWhisperModel";
    emit_model_download_progress(
        app_handle,
        download_id,
        0,
        info.size_bytes,
        "Downloading default Whisper model",
    );

    let app_for_progress = app_handle.clone();
    let manager_for_download = manager;
    let info_for_download = info.clone();
    tokio::task::spawn_blocking(move || {
        manager_for_download.download(&info_for_download, |downloaded, total| {
            emit_model_download_progress(
                &app_for_progress,
                download_id,
                downloaded,
                total,
                format!(
                    "Downloaded {} / {}",
                    format_file_size(downloaded),
                    format_file_size(total)
                ),
            );
        })?;
        manager_for_download.select_model(&info_for_download.name, &info_for_download.version)?;
        anyhow::Ok(())
    })
    .await
    .map_err(|e| format!("Default model repair task failed: {e}"))?
    .map_err(|e| e.to_string())?;

    Ok("Default Whisper model was downloaded and selected.".into())
}

async fn repair_yt_dlp() -> Result<String, String> {
    let destination = managed_tool_path(yt_dlp_binary_name());
    download_binary_to_path(yt_dlp_download_url(), &destination, 1024 * 1024).await?;
    mark_executable(&destination)?;
    let output = tokio::process::Command::new(&destination)
        .arg("--version")
        .output()
        .await
        .map_err(|e| format!("Downloaded yt-dlp could not be started: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "Downloaded yt-dlp failed its version check: {}",
            short_output(&output.stderr)
                .or_else(|| short_output(&output.stdout))
                .unwrap_or_else(|| "No output.".into())
        ));
    }

    Ok(format!("yt-dlp is ready: {}", destination.display()))
}

async fn download_binary_to_path(
    url: &str,
    destination: &Path,
    min_bytes: usize,
) -> Result<(), String> {
    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create download directory: {e}"))?;
    }

    let response = reqwest::get(url)
        .await
        .map_err(|e| format!("Failed to download {url}: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("Download failed with {}", response.status()));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("Failed to read download response: {e}"))?;
    if bytes.len() < min_bytes {
        return Err(format!(
            "Downloaded file is too small: {} bytes",
            bytes.len()
        ));
    }
    if looks_like_html(&bytes) {
        return Err("Downloaded HTML instead of a binary file.".into());
    }

    let tmp_path = destination.with_extension("tmp");
    let _ = tokio::fs::remove_file(&tmp_path).await;
    tokio::fs::write(&tmp_path, &bytes)
        .await
        .map_err(|e| format!("Failed to write downloaded file: {e}"))?;
    let _ = tokio::fs::remove_file(destination).await;
    tokio::fs::rename(&tmp_path, destination)
        .await
        .map_err(|e| format!("Failed to install downloaded file: {e}"))
}

fn looks_like_html(bytes: &[u8]) -> bool {
    let sample_len = bytes.len().min(512);
    let sample = String::from_utf8_lossy(&bytes[..sample_len])
        .trim_start()
        .to_ascii_lowercase();
    sample.starts_with("<!doctype html") || sample.starts_with("<html")
}

fn mark_executable(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path)
            .map_err(|e| format!("Failed to inspect downloaded executable: {e}"))?
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions)
            .map_err(|e| format!("Failed to mark downloaded executable: {e}"))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

async fn repair_python_packages(
    label: &str,
    packages: &[&str],
    timeout: Duration,
) -> Result<String, String> {
    let invocation = resolve_python_invocation().ok_or_else(|| {
        format!(
            "{label} repair requires Python 3. Install Python 3 first or set AUDRAFLOW_PYTHON_BIN."
        )
    })?;
    let mut args = invocation.base_args;
    args.extend([
        "-m".into(),
        "pip".into(),
        "install".into(),
        "--user".into(),
        "-U".into(),
    ]);
    args.extend(packages.iter().map(|package| (*package).to_string()));

    let output = tokio::time::timeout(
        timeout,
        tokio::process::Command::new(&invocation.program)
            .args(&args)
            .output(),
    )
    .await
    .map_err(|_| {
        format!(
            "{label} repair timed out after {} seconds.",
            timeout.as_secs()
        )
    })?
    .map_err(|e| {
        format!(
            "Failed to start Python package repair at {}: {e}",
            invocation.display
        )
    })?;

    if !output.status.success() {
        return Err(format!(
            "{label} repair failed: {}",
            short_output(&output.stderr)
                .or_else(|| short_output(&output.stdout))
                .unwrap_or_else(|| "No output.".into())
        ));
    }

    Ok(format!("{label} installed or updated."))
}

fn preflight_url_import_dependencies(url: &str, skip_start_seconds: f64) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("Invalid URL: {e}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => return Err("Only http and https links are supported".into()),
    }

    if skip_start_seconds > 0.0 {
        ensure_runtime_command_available(
            ffmpeg_command(),
            "FFmpeg",
            "FFmpeg is required when skipping the start of a direct media link. Reinstall AudraFlow or set AUDRAFLOW_FFMPEG_BIN.",
        )?;
    }

    if is_probable_platform_url(&parsed) {
        ensure_runtime_command_available(
            yt_dlp_command(),
            "yt-dlp",
            "This looks like a platform link. Reinstall AudraFlow or set AUDRAFLOW_YT_DLP_BIN before importing it.",
        )?;
    }

    Ok(())
}

fn preflight_transcription_dependencies(
    app_handle: &tauri::AppHandle,
    asr_engine: &str,
) -> Result<(), String> {
    ensure_runtime_command_available(
        ffmpeg_command(),
        "FFmpeg",
        "FFmpeg is required for local media decoding. Reinstall AudraFlow or set AUDRAFLOW_FFMPEG_BIN.",
    )?;
    ensure_runtime_command_available(
        ffprobe_command(),
        "FFprobe",
        "FFprobe is required for media metadata detection. Reinstall AudraFlow or set AUDRAFLOW_FFPROBE_BIN.",
    )?;

    match asr_engine {
        "whisper" => ensure_runtime_command_startable(
            whisper_cli_command(),
            &["--help"],
            Duration::from_secs(8),
            "Whisper CLI",
            "Whisper transcription requires a runnable whisper-cli plus its runtime DLLs. Reinstall AudraFlow or set AUDRAFLOW_WHISPER_CLI.",
        ),
        "sensevoice" => ensure_sensevoice_python_available(),
        "funasr" => {
            ensure_runtime_command_available(
                funasr_cli_command(),
                "Fun-ASR CLI",
                "Fun-ASR transcription requires llama-funasr-cli. Install it or set AUDRAFLOW_FUNASR_CLI.",
            )?;
            resolve_funasr_model_paths(app_handle).map_err(|error| {
                format!(
                    "Fun-ASR model files are required before using the Fun-ASR engine: {error}. Place GGUF files under the Fun-ASR model directory or set AUDRAFLOW_FUNASR_MODEL_DIR."
                )
            })?;
            Ok(())
        }
        _ => Ok(()),
    }
}

fn preflight_requested_transcription_dependencies(
    app_handle: &tauri::AppHandle,
    asr_engine: Option<&str>,
    audio_mode: Option<&str>,
    extreme_accuracy: bool,
) -> Result<(), String> {
    let requested_asr_engine = normalize_asr_engine(asr_engine);
    let audio_mode = normalize_audio_mode(audio_mode);
    let manager = model_manager(app_handle)?;
    ensure_bundled_default_model(app_handle, &manager)?;
    let selected_model = manager.selected_model().map_err(|e| e.to_string())?;
    let installed_models = manager.list_installed_models().map_err(|e| e.to_string())?;
    let has_whisper_model = preferred_lyrics_whisper_model(
        &installed_models,
        selected_model.as_ref(),
        extreme_accuracy,
    )
    .is_some();
    let resolved_engine = resolve_asr_engine(&requested_asr_engine, &audio_mode, has_whisper_model);
    let selected_model = if resolved_engine == "funasr" {
        None
    } else {
        resolve_whisper_model_for_job(
            &resolved_engine,
            &audio_mode,
            selected_model,
            &installed_models,
            extreme_accuracy,
        )
    };

    if resolved_engine == "whisper" {
        let selected_model = selected_model.ok_or_else(|| {
            "No ASR model is selected. Import and select a ggml model in Settings before starting Whisper transcription.".to_string()
        })?;
        if !selected_model.path.is_file() {
            return Err(format!(
                "Selected ASR model file is missing: {}. Re-import or select another model in Settings.",
                selected_model.path.display()
            ));
        }
    }

    preflight_transcription_dependencies(app_handle, &resolved_engine)
}

fn ensure_runtime_command_available(
    command: PathBuf,
    label: &str,
    recovery_hint: &str,
) -> Result<(), String> {
    if is_runtime_command_available(&command) {
        Ok(())
    } else {
        Err(format!(
            "{label} was not found. {recovery_hint} Checked: {}",
            command.display()
        ))
    }
}

fn ensure_runtime_command_startable(
    command: PathBuf,
    args: &[&str],
    timeout: Duration,
    label: &str,
    recovery_hint: &str,
) -> Result<(), String> {
    ensure_runtime_command_available(command.clone(), label, recovery_hint)?;
    let args = args
        .iter()
        .map(|value| (*value).to_string())
        .collect::<Vec<_>>();
    match run_runtime_probe_with_timeout(&command, &args, timeout) {
        Ok(_) => Ok(()),
        Err(error) => Err(format!(
            "{label} could not be started. {recovery_hint} Checked: {}. Error: {error}",
            command.display()
        )),
    }
}

fn ensure_sensevoice_python_available() -> Result<(), String> {
    let invocation = resolve_python_invocation().ok_or_else(|| {
        "SenseVoice requires Python 3. Install Python 3 or set AUDRAFLOW_PYTHON_BIN.".to_string()
    })?;
    let script = r#"import importlib.util, sys
missing = [name for name in ("funasr", "modelscope") if importlib.util.find_spec(name) is None]
if missing:
    print("missing Python package(s): " + ", ".join(missing), file=sys.stderr)
    sys.exit(1)
print("sensevoice dependencies ready")
"#;
    let mut args = invocation.base_args;
    args.push("-c".into());
    args.push(script.into());
    let output =
        run_runtime_probe_with_timeout(&invocation.program, &args, Duration::from_secs(8))
            .map_err(|error| {
                format!(
                    "SenseVoice Python dependency check failed at {}: {error}. Install with: python3 -m pip install --user -U funasr modelscope",
                    invocation.display
                )
            })?;
    if output.status.success() {
        return Ok(());
    }

    Err(format!(
        "SenseVoice requires Python packages funasr and modelscope. {} Install with: python3 -m pip install --user -U funasr modelscope",
        short_output(&output.stderr)
            .or_else(|| short_output(&output.stdout))
            .unwrap_or_else(|| "Dependency check failed.".into())
    ))
}

fn run_runtime_probe_with_timeout(
    program: &Path,
    args: &[String],
    timeout: Duration,
) -> Result<std::process::Output, String> {
    let mut command = std::process::Command::new(program);
    if let Some(parent) = program.parent().filter(|path| !path.as_os_str().is_empty()) {
        command.current_dir(parent);
    }
    let mut child = command
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let start = Instant::now();
    loop {
        match child.try_wait().map_err(|error| error.to_string())? {
            Some(_) => return child.wait_with_output().map_err(|error| error.to_string()),
            None if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("probe timed out after {}s", timeout.as_secs()));
            }
            None => std::thread::sleep(Duration::from_millis(40)),
        }
    }
}

fn is_runtime_command_available(command: &Path) -> bool {
    if command.is_file() {
        return true;
    }
    if command.is_absolute() || command.components().count() > 1 {
        return false;
    }
    command
        .file_name()
        .and_then(|value| value.to_str())
        .and_then(find_system_command)
        .is_some()
}

fn is_probable_platform_url(parsed: &reqwest::Url) -> bool {
    let Some(host) = parsed.host_str().map(|host| host.to_ascii_lowercase()) else {
        return false;
    };
    let known_platform = [
        "youtube.com",
        "youtu.be",
        "music.youtube.com",
        "bilibili.com",
        "vimeo.com",
        "soundcloud.com",
        "x.com",
        "twitter.com",
        "facebook.com",
        "instagram.com",
        "tiktok.com",
        "douyin.com",
    ]
    .iter()
    .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")));

    known_platform && !is_probable_direct_media_url(parsed)
}

fn is_probable_direct_media_url(parsed: &reqwest::Url) -> bool {
    parsed
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .and_then(|name| name.rsplit_once('.').map(|(_, ext)| ext))
        .is_some_and(supported_media_extension)
}

#[derive(Debug)]
struct RuntimeInvocation {
    program: PathBuf,
    base_args: Vec<String>,
    display: String,
}

#[derive(Debug)]
struct FunAsrModelHealthPaths {
    model_dir: Option<PathBuf>,
    encoder_path: PathBuf,
    llm_path: PathBuf,
    vad_path: Option<PathBuf>,
}

fn resolve_python_invocation() -> Option<RuntimeInvocation> {
    if let Some(path) = command_env_override("AUDRAFLOW_PYTHON_BIN")
        .or_else(|| command_env_override("FT_PYTHON_BIN"))
    {
        return Some(RuntimeInvocation {
            display: path.to_string_lossy().into_owned(),
            program: path,
            base_args: vec![],
        });
    }

    ["python3", "python", "py"].iter().find_map(|name| {
        find_system_command(name).map(|program| {
            let mut base_args = Vec::new();
            if is_py_launcher(&program) {
                base_args.push("-3".into());
            }
            RuntimeInvocation {
                display: command_display_path(&program),
                program,
                base_args,
            }
        })
    })
}

fn resolve_demucs_invocation_for_health() -> Option<RuntimeInvocation> {
    if let Some(path) = command_env_override("AUDRAFLOW_DEMUCS_BIN")
        .or_else(|| command_env_override("FT_DEMUCS_BIN"))
    {
        return Some(RuntimeInvocation {
            display: path.to_string_lossy().into_owned(),
            program: path,
            base_args: vec![],
        });
    }

    if let Some(program) = find_system_command("demucs") {
        return Some(RuntimeInvocation {
            display: command_display_path(&program),
            program,
            base_args: vec![],
        });
    }

    for python in ["python3", "python", "py"] {
        if let Some(program) = find_system_command(python) {
            let mut base_args = Vec::new();
            let display = if is_py_launcher(&program) {
                base_args.push("-3".into());
                format!("{} -3 -m demucs", command_display_path(&program))
            } else {
                format!("{} -m demucs", command_display_path(&program))
            };
            base_args.push("-m".into());
            base_args.push("demucs".into());
            return Some(RuntimeInvocation {
                program,
                base_args,
                display,
            });
        }
    }

    None
}

fn is_py_launcher(program: &Path) -> bool {
    program
        .file_stem()
        .and_then(|value| value.to_str())
        .is_some_and(|stem| stem.eq_ignore_ascii_case("py"))
}

fn resolve_funasr_model_paths(
    app_handle: &tauri::AppHandle,
) -> Result<FunAsrModelHealthPaths, String> {
    if let (Some(encoder_path), Some(llm_path)) = (
        command_env_override("AUDRAFLOW_FUNASR_ENCODER")
            .or_else(|| command_env_override("FT_FUNASR_ENCODER")),
        command_env_override("AUDRAFLOW_FUNASR_LLM")
            .or_else(|| command_env_override("FT_FUNASR_LLM")),
    ) {
        ensure_runtime_file(&encoder_path, "Fun-ASR encoder")?;
        ensure_runtime_file(&llm_path, "Fun-ASR decoder")?;
        return Ok(FunAsrModelHealthPaths {
            model_dir: encoder_path.parent().map(Path::to_path_buf),
            encoder_path,
            llm_path,
            vad_path: command_env_override("AUDRAFLOW_FUNASR_VAD")
                .or_else(|| command_env_override("FT_FUNASR_VAD"))
                .filter(|path| path.is_file()),
        });
    }

    let model_dirs = funasr_model_dirs_for_health(app_handle);
    let encoder_path = find_first_named_file(&model_dirs, &["funasr-encoder-f16.gguf"])
        .ok_or_else(|| "Fun-ASR encoder model not found.".to_string())?;
    let llm_path = find_first_named_file(
        &model_dirs,
        &[
            "qwen3-0.6b-q5km.gguf",
            "qwen3-0.6b-q8_0.gguf",
            "qwen3-0.6b-q4km.gguf",
        ],
    )
    .ok_or_else(|| "Fun-ASR decoder model not found.".to_string())?;
    let vad_path = find_first_named_file(&model_dirs, &["fsmn-vad.gguf"]);

    Ok(FunAsrModelHealthPaths {
        model_dir: encoder_path.parent().map(Path::to_path_buf),
        encoder_path,
        llm_path,
        vad_path,
    })
}

fn funasr_model_dirs_for_health(app_handle: &tauri::AppHandle) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(path) = command_env_override("AUDRAFLOW_FUNASR_MODEL_DIR")
        .or_else(|| command_env_override("FT_FUNASR_MODEL_DIR"))
    {
        dirs.push(path);
    }
    if let Ok(app_data) = app_handle.path().app_data_dir() {
        dirs.push(app_data.join("models").join("funasr-nano"));
    }
    for root in runtime_search_roots() {
        dirs.push(root.join("funasr-gguf"));
        dirs.push(root.join("gguf"));
        dirs.push(root.join("models").join("funasr-nano"));
        dirs.push(root.join("external").join("funasr-llamacpp").join("gguf"));
    }
    dedupe_path_list(dirs)
}

fn find_first_named_file(dirs: &[PathBuf], names: &[&str]) -> Option<PathBuf> {
    dirs.iter()
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|path| path.is_file())
}

fn ensure_runtime_file(path: &Path, label: &str) -> Result<(), String> {
    if path.is_file() {
        Ok(())
    } else {
        Err(format!("{label} file not found: {}", path.display()))
    }
}

fn whisper_cli_command() -> PathBuf {
    command_env_override("AUDRAFLOW_WHISPER_CLI")
        .or_else(|| command_env_override("FT_WHISPER_CLI"))
        .or_else(|| find_bundled_command(whisper_cli_binary_name()))
        .or_else(|| find_dev_or_portable_tool(whisper_cli_binary_name()))
        .or_else(|| find_system_command(whisper_cli_binary_name()))
        .unwrap_or_else(|| PathBuf::from(whisper_cli_binary_name()))
}

fn funasr_cli_command() -> PathBuf {
    command_env_override("AUDRAFLOW_FUNASR_CLI")
        .or_else(|| command_env_override("FT_FUNASR_CLI"))
        .or_else(|| find_bundled_command(funasr_cli_binary_name()))
        .or_else(|| find_dev_or_portable_tool(funasr_cli_binary_name()))
        .or_else(|| find_system_command(funasr_cli_binary_name()))
        .unwrap_or_else(|| PathBuf::from(funasr_cli_binary_name()))
}

fn whisper_cli_binary_name() -> &'static str {
    if cfg!(windows) {
        "whisper-cli.exe"
    } else {
        "whisper-cli"
    }
}

fn funasr_cli_binary_name() -> &'static str {
    if cfg!(windows) {
        "llama-funasr-cli.exe"
    } else {
        "llama-funasr-cli"
    }
}

fn tool_binary_name(name: &'static str) -> &'static str {
    match (cfg!(windows), name) {
        (true, "ffmpeg") => "ffmpeg.exe",
        (true, "ffprobe") => "ffprobe.exe",
        _ => name,
    }
}

fn find_dev_or_portable_tool(name: &str) -> Option<PathBuf> {
    for root in runtime_search_roots() {
        for candidate in [
            root.join(name),
            root.join("bin").join(name),
            root.join("release")
                .join("linux-portable")
                .join("AudraFlow")
                .join("bin")
                .join(name),
            root.join("release")
                .join("windows-portable")
                .join("AudraFlow")
                .join("bin")
                .join(name),
            root.join("external")
                .join("whisper.cpp")
                .join("build-linux")
                .join("bin")
                .join(name),
            root.join("external")
                .join("whisper.cpp")
                .join("build")
                .join("bin")
                .join(name),
            root.join("external")
                .join("Fun-ASR")
                .join("runtime")
                .join("llama.cpp")
                .join("build")
                .join("bin")
                .join(name),
            root.join("external")
                .join("funasr-llamacpp")
                .join("bin")
                .join(name),
        ] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        if let Some(path) = find_staged_binary(&root, name) {
            return Some(path);
        }
    }
    None
}

fn find_staged_binary(root: &Path, name: &str) -> Option<PathBuf> {
    let stem = name.strip_suffix(".exe").unwrap_or(name);
    for dir in [
        root.join("src-tauri").join("binaries"),
        root.join("binaries"),
    ] {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
                    continue;
                };
                if file_name == name || file_name.starts_with(&format!("{stem}-")) {
                    return Some(path);
                }
            }
        }
    }
    None
}

fn runtime_search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(resource_dir) = std::env::var_os("AUDRAFLOW_RESOURCE_DIR") {
        roots.push(PathBuf::from(resource_dir));
    }
    if let Ok(exe) = std::env::current_exe() {
        roots.extend(exe.ancestors().map(Path::to_path_buf));
    }
    if let Ok(cwd) = std::env::current_dir() {
        roots.extend(cwd.ancestors().map(Path::to_path_buf));
    }
    dedupe_path_list(roots)
}

fn dedupe_path_list(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut deduped = Vec::new();
    for path in paths {
        if !deduped.contains(&path) {
            deduped.push(path);
        }
    }
    deduped
}

fn command_display_path(path: &Path) -> String {
    if path.is_absolute() || path.components().count() > 1 {
        return path.to_string_lossy().into_owned();
    }
    path.file_name()
        .and_then(|value| value.to_str())
        .and_then(find_system_command)
        .unwrap_or_else(|| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn first_output_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(truncate_runtime_text)
}

fn short_output(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(truncate_runtime_text(trimmed))
    }
}

fn truncate_runtime_text(text: &str) -> String {
    const MAX_CHARS: usize = 180;
    if text.chars().count() <= MAX_CHARS {
        text.to_string()
    } else {
        format!("{}...", text.chars().take(MAX_CHARS).collect::<String>())
    }
}

fn directory_size_bytes(path: &Path) -> Result<u64, String> {
    if !path.exists() {
        return Ok(0);
    }
    let mut total = 0u64;
    for entry in std::fs::read_dir(path).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let meta = entry.metadata().map_err(|e| e.to_string())?;
        if meta.is_dir() {
            total += directory_size_bytes(&entry.path())?;
        } else {
            total += meta.len();
        }
    }
    Ok(total)
}

fn count_directory_children(path: &Path) -> Result<u64, String> {
    if !path.exists() {
        return Ok(0);
    }
    Ok(std::fs::read_dir(path)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .count() as u64)
}

fn clear_directory_children(path: &Path) -> Result<u64, String> {
    if !path.exists() {
        return Ok(0);
    }
    let mut removed = 0u64;
    for entry in std::fs::read_dir(path).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let entry_path = entry.path();
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            std::fs::remove_dir_all(&entry_path).map_err(|e| e.to_string())?;
        } else {
            std::fs::remove_file(&entry_path).map_err(|e| e.to_string())?;
        }
        removed += 1;
    }
    Ok(removed)
}

fn diagnostics_preview(app_handle: &tauri::AppHandle) -> Result<DiagnosticsPreview, String> {
    let db_path = storage_db_path()?;
    let telemetry_path = telemetry_events_path(app_handle)?;
    let model_dir = model_cache_dir(app_handle)?;
    let consent = read_telemetry_consent(app_handle)?;

    Ok(DiagnosticsPreview {
        fields: vec![
            "app_version".into(),
            "os".into(),
            "arch".into(),
            "telemetry_enabled".into(),
            "local_history_bytes".into(),
            "telemetry_events_bytes".into(),
            "model_cache_bytes".into(),
            "model_cache_items".into(),
        ],
        local_history_bytes: file_size_or_zero(&db_path),
        telemetry_events_bytes: file_size_or_zero(&telemetry_path),
        model_cache_bytes: directory_size_bytes(&model_dir)?,
        model_cache_items: count_directory_children(&model_dir)?,
        telemetry_enabled: consent.enabled,
    })
}

fn record_local_telemetry(
    app_handle: &tauri::AppHandle,
    request: TelemetryEventRequest,
) -> Result<(), String> {
    if !read_telemetry_consent(app_handle)?.enabled {
        return Ok(());
    }
    let record = telemetry_request_to_record(request)?;
    append_local_telemetry(app_handle, &record)
}

fn correction_op_type(before: &str, after: &str) -> &'static str {
    if before.is_empty() && !after.is_empty() {
        "insert"
    } else if !before.is_empty() && after.is_empty() {
        "delete"
    } else {
        "replace"
    }
}

fn supported_media_extension(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "mp3" | "wav" | "m4a" | "mp4" | "mov" | "aac" | "flac" | "ogg" | "webm" | "mkv"
    )
}

fn is_supported_media_path(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(supported_media_extension)
}

fn scan_media_folder(folder_path: &Path) -> Result<Vec<String>, String> {
    if !folder_path.is_dir() {
        return Err(format!("Not a folder: {}", folder_path.display()));
    }

    let mut files = Vec::new();
    scan_media_folder_inner(folder_path, &mut files)?;
    files.sort();
    files.dedup();
    Ok(files)
}

fn scan_media_folder_inner(folder_path: &Path, files: &mut Vec<String>) -> Result<(), String> {
    let entries = std::fs::read_dir(folder_path)
        .map_err(|e| format!("Failed to read folder {}: {e}", folder_path.display()))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read folder entry: {e}"))?;
        let path = entry.path();
        if path.is_dir() {
            scan_media_folder_inner(&path, files)?;
        } else if is_supported_media_path(&path) {
            files.push(path.display().to_string());
        }
    }

    Ok(())
}

fn inspect_media_file(path: &Path) -> Result<MediaFileInfo, String> {
    if !path.is_file() {
        return Err(format!("Not a file: {}", path.display()));
    }
    if !is_supported_media_path(path) {
        return Err(format!("Unsupported media file: {}", path.display()));
    }

    let metadata = std::fs::metadata(path)
        .map_err(|e| format!("Failed to inspect file {}: {e}", path.display()))?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("media")
        .to_string();
    let format = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_uppercase();

    Ok(MediaFileInfo {
        file_path: path.display().to_string(),
        file_name,
        format,
        size_bytes: metadata.len(),
        duration_seconds: probe_media_duration_seconds(path),
    })
}

fn job_summary_to_dto(
    storage: &audraflow_storage::Storage,
    job: audraflow_storage::JobRow,
) -> Result<JobSummaryDto, String> {
    let path = PathBuf::from(&job.file_path);
    let segments = storage
        .get_segments(&job.job_id)
        .map_err(|e| e.to_string())?;
    let duration_seconds = job.audio_duration_s.or_else(|| {
        segments
            .last()
            .map(|segment| segment.end_ms.max(0) as f64 / 1000.0)
    });

    Ok(JobSummaryDto {
        job_id: job.job_id,
        file_name: path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("media")
            .to_string(),
        format: path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_uppercase(),
        size_bytes: file_size_or_zero(&path),
        file_path: job.file_path,
        duration_seconds,
        state: job.state,
        extreme_accuracy: job.extreme_accuracy,
        segment_count: segments.len() as u32,
        created_at: job.created_at,
        completed_at: job.completed_at,
    })
}

fn probe_media_duration_seconds(path: &Path) -> Option<f64> {
    let output = std::process::Command::new(ffprobe_command())
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|duration| duration.is_finite() && *duration > 0.0)
}

fn yt_dlp_command() -> PathBuf {
    if let Some(path) = command_env_override("AUDRAFLOW_YT_DLP_BIN")
        .or_else(|| command_env_override("FT_YT_DLP_BIN"))
    {
        return path;
    }

    if let Some(path) = find_managed_tool(yt_dlp_binary_name()) {
        return path;
    }

    if let Some(path) =
        find_bundled_command("yt-dlp").or_else(|| find_dev_or_portable_tool(yt_dlp_binary_name()))
    {
        return path;
    }

    let winget_path = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|path| {
            path.join("Microsoft")
                .join("WinGet")
                .join("Packages")
                .join("yt-dlp.yt-dlp_Microsoft.Winget.Source_8wekyb3d8bbwe")
                .join("yt-dlp.exe")
        });

    match winget_path {
        Some(path) if path.exists() => path,
        _ => find_system_command("yt-dlp").unwrap_or_else(|| PathBuf::from("yt-dlp")),
    }
}

fn yt_dlp_binary_name() -> &'static str {
    if cfg!(windows) {
        "yt-dlp.exe"
    } else {
        "yt-dlp"
    }
}

fn managed_tools_bin_dir() -> PathBuf {
    app_data_dir().join("tools").join("bin")
}

fn managed_tool_path(name: &str) -> PathBuf {
    managed_tools_bin_dir().join(name)
}

fn find_managed_tool(name: &str) -> Option<PathBuf> {
    let path = managed_tool_path(name);
    path.is_file().then_some(path)
}

fn yt_dlp_download_url() -> &'static str {
    if cfg!(windows) {
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe"
    } else if cfg!(target_os = "macos") {
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos"
    } else {
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux"
    }
}

fn apply_yt_dlp_youtube_compat(command: &mut tokio::process::Command) {
    let extractor_args = std::env::var("AUDRAFLOW_YT_DLP_EXTRACTOR_ARGS")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "youtube:player_client=android_vr".to_string());
    command.arg("--extractor-args").arg(extractor_args);
}

fn ffmpeg_command() -> PathBuf {
    command_env_override("AUDRAFLOW_FFMPEG_BIN")
        .or_else(|| command_env_override("FT_FFMPEG_BIN"))
        .or_else(|| find_bundled_command("ffmpeg"))
        .or_else(|| find_dev_or_portable_tool(tool_binary_name("ffmpeg")))
        .unwrap_or_else(|| PathBuf::from("ffmpeg"))
}

fn ffprobe_command() -> PathBuf {
    command_env_override("AUDRAFLOW_FFPROBE_BIN")
        .or_else(|| command_env_override("FT_FFPROBE_BIN"))
        .or_else(|| find_bundled_command("ffprobe"))
        .or_else(|| find_dev_or_portable_tool(tool_binary_name("ffprobe")))
        .unwrap_or_else(|| PathBuf::from("ffprobe"))
}

fn command_env_override(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

fn find_system_command(name: &str) -> Option<PathBuf> {
    if let Some(path) = find_command_in_path(name) {
        return Some(path);
    }

    let mut candidates = vec![
        PathBuf::from("/usr/bin").join(name),
        PathBuf::from("/usr/local/bin").join(name),
        PathBuf::from("/snap/bin").join(name),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join(".local/bin").join(name));
    }

    candidates.into_iter().find(|path| path.exists())
}

fn find_command_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
        if cfg!(windows) {
            let exe_candidate = dir.join(format!("{name}.exe"));
            if exe_candidate.exists() {
                return Some(exe_candidate);
            }
        }
    }
    None
}

fn find_bundled_command(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    for root in exe.ancestors() {
        for candidate in bundled_command_candidates(root, name) {
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

fn bundled_command_candidates(root: &Path, name: &str) -> Vec<PathBuf> {
    let windows_name = if cfg!(windows) && !name.ends_with(".exe") {
        Some(format!("{name}.exe"))
    } else {
        None
    };
    let mut candidates = vec![
        root.join("bin").join(name),
        root.join("resources").join("bin").join(name),
        root.join("resources").join(name),
        root.join(name),
        root.join("external").join("ffmpeg").join("bin").join(name),
        root.join("tools").join("ffmpeg").join("bin").join(name),
    ];
    if let Some(windows_name) = windows_name {
        candidates.extend([
            root.join("bin").join(&windows_name),
            root.join("resources").join("bin").join(&windows_name),
            root.join("resources").join(&windows_name),
            root.join(&windows_name),
            root.join("external")
                .join("ffmpeg")
                .join("bin")
                .join(&windows_name),
            root.join("tools")
                .join("ffmpeg")
                .join("bin")
                .join(&windows_name),
        ]);
    }
    candidates
}

fn format_seconds_arg(seconds: f64) -> String {
    let rounded = seconds.round();
    if (seconds - rounded).abs() < 0.001 {
        format!("{}", rounded as u64)
    } else {
        let formatted = format!("{seconds:.3}");
        formatted
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

fn normalize_skip_start_seconds(value: Option<f64>) -> Result<f64, String> {
    let seconds = value.unwrap_or(0.0);
    if !seconds.is_finite() || seconds < 0.0 {
        return Err("Skip intro must be 0 or a positive number of seconds".into());
    }
    Ok(seconds.min(MAX_SKIP_START_SECONDS))
}

fn normalize_url_preview_seconds(value: Option<f64>) -> Result<f64, String> {
    let seconds = value.unwrap_or(DEFAULT_URL_PREVIEW_SECONDS);
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err("Preview duration must be a positive number of seconds".into());
    }
    Ok(seconds.min(MAX_URL_PREVIEW_SECONDS))
}

fn trim_stderr(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr).trim().to_string();
    let mut chars = text.chars();
    let shortened: String = chars.by_ref().take(1200).collect();
    if chars.next().is_none() {
        text
    } else {
        format!("{shortened}...")
    }
}

#[cfg(target_os = "windows")]
fn pipe_exists() -> bool {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(ORCHESTRATOR_PIPE)
        .is_ok()
}

#[cfg(not(target_os = "windows"))]
fn orchestrator_socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("audraflow-orchestrator.sock")
}

#[cfg(not(target_os = "windows"))]
fn socket_exists() -> bool {
    std::os::unix::net::UnixStream::connect(orchestrator_socket_path()).is_ok()
}

fn workspace_root_from_current_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    for ancestor in exe.ancestors() {
        if ancestor.join("Cargo.toml").exists() && ancestor.join("orchestrator").exists() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn start_orchestrator(app_handle: &tauri::AppHandle) {
    if pipe_exists() {
        log::info!("Orchestrator pipe already available");
        return;
    }

    let mut command = if cfg!(debug_assertions) {
        let Some(workspace_root) = workspace_root_from_current_exe() else {
            log::warn!("Could not locate workspace root; orchestrator was not started");
            return;
        };

        let mut command = std::process::Command::new("cargo");
        command
            .arg("run")
            .arg("-p")
            .arg("audraflow-orchestrator")
            .arg("--bin")
            .arg("audraflow-orchestrator")
            .current_dir(&workspace_root);
        command
    } else {
        let Some(app_dir) = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
        else {
            log::warn!("Could not locate app executable directory; orchestrator was not started");
            return;
        };

        let orchestrator_exe = app_dir.join("audraflow-orchestrator.exe");
        let mut command = std::process::Command::new(orchestrator_exe);
        command.current_dir(app_dir);
        command
    };

    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Ok(resource_dir) = app_handle.path().resource_dir() {
        command.env("AUDRAFLOW_RESOURCE_DIR", resource_dir);
    }

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    match command.spawn() {
        Ok(child) => log::info!("Started orchestrator process: {}", child.id()),
        Err(e) => log::error!("Failed to start orchestrator: {}", e),
    }
}

#[cfg(not(target_os = "windows"))]
fn start_orchestrator(app_handle: &tauri::AppHandle) {
    if socket_exists() {
        log::info!("Orchestrator socket already available");
        return;
    }

    let mut command = if cfg!(debug_assertions) {
        let Some(workspace_root) = workspace_root_from_current_exe() else {
            log::warn!("Could not locate workspace root; orchestrator was not started");
            return;
        };

        let debug_orchestrator = workspace_root
            .join("target")
            .join("debug")
            .join("audraflow-orchestrator");
        if debug_orchestrator.is_file() {
            let mut command = std::process::Command::new(debug_orchestrator);
            command.current_dir(&workspace_root);
            command
        } else {
            let mut command = std::process::Command::new("cargo");
            command
                .arg("run")
                .arg("-p")
                .arg("audraflow-orchestrator")
                .arg("--bin")
                .arg("audraflow-orchestrator")
                .current_dir(&workspace_root);
            command
        }
    } else {
        let Some(app_dir) = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
        else {
            log::warn!("Could not locate app executable directory; orchestrator was not started");
            return;
        };

        let orchestrator_exe = app_dir.join("audraflow-orchestrator");
        let mut command = std::process::Command::new(orchestrator_exe);
        command.current_dir(app_dir);
        command
    };

    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Ok(resource_dir) = app_handle.path().resource_dir() {
        command.env("AUDRAFLOW_RESOURCE_DIR", resource_dir);
    }

    match command.spawn() {
        Ok(child) => log::info!("Started orchestrator process: {}", child.id()),
        Err(e) => log::error!("Failed to start orchestrator: {}", e),
    }
}

fn sanitize_remote_filename(raw: &str) -> String {
    let name = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("remote-audio")
        .split('?')
        .next()
        .unwrap_or("remote-audio")
        .split('#')
        .next()
        .unwrap_or("remote-audio");

    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();

    let trimmed = sanitized.trim_matches(['.', '_', '-']);
    if trimmed.is_empty() {
        "remote-audio".into()
    } else {
        trimmed.chars().take(120).collect()
    }
}

fn extension_from_content_type(content_type: Option<&str>) -> Option<&'static str> {
    let value = content_type?.split(';').next()?.trim().to_ascii_lowercase();
    match value.as_str() {
        "audio/mpeg" | "audio/mp3" => Some("mp3"),
        "audio/wav" | "audio/x-wav" | "audio/vnd.wave" => Some("wav"),
        "audio/mp4" | "audio/aac" => Some("m4a"),
        "audio/flac" | "audio/x-flac" => Some("flac"),
        "audio/ogg" | "application/ogg" => Some("ogg"),
        "video/mp4" => Some("mp4"),
        "video/quicktime" => Some("mov"),
        "video/webm" | "audio/webm" => Some("webm"),
        "video/x-matroska" => Some("mkv"),
        _ => None,
    }
}

fn filename_from_headers_or_url(url: &str, content_type: Option<&str>) -> Result<String, String> {
    let mut name = sanitize_remote_filename(url);
    let ext = Path::new(&name)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());

    if let Some(ext) = ext {
        if supported_media_extension(&ext) {
            return Ok(name);
        }
    }

    if let Some(ext) = extension_from_content_type(content_type) {
        name = format!("{name}.{ext}");
        return Ok(name);
    }

    Err("URL must point directly to a supported audio/video file".into())
}

async fn download_remote_media(
    app_handle: &tauri::AppHandle,
    client_job_id: &str,
    url: &str,
    skip_start_seconds: f64,
) -> Result<PathBuf, String> {
    emit_job_log(
        app_handle,
        client_job_id,
        "info",
        "Checking direct media URL",
    );
    emit_job_progress(
        app_handle,
        client_job_id,
        "import",
        5.0,
        "Checking direct media URL",
    );
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("Invalid URL: {e}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => return Err("Only http and https links are supported".into()),
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(REMOTE_MEDIA_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| e.to_string())?;

    let response = client
        .get(parsed)
        .send()
        .await
        .map_err(|e| format!("Failed to download URL: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Failed to download URL: {e}"))?;

    let content_length = response.content_length();
    if content_length.is_some_and(|len| len > MAX_REMOTE_MEDIA_BYTES) {
        return Err("Remote media is larger than the 2 GB limit".into());
    }
    if let Some(len) = content_length {
        emit_job_log(
            app_handle,
            client_job_id,
            "info",
            format!("Direct media size: {:.1} MB", len as f64 / 1024.0 / 1024.0),
        );
        emit_job_progress(
            app_handle,
            client_job_id,
            "download",
            10.0,
            format!(
                "Downloading direct media ({:.1} MB)",
                len as f64 / 1024.0 / 1024.0
            ),
        );
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string());

    if content_type
        .as_deref()
        .is_some_and(|value| value.to_ascii_lowercase().contains("text/html"))
    {
        return Err("URL returned an HTML page, not a direct media file".into());
    }

    let filename = filename_from_headers_or_url(url, content_type.as_deref())?;
    emit_job_log(
        app_handle,
        client_job_id,
        "info",
        format!("Downloading direct media as {filename}"),
    );
    if content_length.is_none() {
        emit_job_progress(
            app_handle,
            client_job_id,
            "download",
            10.0,
            "Downloading direct media",
        );
    }
    let cache_dir = app_handle
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("remote-media");
    std::fs::create_dir_all(&cache_dir).map_err(|e| format!("Failed to create cache dir: {e}"))?;

    let output_path = cache_dir.join(format!("{}-{filename}", uuid::Uuid::new_v4()));
    let mut file = std::fs::File::create(&output_path)
        .map_err(|e| format!("Failed to create downloaded file: {e}"))?;

    let mut downloaded = 0_u64;
    let mut stream = response.bytes_stream();
    use futures_util::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Failed while downloading URL: {e}"))?;
        downloaded = downloaded
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| "Remote media size overflowed".to_string())?;
        if downloaded > MAX_REMOTE_MEDIA_BYTES {
            let _ = std::fs::remove_file(&output_path);
            return Err("Remote media is larger than the 2 GB limit".into());
        }

        if let Some(total) = content_length {
            let pct = 10.0 + (downloaded as f64 / total as f64) * 70.0;
            emit_job_progress(
                app_handle,
                client_job_id,
                "download",
                pct,
                format!(
                    "Downloaded {:.1} / {:.1} MB",
                    downloaded as f64 / 1024.0 / 1024.0,
                    total as f64 / 1024.0 / 1024.0
                ),
            );
        } else {
            emit_job_progress(
                app_handle,
                client_job_id,
                "download",
                40.0,
                format!("Downloaded {:.1} MB", downloaded as f64 / 1024.0 / 1024.0),
            );
        }

        use std::io::Write;
        file.write_all(&chunk)
            .map_err(|e| format!("Failed to save downloaded file: {e}"))?;
    }

    if downloaded == 0 {
        let _ = std::fs::remove_file(&output_path);
        return Err("Remote media download was empty".into());
    }

    emit_job_log(
        app_handle,
        client_job_id,
        "info",
        format!(
            "Direct download complete: {:.1} MB",
            downloaded as f64 / 1024.0 / 1024.0
        ),
    );
    emit_job_progress(
        app_handle,
        client_job_id,
        "download",
        85.0,
        "Direct download complete",
    );
    trim_media_start_if_needed(app_handle, client_job_id, output_path, skip_start_seconds).await
}

async fn trim_media_start_if_needed(
    app_handle: &tauri::AppHandle,
    client_job_id: &str,
    input_path: PathBuf,
    skip_start_seconds: f64,
) -> Result<PathBuf, String> {
    if skip_start_seconds <= 0.0 {
        return Ok(input_path);
    }

    let skip_arg = format_seconds_arg(skip_start_seconds);
    emit_job_log(
        app_handle,
        client_job_id,
        "info",
        format!("Skipping first {skip_arg} seconds with ffmpeg"),
    );
    emit_job_progress(
        app_handle,
        client_job_id,
        "trim",
        86.0,
        format!("Skipping first {skip_arg} seconds"),
    );

    let output_path = input_path.with_file_name(format!(
        "{}-skip-{}.m4a",
        input_path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("media"),
        skip_arg.replace('.', "_")
    ));

    let output = tokio::process::Command::new(ffmpeg_command())
        .arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-ss")
        .arg(&skip_arg)
        .arg("-i")
        .arg(&input_path)
        .arg("-vn")
        .arg("-c:a")
        .arg("aac")
        .arg("-b:a")
        .arg("128k")
        .arg(&output_path)
        .output()
        .await
        .map_err(|e| format!("ffmpeg is required to skip the intro for direct media links: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "ffmpeg could not skip the intro: {}",
            trim_stderr(&output.stderr)
        ));
    }

    let size = std::fs::metadata(&output_path)
        .map_err(|e| format!("Failed to inspect trimmed media: {e}"))?
        .len();
    if size == 0 {
        let _ = std::fs::remove_file(&output_path);
        return Err("Trimmed media was empty after skipping the intro".into());
    }

    emit_job_log(
        app_handle,
        client_job_id,
        "info",
        format!(
            "Intro skipped: created {} ({:.1} MB)",
            output_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("trimmed media"),
            size as f64 / 1024.0 / 1024.0
        ),
    );
    emit_job_progress(
        app_handle,
        client_job_id,
        "trim",
        89.0,
        "Intro skip complete",
    );

    Ok(output_path)
}

async fn download_platform_media(
    app_handle: &tauri::AppHandle,
    client_job_id: &str,
    url: &str,
    audio_quality: &str,
    audio_format: &str,
    skip_start_seconds: f64,
) -> Result<PathBuf, String> {
    emit_job_log(
        app_handle,
        client_job_id,
        "info",
        "Resolving platform link with yt-dlp",
    );
    emit_job_progress(
        app_handle,
        client_job_id,
        "resolve",
        15.0,
        "Resolving platform link with yt-dlp",
    );
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("Invalid URL: {e}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => return Err("Only http and https links are supported".into()),
    }

    let cache_dir = app_handle
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("platform-media")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&cache_dir).map_err(|e| format!("Failed to create cache dir: {e}"))?;

    let audio_format = normalize_audio_format(audio_format);
    let audio_quality = normalize_audio_quality(audio_quality);
    let skip_arg = format_seconds_arg(skip_start_seconds);
    let output_template = cache_dir.join("media.%(ext)s");
    let mut command = tokio::process::Command::new(yt_dlp_command());
    apply_yt_dlp_youtube_compat(&mut command);
    command
        .arg("--no-playlist")
        .arg("--newline")
        .arg("--windows-filenames")
        .arg("--max-filesize")
        .arg(format!("{}M", MAX_REMOTE_MEDIA_BYTES / 1024 / 1024))
        .arg("-f")
        .arg(yt_dlp_format_selector(audio_quality))
        .arg("-o")
        .arg(&output_template);

    if skip_start_seconds > 0.0 {
        command
            .arg("--download-sections")
            .arg(format!("*{skip_arg}-inf"))
            .arg("--force-keyframes-at-cuts");
        emit_job_log(
            app_handle,
            client_job_id,
            "info",
            format!("Skipping first {skip_arg} seconds before platform download"),
        );
    }

    if audio_format != "source" {
        command
            .arg("-x")
            .arg("--audio-format")
            .arg(audio_format)
            .arg("--audio-quality")
            .arg(yt_dlp_audio_quality_arg(audio_quality));
    } else {
        command.arg("--merge-output-format").arg("mp4");
    }

    command
        .arg(url)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1");

    emit_job_log(
        app_handle,
        client_job_id,
        "info",
        format!(
            "Starting platform download (quality: {}, format: {}, skip: {} sec)",
            audio_quality, audio_format, skip_arg
        ),
    );
    emit_job_progress(
        app_handle,
        client_job_id,
        "download",
        25.0,
        "Starting platform download",
    );
    let mut child = command
        .spawn()
        .map_err(|e| {
            format!(
                "yt-dlp is required for platform links but was not found: {e}. Reinstall AudraFlow or set AUDRAFLOW_YT_DLP_BIN to its executable path."
            )
        })?;

    let mut log_tasks = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        let app_handle = app_handle.clone();
        let client_job_id = client_job_id.to_string();
        log_tasks.push(tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim();
                if !line.is_empty() {
                    emit_job_log(&app_handle, &client_job_id, "info", line.to_string());
                    if let Some(pct) = parse_yt_dlp_progress(line) {
                        emit_job_progress(
                            &app_handle,
                            &client_job_id,
                            "download",
                            25.0 + pct * 0.55,
                            line.to_string(),
                        );
                    }
                }
            }
        }));
    }
    if let Some(stderr) = child.stderr.take() {
        let app_handle = app_handle.clone();
        let client_job_id = client_job_id.to_string();
        log_tasks.push(tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim();
                if !line.is_empty() {
                    let level = if line.to_ascii_lowercase().contains("error") {
                        "error"
                    } else {
                        "warn"
                    };
                    emit_job_log(&app_handle, &client_job_id, level, line.to_string());
                    if let Some(pct) = parse_yt_dlp_progress(line) {
                        emit_job_progress(
                            &app_handle,
                            &client_job_id,
                            "download",
                            25.0 + pct * 0.55,
                            line.to_string(),
                        );
                    }
                }
            }
        }));
    }

    let output = tokio::time::timeout(
        Duration::from_secs(PLATFORM_DOWNLOAD_TIMEOUT_SECS),
        child.wait(),
    )
    .await
    .map_err(|_| "Platform download timed out".to_string())?
    .map_err(|e| format!("Failed to run yt-dlp: {e}"))?;

    for task in log_tasks {
        let _ = task.await;
    }

    if !output.success() {
        let message = "yt-dlp could not download this link";
        emit_job_log(app_handle, client_job_id, "error", message);
        return Err(format!("Platform download failed: {message}"));
    }

    let mut candidates = Vec::new();
    for entry in
        std::fs::read_dir(&cache_dir).map_err(|e| format!("Failed to read cache dir: {e}"))?
    {
        let entry = entry.map_err(|e| format!("Failed to read downloaded file: {e}"))?;
        let path = entry.path();
        if path.is_file() && is_supported_media_path(&path) {
            let size = entry
                .metadata()
                .map_err(|e| format!("Failed to inspect downloaded file: {e}"))?
                .len();
            candidates.push((path, size));
        }
    }

    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.1));
    let (path, size) = candidates
        .into_iter()
        .next()
        .ok_or_else(|| "Platform link did not produce a supported audio/video file".to_string())?;

    if size == 0 {
        let _ = std::fs::remove_file(&path);
        return Err("Platform download was empty".into());
    }
    if size > MAX_REMOTE_MEDIA_BYTES {
        let _ = std::fs::remove_file(&path);
        return Err("Remote media is larger than the 2 GB limit".into());
    }

    emit_job_log(
        app_handle,
        client_job_id,
        "info",
        format!(
            "Platform download complete: {} ({:.1} MB)",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("media file"),
            size as f64 / 1024.0 / 1024.0
        ),
    );
    emit_job_progress(
        app_handle,
        client_job_id,
        "download",
        85.0,
        "Platform download complete",
    );
    Ok(path)
}

fn parse_yt_dlp_progress(line: &str) -> Option<f64> {
    let marker = "[download]";
    let text = line.strip_prefix(marker)?.trim_start();
    let pct_end = text.find('%')?;
    let pct_text = text[..pct_end].trim();
    pct_text.parse::<f64>().ok()
}

fn normalize_audio_quality(value: &str) -> &'static str {
    match value {
        "small" => "small",
        "medium" => "medium",
        "best" => "best",
        _ => "auto",
    }
}

fn normalize_audio_format(value: &str) -> &'static str {
    match value {
        "mp3" => "mp3",
        "m4a" => "m4a",
        "wav" => "wav",
        _ => "source",
    }
}

fn yt_dlp_format_selector(audio_quality: &str) -> &'static str {
    match audio_quality {
        "small" => "ba[abr<=64]/ba[filesize<20M]/worstaudio/best",
        "medium" | "auto" => "ba[abr<=128]/ba/bestaudio/best",
        "best" => "ba/bestaudio/best",
        _ => "ba/bestaudio/best",
    }
}

fn yt_dlp_audio_quality_arg(audio_quality: &str) -> &'static str {
    match audio_quality {
        "small" => "64K",
        "medium" | "auto" => "128K",
        "best" => "0",
        _ => "128K",
    }
}

async fn download_url_media(
    app_handle: &tauri::AppHandle,
    client_job_id: &str,
    url: &str,
    audio_quality: &str,
    audio_format: &str,
    skip_start_seconds: f64,
) -> Result<PathBuf, String> {
    match download_remote_media(app_handle, client_job_id, url, skip_start_seconds).await {
        Ok(path) => Ok(path),
        Err(direct_error) => {
            emit_job_log(
                app_handle,
                client_job_id,
                "warn",
                format!(
                    "Direct media download failed, trying platform resolver: {}",
                    direct_error
                ),
            );
            log::info!(
                "Direct media download failed; trying platform resolver: {}",
                direct_error
            );
            download_platform_media(
                app_handle,
                client_job_id,
                url,
                audio_quality,
                audio_format,
                skip_start_seconds,
            ).await.map_err(|platform_error| {
                format!("Direct download failed: {direct_error}; platform download failed: {platform_error}")
            })
        }
    }
}

async fn create_url_preview(
    app_handle: &tauri::AppHandle,
    url: &str,
    preview_seconds: f64,
) -> Result<UrlPreviewResponse, String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("Invalid URL: {e}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => return Err("Only http and https links are supported".into()),
    }

    let cache_dir = app_handle
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("url-previews")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| format!("Failed to create preview dir: {e}"))?;

    match create_platform_preview(&cache_dir, url, preview_seconds).await {
        Ok(path) => Ok(UrlPreviewResponse {
            file_path: path.to_string_lossy().into_owned(),
            preview_seconds,
            source: "yt-dlp",
            message: format!(
                "Preview ready: first {} seconds resolved with yt-dlp",
                format_seconds_arg(preview_seconds)
            ),
        }),
        Err(platform_error) => {
            log::warn!("yt-dlp preview failed, trying ffmpeg: {}", platform_error);
            match create_direct_preview(&cache_dir, url, preview_seconds).await {
                Ok(path) => Ok(UrlPreviewResponse {
                    file_path: path.to_string_lossy().into_owned(),
                    preview_seconds,
                    source: "ffmpeg",
                    message: format!(
                        "Preview ready: first {} seconds captured from direct media",
                        format_seconds_arg(preview_seconds)
                    ),
                }),
                Err(direct_error) => Err(format!(
                    "Could not create URL preview. yt-dlp failed: {platform_error}; ffmpeg failed: {direct_error}"
                )),
            }
        }
    }
}

async fn create_platform_preview(
    cache_dir: &Path,
    url: &str,
    preview_seconds: f64,
) -> Result<PathBuf, String> {
    let output_template = cache_dir.join("preview.%(ext)s");
    let section = format!("*0-{}", format_seconds_arg(preview_seconds));
    let output = tokio::time::timeout(Duration::from_secs(URL_PREVIEW_TIMEOUT_SECS), {
        let mut command = tokio::process::Command::new(yt_dlp_command());
        apply_yt_dlp_youtube_compat(&mut command);
        command
            .arg("--no-playlist")
            .arg("--newline")
            .arg("--windows-filenames")
            .arg("--download-sections")
            .arg(section)
            .arg("--force-keyframes-at-cuts")
            .arg("-f")
            .arg("ba[abr<=128]/ba/bestaudio/best")
            .arg("-x")
            .arg("--audio-format")
            .arg("m4a")
            .arg("--audio-quality")
            .arg("128K")
            .arg("-o")
            .arg(&output_template)
            .arg(url)
            .env("PYTHONIOENCODING", "utf-8")
            .env("PYTHONUTF8", "1")
            .output()
    })
    .await
    .map_err(|_| "yt-dlp preview timed out".to_string())?
    .map_err(|e| format!("Failed to run yt-dlp: {e}"))?;

    if !output.status.success() {
        return Err(trim_stderr(&output.stderr));
    }

    find_preview_file(cache_dir)
}

async fn create_direct_preview(
    cache_dir: &Path,
    url: &str,
    preview_seconds: f64,
) -> Result<PathBuf, String> {
    let output_path = cache_dir.join("preview.m4a");
    let output = tokio::time::timeout(
        Duration::from_secs(URL_PREVIEW_TIMEOUT_SECS),
        tokio::process::Command::new(ffmpeg_command())
            .arg("-y")
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-i")
            .arg(url)
            .arg("-t")
            .arg(format_seconds_arg(preview_seconds))
            .arg("-vn")
            .arg("-c:a")
            .arg("aac")
            .arg("-b:a")
            .arg("128k")
            .arg(&output_path)
            .output(),
    )
    .await
    .map_err(|_| "ffmpeg preview timed out".to_string())?
    .map_err(|e| format!("Failed to run ffmpeg: {e}"))?;

    if !output.status.success() {
        return Err(trim_stderr(&output.stderr));
    }

    ensure_non_empty_preview(output_path)
}

fn find_preview_file(cache_dir: &Path) -> Result<PathBuf, String> {
    let mut candidates = Vec::new();
    for entry in
        std::fs::read_dir(cache_dir).map_err(|e| format!("Failed to read preview dir: {e}"))?
    {
        let entry = entry.map_err(|e| format!("Failed to inspect preview file: {e}"))?;
        let path = entry.path();
        if path.is_file() && is_supported_media_path(&path) {
            let size = entry
                .metadata()
                .map_err(|e| format!("Failed to inspect preview file: {e}"))?
                .len();
            candidates.push((path, size));
        }
    }

    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.1));
    let (path, size) = candidates
        .into_iter()
        .next()
        .ok_or_else(|| "Preview did not produce a supported audio file".to_string())?;
    ensure_non_empty_preview_with_size(path, size)
}

fn ensure_non_empty_preview(path: PathBuf) -> Result<PathBuf, String> {
    let size = std::fs::metadata(&path)
        .map_err(|e| format!("Failed to inspect preview file: {e}"))?
        .len();
    ensure_non_empty_preview_with_size(path, size)
}

fn ensure_non_empty_preview_with_size(path: PathBuf, size: u64) -> Result<PathBuf, String> {
    if size == 0 {
        let _ = std::fs::remove_file(&path);
        return Err("Preview media was empty".into());
    }
    Ok(path)
}

#[allow(clippy::too_many_arguments)]
fn create_job_for_local_file(
    app_handle: &tauri::AppHandle,
    file_path: String,
    file_hash: String,
    asr_engine: Option<String>,
    language: Option<String>,
    audio_mode: Option<String>,
    vocal_separation: Option<String>,
    extreme_accuracy: bool,
    export_formats: Vec<String>,
) -> Result<JobStatus, String> {
    let job_id = uuid::Uuid::new_v4().to_string();
    log::info!("Creating job {} for file: {}", job_id, file_path);
    let file_hash = if file_hash.trim().is_empty() {
        hash_file_sha256(&file_path)?
    } else {
        file_hash
    };
    let requested_asr_engine = normalize_asr_engine(asr_engine.as_deref());
    let audio_mode = normalize_audio_mode(audio_mode.as_deref());
    let manager = model_manager(app_handle)?;
    ensure_bundled_default_model(app_handle, &manager)?;
    let selected_model = manager.selected_model().map_err(|e| e.to_string())?;
    let installed_models = manager.list_installed_models().map_err(|e| e.to_string())?;
    let has_whisper_model = preferred_lyrics_whisper_model(
        &installed_models,
        selected_model.as_ref(),
        extreme_accuracy,
    )
    .is_some();
    let asr_engine = resolve_asr_engine(&requested_asr_engine, &audio_mode, has_whisper_model);
    let selected_model = if asr_engine == "funasr" {
        None
    } else {
        resolve_whisper_model_for_job(
            &asr_engine,
            &audio_mode,
            selected_model,
            &installed_models,
            extreme_accuracy,
        )
    };
    let selected_model = if asr_engine == "whisper" {
        Some(selected_model.ok_or_else(|| {
            "No ASR model is selected. Import and select a ggml model in Settings before starting Whisper transcription.".to_string()
        })?)
    } else {
        selected_model
    };
    if let Some(selected_model) = selected_model.as_ref() {
        if !selected_model.path.is_file() {
            return Err(format!(
                "Selected ASR model file is missing: {}. Re-import or select another model in Settings.",
                selected_model.path.display()
            ));
        }
    }
    preflight_transcription_dependencies(app_handle, &asr_engine)?;
    let transcription_language =
        normalize_transcription_language(language.as_deref(), selected_model.as_ref());
    let vocal_separation =
        normalize_vocal_separation(vocal_separation.as_deref(), audio_mode.as_str());
    let model_path = selected_model
        .as_ref()
        .map(|model| model.path.to_string_lossy().into_owned());
    let model_name = selected_model.as_ref().map(|model| model.info.name.clone());
    let model_version = selected_model
        .as_ref()
        .map(|model| model.info.version.clone());

    let response = send_orchestrator_message(IpcMessage::JobCreate(JobCreate {
        job_id: job_id.clone(),
        file_path,
        file_hash,
        extreme_accuracy,
        export_formats,
        asr_engine: Some(asr_engine),
        model_path,
        model_name,
        model_version,
        language: Some(transcription_language),
        audio_mode: Some(audio_mode),
        vocal_separation: Some(vocal_separation),
        audio_duration_s: None,
        snr_db: None,
        estimated_speakers: None,
    }))?;

    match response {
        IpcMessage::JobPlan(_) => Ok(JobStatus {
            job_id,
            state: JobState::Pending,
            progress_pct: 0.0,
            message: Some("Queued".into()),
            estimated_remaining_s: None,
            rtf_current: None,
            ttfv_s: None,
        }),
        IpcMessage::JobStatus(status) => Ok(status),
        other => Err(format!("Unexpected orchestrator response: {other:?}")),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            log::info!("AudraFlow v0.1.0 started");
            start_orchestrator(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            cmd_create_job,
            cmd_scan_media_folder,
            cmd_inspect_media_files,
            cmd_create_job_from_url,
            cmd_create_url_preview,
            cmd_list_jobs,
            cmd_get_job_status,
            cmd_cancel_job,
            cmd_pause_job,
            cmd_resume_job,
            cmd_retry_job,
            cmd_skip_job,
            cmd_get_transcript,
            cmd_search_transcript,
            cmd_update_segment,
            cmd_accept_term_candidate,
            cmd_add_glossary_entry,
            cmd_save_glossary_entry,
            cmd_list_glossary_entries,
            cmd_delete_glossary_entry,
            cmd_update_speaker_label,
            cmd_add_timestamp_mark,
            cmd_record_telemetry_event,
            cmd_get_telemetry_consent,
            cmd_set_telemetry_consent,
            cmd_clear_local_history,
            cmd_delete_model_cache,
            cmd_get_model_settings,
            cmd_get_model_catalog,
            cmd_import_local_model,
            cmd_download_model,
            cmd_select_model,
            cmd_delete_model,
            cmd_clear_unused_models,
            cmd_get_diagnostics_preview,
            cmd_get_device_diagnostics,
            cmd_get_runtime_health,
            cmd_repair_runtime_dependency,
            cmd_export_diagnostics_package,
            cmd_export_transcript,
            cmd_render_transcript_export,
            cmd_estimate_job,
            cmd_activate_license,
            cmd_get_license_state,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// ── Tauri Commands ─────────────────────────────────────────────────────────

#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn cmd_create_job(
    app_handle: tauri::AppHandle,
    file_path: String,
    file_hash: String,
    asr_engine: Option<String>,
    language: Option<String>,
    audio_mode: Option<String>,
    vocal_separation: Option<String>,
    extreme_accuracy: bool,
    export_formats: Vec<String>,
) -> Result<JobStatus, String> {
    create_job_for_local_file(
        &app_handle,
        file_path,
        file_hash,
        asr_engine,
        language,
        audio_mode,
        vocal_separation,
        extreme_accuracy,
        export_formats,
    )
}

#[tauri::command]
async fn cmd_scan_media_folder(folder_path: String) -> Result<Vec<String>, String> {
    let folder = PathBuf::from(folder_path.trim());
    let files = scan_media_folder(&folder)?;
    if files.is_empty() {
        Err("No supported audio or video files were found in this folder".into())
    } else {
        Ok(files)
    }
}

#[tauri::command]
async fn cmd_inspect_media_files(file_paths: Vec<String>) -> Result<Vec<MediaFileInfo>, String> {
    let mut infos = Vec::new();
    for file_path in file_paths {
        let path = PathBuf::from(file_path.trim());
        if is_supported_media_path(&path) {
            infos.push(inspect_media_file(&path)?);
        }
    }

    if infos.is_empty() {
        Err("No supported audio or video files were selected".into())
    } else {
        Ok(infos)
    }
}

#[tauri::command]
async fn cmd_create_job_from_url(
    app_handle: tauri::AppHandle,
    request: CreateJobFromUrlRequest,
) -> Result<JobStatus, String> {
    log::info!("Creating job from URL: {}", request.url);
    let client_job_id = if request.client_job_id.trim().is_empty() {
        uuid::Uuid::new_v4().to_string()
    } else {
        request.client_job_id
    };
    emit_job_log(&app_handle, &client_job_id, "info", "URL import started");
    emit_job_progress(
        &app_handle,
        &client_job_id,
        "import",
        1.0,
        "URL import started",
    );
    let skip_start_seconds = normalize_skip_start_seconds(request.skip_start_seconds)?;
    if skip_start_seconds > 0.0 {
        emit_job_log(
            &app_handle,
            &client_job_id,
            "info",
            format!(
                "Intro skip enabled: first {} seconds will be ignored",
                format_seconds_arg(skip_start_seconds)
            ),
        );
    }
    if let Err(error) = preflight_url_import_dependencies(request.url.trim(), skip_start_seconds) {
        emit_job_log(&app_handle, &client_job_id, "error", error.clone());
        emit_job_progress(&app_handle, &client_job_id, "import", 100.0, error.clone());
        return Err(error);
    }
    if let Err(error) = preflight_requested_transcription_dependencies(
        &app_handle,
        request.asr_engine.as_deref(),
        request.audio_mode.as_deref(),
        request.extreme_accuracy,
    ) {
        emit_job_log(&app_handle, &client_job_id, "error", error.clone());
        emit_job_progress(&app_handle, &client_job_id, "import", 100.0, error.clone());
        return Err(error);
    }
    let file_path = download_url_media(
        &app_handle,
        &client_job_id,
        request.url.trim(),
        request.audio_quality.as_deref().unwrap_or("auto"),
        request.audio_format.as_deref().unwrap_or("source"),
        skip_start_seconds,
    )
    .await?;
    emit_job_log(
        &app_handle,
        &client_job_id,
        "info",
        "Creating transcription job",
    );
    emit_job_progress(
        &app_handle,
        &client_job_id,
        "queue",
        90.0,
        "Creating transcription job",
    );
    let status = create_job_for_local_file(
        &app_handle,
        file_path.to_string_lossy().into_owned(),
        String::new(),
        request.asr_engine,
        request.language,
        request.audio_mode,
        request.vocal_separation,
        request.extreme_accuracy,
        request.export_formats,
    )?;
    emit_job_log(
        &app_handle,
        &client_job_id,
        "info",
        format!("Queued as backend job {}", status.job_id),
    );
    emit_job_log(
        &app_handle,
        &status.job_id,
        "info",
        "Job is queued and waiting for processing",
    );
    emit_job_progress(
        &app_handle,
        &client_job_id,
        "queued",
        100.0,
        "Import complete; waiting for transcription runtime",
    );
    emit_job_progress(
        &app_handle,
        &status.job_id,
        "queued",
        100.0,
        "Import complete; waiting for transcription runtime",
    );
    Ok(status)
}

#[tauri::command]
async fn cmd_create_url_preview(
    app_handle: tauri::AppHandle,
    request: UrlPreviewRequest,
) -> Result<UrlPreviewResponse, String> {
    let preview_seconds = normalize_url_preview_seconds(request.preview_seconds)?;
    create_url_preview(&app_handle, request.url.trim(), preview_seconds).await
}

#[tauri::command]
async fn cmd_list_jobs(limit: Option<u32>) -> Result<Vec<JobSummaryDto>, String> {
    let db_path = storage_db_path()?;
    let storage = audraflow_storage::Storage::open(&db_path).map_err(|e| e.to_string())?;
    let limit = limit.unwrap_or(100).clamp(1, 500) as usize;
    storage
        .list_jobs(limit)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|job| job_summary_to_dto(&storage, job))
        .filter(|result| {
            result
                .as_ref()
                .map(|job| job.state == "completed")
                .unwrap_or(true)
        })
        .collect()
}

#[tauri::command]
async fn cmd_get_job_status(job_id: String) -> Result<JobStatus, String> {
    log::debug!("Querying status for job: {}", job_id);
    let response = send_orchestrator_message(IpcMessage::JobStatus(JobStatus {
        job_id: job_id.clone(),
        state: JobState::Pending,
        progress_pct: 0.0,
        message: None,
        estimated_remaining_s: None,
        rtf_current: None,
        ttfv_s: None,
    }))?;
    expect_job_status(response)
}

#[tauri::command]
async fn cmd_cancel_job(job_id: String) -> Result<JobStatus, String> {
    log::info!("Cancelling job: {}", job_id);
    let response = send_orchestrator_message(IpcMessage::JobCancel(JobControl {
        job_id,
        reason: Some("user_cancelled".into()),
    }))?;
    expect_job_status(response)
}

#[tauri::command]
async fn cmd_pause_job(job_id: String) -> Result<JobStatus, String> {
    log::info!("Pausing job: {}", job_id);
    let response = send_orchestrator_message(IpcMessage::JobPause(JobControl {
        job_id,
        reason: Some("user_paused".into()),
    }))?;
    expect_job_status(response)
}

#[tauri::command]
async fn cmd_resume_job(job_id: String) -> Result<JobStatus, String> {
    log::info!("Resuming job: {}", job_id);
    let response = send_orchestrator_message(IpcMessage::JobResume(JobControl {
        job_id,
        reason: Some("user_resumed".into()),
    }))?;
    expect_job_status(response)
}

#[tauri::command]
async fn cmd_retry_job(job_id: String) -> Result<JobStatus, String> {
    log::info!("Retrying job: {}", job_id);
    let response = send_orchestrator_message(IpcMessage::JobRetry(JobControl {
        job_id,
        reason: Some("user_retried".into()),
    }))?;
    expect_job_status(response)
}

#[tauri::command]
async fn cmd_skip_job(job_id: String) -> Result<JobStatus, String> {
    log::info!("Skipping job: {}", job_id);
    let response = send_orchestrator_message(IpcMessage::JobSkip(JobControl {
        job_id,
        reason: Some("user_skipped".into()),
    }))?;
    expect_job_status(response)
}

#[tauri::command]
async fn cmd_get_transcript(job_id: String) -> Result<TranscriptResponse, String> {
    log::info!("Loading transcript for job {}", job_id);
    let db_path = storage_db_path()?;
    let storage = audraflow_storage::Storage::open(&db_path).map_err(|e| e.to_string())?;
    let job = storage
        .get_job(&job_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("No job found for id {job_id}"))?;
    let segments = storage.get_segments(&job_id).map_err(|e| e.to_string())?;
    if segments.is_empty() {
        return Err("No transcript segments found for this job yet".into());
    }

    let segments = segments
        .into_iter()
        .map(|segment| segment_to_dto(&storage, segment))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(TranscriptResponse {
        job_id: job.job_id,
        media_src_path: job.file_path.clone(),
        file_path: job.file_path,
        segments,
    })
}

#[tauri::command]
async fn cmd_search_transcript(
    job_id: String,
    query: String,
) -> Result<Vec<TranscriptSegmentDto>, String> {
    let job_id = job_id.trim().to_string();
    if job_id.is_empty() {
        return Err("jobId is required".into());
    }
    let db_path = storage_db_path()?;
    let storage = audraflow_storage::Storage::open(&db_path).map_err(|e| e.to_string())?;
    search_transcript_segments(&storage, &job_id, &query)
}

#[tauri::command]
async fn cmd_update_segment(
    app_handle: tauri::AppHandle,
    request: UpdateSegmentRequest,
) -> Result<TranscriptSegmentDto, String> {
    let segment_id = request.segment_id.trim().to_string();
    if segment_id.is_empty() {
        return Err("segmentId is required".into());
    }

    let text = request.text.map(|value| value.trim().to_string());
    let speaker = request.speaker.map(|value| value.trim().to_string());
    if text.is_none() && speaker.is_none() {
        return Err("No segment changes were provided".into());
    }

    let db_path = storage_db_path()?;
    let storage = audraflow_storage::Storage::open(&db_path).map_err(|e| e.to_string())?;
    let before = storage
        .get_segment(&segment_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("No segment found for id {segment_id}"))?;

    if let Some(new_text) = text.as_deref() {
        if new_text != before.text {
            storage
                .record_correction(
                    &segment_id,
                    "text",
                    &before.text,
                    new_text,
                    &CorrectionSource::User,
                    false,
                )
                .map_err(|e| e.to_string())?;
            record_local_telemetry(
                &app_handle,
                TelemetryEventRequest {
                    event_type: "correction_event".into(),
                    job_id: None,
                    segment_id: Some(segment_id.clone()),
                    audio_hours: None,
                    transcript_chars: None,
                    active_seconds: None,
                    inactive_seconds: None,
                    completed_ratio: None,
                    op_type: Some(correction_op_type(&before.text, new_text).into()),
                    chars_before: Some(before.text.chars().count() as u32),
                    chars_after: Some(new_text.chars().count() as u32),
                    source: Some("user".into()),
                    from_ms: None,
                    to_ms: None,
                    trigger: None,
                    mark_ms: None,
                    label_type: None,
                    format: None,
                    include_timestamps: None,
                    include_speakers: None,
                    include_marks: None,
                },
            )
            .ok();
        }
    }

    if let Some(new_speaker) = speaker.as_deref() {
        let old_speaker = before.speaker_id.as_deref().unwrap_or("");
        if new_speaker != old_speaker {
            storage
                .record_correction(
                    &segment_id,
                    "speaker_id",
                    old_speaker,
                    new_speaker,
                    &CorrectionSource::User,
                    false,
                )
                .map_err(|e| e.to_string())?;
            record_local_telemetry(
                &app_handle,
                TelemetryEventRequest {
                    event_type: "correction_event".into(),
                    job_id: None,
                    segment_id: Some(segment_id.clone()),
                    audio_hours: None,
                    transcript_chars: None,
                    active_seconds: None,
                    inactive_seconds: None,
                    completed_ratio: None,
                    op_type: Some("replace".into()),
                    chars_before: Some(0),
                    chars_after: Some(0),
                    source: Some("user".into()),
                    from_ms: None,
                    to_ms: None,
                    trigger: None,
                    mark_ms: None,
                    label_type: None,
                    format: None,
                    include_timestamps: None,
                    include_speakers: None,
                    include_marks: None,
                },
            )
            .ok();
        }
    }

    storage
        .update_segment(&segment_id, text.as_deref(), speaker.as_deref())
        .map_err(|e| e.to_string())?;
    let updated = storage
        .get_segment(&segment_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("No segment found for id {segment_id} after update"))?;
    segment_to_dto(&storage, updated)
}

#[tauri::command]
async fn cmd_accept_term_candidate(
    request: AcceptTermCandidateRequest,
) -> Result<TranscriptSegmentDto, String> {
    let segment_id = request.segment_id.trim().to_string();
    if segment_id.is_empty() {
        return Err("segmentId is required".into());
    }

    let db_path = storage_db_path()?;
    let storage = audraflow_storage::Storage::open(&db_path).map_err(|e| e.to_string())?;
    let segment = storage
        .get_segment(&segment_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("No segment found for id {segment_id}"))?;

    if segment.text != segment.raw_text
        && !storage
            .segment_has_corrections(&segment_id)
            .map_err(|e| e.to_string())?
    {
        storage
            .record_correction(
                &segment_id,
                "text",
                &segment.raw_text,
                &segment.text,
                &CorrectionSource::Lexicon,
                false,
            )
            .map_err(|e| e.to_string())?;
    }
    storage
        .remove_low_confidence_reason(&segment_id, "term_conflict")
        .map_err(|e| e.to_string())?;

    let updated = storage
        .get_segment(&segment_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("No segment found for id {segment_id} after update"))?;
    segment_to_dto(&storage, updated)
}

#[tauri::command]
async fn cmd_add_glossary_entry(
    request: AddGlossaryEntryRequest,
) -> Result<GlossaryApplyResult, String> {
    let canonical = request.canonical.trim().to_string();
    if canonical.is_empty() {
        return Err("canonical is required".into());
    }

    let aliases = sanitize_glossary_aliases(&canonical, request.aliases);
    if aliases.is_empty() {
        return Err("At least one alias different from the canonical term is required".into());
    }

    let category = request
        .category
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let db_path = storage_db_path()?;
    let storage = audraflow_storage::Storage::open(&db_path).map_err(|e| e.to_string())?;
    let entry = storage
        .upsert_glossary_entry(&canonical, &aliases, category)
        .map_err(|e| e.to_string())?;

    let updated_count = if let Some(job_id) = request
        .job_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        apply_glossary_entry_to_job(&storage, job_id, &entry)?
    } else {
        0
    };

    let updated_segments = if let Some(job_id) = request
        .job_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        storage
            .get_segments(job_id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|segment| segment_to_dto(&storage, segment))
            .collect::<Result<Vec<_>, _>>()?
    } else {
        Vec::new()
    };

    Ok(GlossaryApplyResult {
        entry: glossary_entry_to_dto(entry),
        updated_segments,
        updated_count,
    })
}

#[tauri::command]
async fn cmd_save_glossary_entry(
    request: SaveGlossaryEntryRequest,
) -> Result<Vec<GlossaryEntryDto>, String> {
    let canonical = request.canonical.trim().to_string();
    if canonical.is_empty() {
        return Err("canonical is required".into());
    }

    let aliases = sanitize_glossary_aliases(&canonical, request.aliases);
    let category = request
        .category
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let db_path = storage_db_path()?;
    let storage = audraflow_storage::Storage::open(&db_path).map_err(|e| e.to_string())?;

    if let Some(id) = request.id {
        if id <= 0 {
            return Err("Glossary entry id must be positive".into());
        }
        storage
            .replace_glossary_entry(id, &canonical, &aliases, category)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Glossary entry was not found".to_string())?;
    } else {
        storage
            .upsert_glossary_entry(&canonical, &aliases, category)
            .map_err(|e| e.to_string())?;
    }

    Ok(storage
        .list_glossary_entries()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(glossary_entry_to_dto)
        .collect::<Vec<_>>())
}

#[tauri::command]
async fn cmd_list_glossary_entries() -> Result<Vec<GlossaryEntryDto>, String> {
    let db_path = storage_db_path()?;
    let storage = audraflow_storage::Storage::open(&db_path).map_err(|e| e.to_string())?;
    Ok(storage
        .list_glossary_entries()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(glossary_entry_to_dto)
        .collect::<Vec<_>>())
}

#[tauri::command]
async fn cmd_delete_glossary_entry(id: i64) -> Result<Vec<GlossaryEntryDto>, String> {
    if id <= 0 {
        return Err("Glossary entry id must be positive".into());
    }

    let db_path = storage_db_path()?;
    let storage = audraflow_storage::Storage::open(&db_path).map_err(|e| e.to_string())?;
    if !storage
        .disable_glossary_entry(id)
        .map_err(|e| e.to_string())?
    {
        return Err("Glossary entry was not found or already deleted".into());
    }

    Ok(storage
        .list_glossary_entries()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(glossary_entry_to_dto)
        .collect::<Vec<_>>())
}

#[tauri::command]
async fn cmd_update_speaker_label(
    app_handle: tauri::AppHandle,
    request: UpdateSpeakerLabelRequest,
) -> Result<Vec<TranscriptSegmentDto>, String> {
    let job_id = request.job_id.trim().to_string();
    let from_speaker = request.from_speaker.trim().to_string();
    let to_speaker = request.to_speaker.trim().to_string();
    if job_id.is_empty() {
        return Err("jobId is required".into());
    }
    if from_speaker.is_empty() || to_speaker.is_empty() {
        return Err("Speaker labels cannot be empty".into());
    }
    if from_speaker == to_speaker {
        return Err("Speaker labels are unchanged".into());
    }

    let db_path = storage_db_path()?;
    let storage = audraflow_storage::Storage::open(&db_path).map_err(|e| e.to_string())?;
    let segments = storage.get_segments(&job_id).map_err(|e| e.to_string())?;
    let matching_segments = segments
        .iter()
        .filter(|segment| segment.speaker_id.as_deref().unwrap_or("Speaker") == from_speaker)
        .collect::<Vec<_>>();

    if matching_segments.is_empty() {
        return Err(format!("No segments found for speaker {from_speaker}"));
    }

    for segment in matching_segments {
        storage
            .record_correction(
                &segment.segment_id,
                "speaker_id",
                segment.speaker_id.as_deref().unwrap_or(""),
                &to_speaker,
                &CorrectionSource::User,
                false,
            )
            .map_err(|e| e.to_string())?;
        record_local_telemetry(
            &app_handle,
            TelemetryEventRequest {
                event_type: "correction_event".into(),
                job_id: Some(job_id.clone()),
                segment_id: Some(segment.segment_id.clone()),
                audio_hours: None,
                transcript_chars: None,
                active_seconds: None,
                inactive_seconds: None,
                completed_ratio: None,
                op_type: Some("replace".into()),
                chars_before: Some(0),
                chars_after: Some(0),
                source: Some("merge".into()),
                from_ms: None,
                to_ms: None,
                trigger: None,
                mark_ms: None,
                label_type: None,
                format: None,
                include_timestamps: None,
                include_speakers: None,
                include_marks: None,
            },
        )
        .ok();
    }
    storage
        .update_speaker_label_for_job(&job_id, &from_speaker, &to_speaker)
        .map_err(|e| e.to_string())?;

    storage
        .get_segments(&job_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|segment| segment_to_dto(&storage, segment))
        .collect()
}

#[tauri::command]
async fn cmd_add_timestamp_mark(
    app_handle: tauri::AppHandle,
    request: AddTimestampMarkRequest,
) -> Result<TranscriptSegmentDto, String> {
    let segment_id = request.segment_id.trim().to_string();
    if segment_id.is_empty() {
        return Err("segmentId is required".into());
    }
    if request.mark_ms < 0 {
        return Err("markMs must be zero or greater".into());
    }

    let label = request
        .label
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let note = request
        .note
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let db_path = storage_db_path()?;
    let storage = audraflow_storage::Storage::open(&db_path).map_err(|e| e.to_string())?;
    let segment = storage
        .get_segment(&segment_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("No segment found for id {segment_id}"))?;
    storage
        .add_mark(&segment_id, request.mark_ms, label, note)
        .map_err(|e| e.to_string())?;
    record_local_telemetry(
        &app_handle,
        TelemetryEventRequest {
            event_type: "timestamp_mark".into(),
            job_id: None,
            segment_id: Some(segment_id.clone()),
            audio_hours: None,
            transcript_chars: None,
            active_seconds: None,
            inactive_seconds: None,
            completed_ratio: None,
            op_type: None,
            chars_before: None,
            chars_after: None,
            source: None,
            from_ms: None,
            to_ms: None,
            trigger: None,
            mark_ms: Some(request.mark_ms),
            label_type: Some(if label.is_some() { "custom" } else { "none" }.into()),
            format: None,
            include_timestamps: None,
            include_speakers: None,
            include_marks: None,
        },
    )
    .ok();
    segment_to_dto(&storage, segment)
}

#[tauri::command]
async fn cmd_record_telemetry_event(
    app_handle: tauri::AppHandle,
    request: TelemetryEventRequest,
) -> Result<(), String> {
    record_local_telemetry(&app_handle, request)
}

#[tauri::command]
async fn cmd_get_telemetry_consent(
    app_handle: tauri::AppHandle,
) -> Result<TelemetryConsentState, String> {
    read_telemetry_consent(&app_handle)
}

#[tauri::command]
async fn cmd_set_telemetry_consent(
    app_handle: tauri::AppHandle,
    request: SetTelemetryConsentRequest,
) -> Result<TelemetryConsentState, String> {
    write_telemetry_consent(&app_handle, request.enabled)
}

#[tauri::command]
async fn cmd_clear_local_history(
    app_handle: tauri::AppHandle,
) -> Result<PrivacyActionResult, String> {
    let db_path = storage_db_path()?;
    let telemetry_path = telemetry_events_path(&app_handle)?;
    let before = file_size_or_zero(&db_path) + file_size_or_zero(&telemetry_path);
    let storage = audraflow_storage::Storage::open(&db_path).map_err(|e| e.to_string())?;
    storage.clear_job_history().map_err(|e| e.to_string())?;
    if telemetry_path.exists() {
        std::fs::remove_file(&telemetry_path).map_err(|e| e.to_string())?;
    }
    let after = file_size_or_zero(&db_path);

    Ok(PrivacyActionResult {
        message: "Local job history and telemetry events were cleared.".into(),
        bytes_freed: before.saturating_sub(after),
        items_affected: 1,
    })
}

#[tauri::command]
async fn cmd_delete_model_cache(
    app_handle: tauri::AppHandle,
) -> Result<PrivacyActionResult, String> {
    let model_dir = model_cache_dir(&app_handle)?;
    let before = directory_size_bytes(&model_dir)?;
    let removed = clear_directory_children(&model_dir)?;

    Ok(PrivacyActionResult {
        message: "Model cache was cleared.".into(),
        bytes_freed: before,
        items_affected: removed,
    })
}

#[tauri::command]
async fn cmd_get_model_settings(app_handle: tauri::AppHandle) -> Result<ModelSettingsDto, String> {
    model_settings(&app_handle)
}

#[tauri::command]
async fn cmd_get_model_catalog(
    app_handle: tauri::AppHandle,
) -> Result<Vec<ModelCatalogEntryDto>, String> {
    builtin_model_catalog(&app_handle)
}

#[tauri::command]
async fn cmd_import_local_model(
    app_handle: tauri::AppHandle,
    request: ImportLocalModelRequest,
) -> Result<ModelSettingsDto, String> {
    import_local_model(&app_handle, request)
}

#[tauri::command]
async fn cmd_download_model(
    app_handle: tauri::AppHandle,
    request: DownloadModelRequest,
) -> Result<ModelActionResult, String> {
    download_model(app_handle, request).await
}

#[tauri::command]
async fn cmd_select_model(
    app_handle: tauri::AppHandle,
    request: SelectModelRequest,
) -> Result<ModelSettingsDto, String> {
    let manager = model_manager(&app_handle)?;
    manager
        .select_model(&request.name, &request.version)
        .map_err(|e| e.to_string())?;
    model_settings(&app_handle)
}

#[tauri::command]
async fn cmd_delete_model(
    app_handle: tauri::AppHandle,
    request: DeleteModelRequest,
) -> Result<ModelActionResult, String> {
    let manager = model_manager(&app_handle)?;
    let installed = manager
        .list_installed_models()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|model| model.info.name == request.name && model.info.version == request.version)
        .ok_or_else(|| {
            format!(
                "Model is not installed: {} v{}",
                request.name, request.version
            )
        })?;
    let bytes_freed = directory_size_bytes(installed.path.parent().unwrap_or(&installed.path))
        .unwrap_or_else(|_| file_size_or_zero(&installed.path));

    manager
        .remove_model(&request.name, &request.version)
        .map_err(|e| e.to_string())?;

    Ok(ModelActionResult {
        message: format!("Deleted {} v{}.", request.name, request.version),
        bytes_freed,
        items_affected: 1,
        settings: model_settings(&app_handle)?,
    })
}

#[tauri::command]
async fn cmd_clear_unused_models(
    app_handle: tauri::AppHandle,
) -> Result<ModelActionResult, String> {
    let manager = model_manager(&app_handle)?;
    let selected = manager.selected_model().map_err(|e| e.to_string())?;
    let installed = manager.list_installed_models().map_err(|e| e.to_string())?;
    let mut bytes_freed = 0u64;
    let mut removed = 0u64;

    for model in installed {
        let is_selected = selected.as_ref().is_some_and(|selected| {
            selected.info.name == model.info.name && selected.info.version == model.info.version
        });
        if is_selected {
            continue;
        }

        bytes_freed = bytes_freed.saturating_add(
            directory_size_bytes(model.path.parent().unwrap_or(&model.path))
                .unwrap_or_else(|_| file_size_or_zero(&model.path)),
        );
        manager
            .remove_model(&model.info.name, &model.info.version)
            .map_err(|e| e.to_string())?;
        removed += 1;
    }

    Ok(ModelActionResult {
        message: if removed == 0 {
            "No unused models to clear.".into()
        } else {
            format!("Cleared {removed} unused model(s).")
        },
        bytes_freed,
        items_affected: removed,
        settings: model_settings(&app_handle)?,
    })
}

#[tauri::command]
async fn cmd_get_diagnostics_preview(
    app_handle: tauri::AppHandle,
) -> Result<DiagnosticsPreview, String> {
    diagnostics_preview(&app_handle)
}

#[tauri::command]
async fn cmd_get_device_diagnostics() -> Result<DeviceDiagnosticsDto, String> {
    Ok(detect_device_diagnostics())
}

#[tauri::command]
async fn cmd_get_runtime_health(app_handle: tauri::AppHandle) -> Result<RuntimeHealthDto, String> {
    Ok(runtime_health(&app_handle).await)
}

#[tauri::command]
async fn cmd_repair_runtime_dependency(
    app_handle: tauri::AppHandle,
    id: String,
) -> Result<RuntimeRepairResultDto, String> {
    repair_runtime_dependency(app_handle, &id).await
}

#[tauri::command]
async fn cmd_export_diagnostics_package(app_handle: tauri::AppHandle) -> Result<String, String> {
    let preview = diagnostics_preview(&app_handle)?;
    let payload = serde_json::json!({
        "app_version": "0.1.0-alpha",
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "telemetry_enabled": preview.telemetry_enabled,
        "local_history_bytes": preview.local_history_bytes,
        "telemetry_events_bytes": preview.telemetry_events_bytes,
        "model_cache_bytes": preview.model_cache_bytes,
        "model_cache_items": preview.model_cache_items,
        "fields": preview.fields,
        "generated_at_ms": now_unix_ms(),
    });
    let output_dir = app_handle
        .path()
        .download_dir()
        .or_else(|_| app_handle.path().app_data_dir())
        .map_err(|e| e.to_string())?
        .join("AudraFlow");
    std::fs::create_dir_all(&output_dir).map_err(|e| e.to_string())?;
    let output_path = output_dir.join(format!("diagnostics-{}.json", now_unix_ms()));
    let json = serde_json::to_vec_pretty(&payload).map_err(|e| e.to_string())?;
    std::fs::write(&output_path, json).map_err(|e| e.to_string())?;
    Ok(output_path.to_string_lossy().into_owned())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn cmd_export_transcript(
    app_handle: tauri::AppHandle,
    job_id: String,
    format: String,
    include_speakers: bool,
    include_timestamps: bool,
    include_marks: bool,
    speaker_filter: Option<String>,
    output_path: Option<String>,
) -> Result<String, String> {
    log::info!("Exporting job {} as {}", job_id, format);
    let db_path = storage_db_path()?;
    let storage = audraflow_storage::Storage::open(&db_path).map_err(|e| e.to_string())?;
    let segments = storage.get_segments(&job_id).map_err(|e| e.to_string())?;
    if segments.is_empty() {
        return Err("No transcript segments found for this job".into());
    }

    let normalized = format.trim().to_ascii_lowercase();
    let extension = match normalized.as_str() {
        "txt" | "markdown" | "md" | "srt" | "vtt" | "json" | "docx" => normalized.as_str(),
        other => return Err(format!("Unsupported export format: {other}")),
    };
    let extension = if extension == "markdown" {
        "md"
    } else {
        extension
    };

    let output_path = resolve_export_output_path(&app_handle, &job_id, extension, output_path)?;
    let speaker_filter = normalize_speaker_filter(speaker_filter.as_deref(), include_speakers)?;
    if extension == "docx" {
        let title = export_title_for_job(&storage, &job_id)?;
        let export_segments = storage_segments_to_ipc_segments(&storage, &segments, include_marks)?;
        let content = audraflow_export::export_docx(
            &export_segments,
            &title,
            &audraflow_export::ExportOptions {
                include_timestamps,
                include_speakers: speaker_filter.includes_any_speaker(),
                include_marks,
                speaker_filter: speaker_filter.to_export_filter(),
            },
        )
        .map_err(|e| e.to_string())?;
        std::fs::write(&output_path, content).map_err(|e| e.to_string())?;
    } else {
        let content = render_export(
            &storage,
            &job_id,
            &segments,
            extension,
            speaker_filter,
            include_timestamps,
            include_marks,
        )?;
        std::fs::write(&output_path, content).map_err(|e| e.to_string())?;
    }
    record_local_telemetry(
        &app_handle,
        TelemetryEventRequest {
            event_type: "export_completed".into(),
            job_id: Some(job_id.clone()),
            segment_id: None,
            audio_hours: None,
            transcript_chars: None,
            active_seconds: None,
            inactive_seconds: None,
            completed_ratio: None,
            op_type: None,
            chars_before: None,
            chars_after: None,
            source: None,
            from_ms: None,
            to_ms: None,
            trigger: None,
            mark_ms: None,
            label_type: None,
            format: Some(extension.to_string()),
            include_timestamps: Some(include_timestamps),
            include_speakers: Some(speaker_filter.includes_any_speaker()),
            include_marks: Some(include_marks),
        },
    )
    .ok();
    Ok(output_path.to_string_lossy().into_owned())
}

#[tauri::command]
async fn cmd_render_transcript_export(
    job_id: String,
    format: String,
    include_speakers: bool,
    include_timestamps: bool,
    include_marks: bool,
    speaker_filter: Option<String>,
) -> Result<String, String> {
    let db_path = storage_db_path()?;
    let storage = audraflow_storage::Storage::open(&db_path).map_err(|e| e.to_string())?;
    let segments = storage.get_segments(&job_id).map_err(|e| e.to_string())?;
    if segments.is_empty() {
        return Err("No transcript segments found for this job".into());
    }

    let normalized = normalize_text_export_format(&format)?;
    if normalized == "docx" {
        return Err("DOCX is a file-only export format".into());
    }

    render_export(
        &storage,
        &job_id,
        &segments,
        &normalized,
        normalize_speaker_filter(speaker_filter.as_deref(), include_speakers)?,
        include_timestamps,
        include_marks,
    )
}

fn normalize_text_export_format(format: &str) -> Result<String, String> {
    let normalized = format.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "txt" | "text" | "plain" => Ok("txt".into()),
        "markdown" | "md" => Ok("md".into()),
        "srt" | "vtt" | "json" | "docx" => Ok(normalized),
        "obsidian" | "clipboardobsidian" | "clipboard-obsidian" | "clipboard_obsidian" => {
            Ok("obsidian".into())
        }
        "notion" | "clipboardnotion" | "clipboard-notion" | "clipboard_notion" => {
            Ok("notion".into())
        }
        other => Err(format!("Unsupported export format: {other}")),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SpeakerExportFilter {
    All,
    NamedOnly,
    Hidden,
}

impl SpeakerExportFilter {
    fn includes_any_speaker(self) -> bool {
        !matches!(self, Self::Hidden)
    }

    fn to_export_filter(self) -> audraflow_export::SpeakerFilter {
        match self {
            Self::All => audraflow_export::SpeakerFilter::All,
            Self::NamedOnly => audraflow_export::SpeakerFilter::NamedOnly,
            Self::Hidden => audraflow_export::SpeakerFilter::Hidden,
        }
    }
}

fn normalize_speaker_filter(
    speaker_filter: Option<&str>,
    include_speakers: bool,
) -> Result<SpeakerExportFilter, String> {
    if !include_speakers {
        return Ok(SpeakerExportFilter::Hidden);
    }

    match speaker_filter
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("all")
        .to_ascii_lowercase()
        .as_str()
    {
        "all" => Ok(SpeakerExportFilter::All),
        "namedonly" | "named_only" | "named-only" => Ok(SpeakerExportFilter::NamedOnly),
        "hidden" | "hide" | "none" => Ok(SpeakerExportFilter::Hidden),
        other => Err(format!("Unsupported speaker filter: {other}")),
    }
}

fn should_render_speaker(speaker: Option<&str>, filter: SpeakerExportFilter) -> bool {
    match filter {
        SpeakerExportFilter::Hidden => false,
        SpeakerExportFilter::All => speaker.is_some_and(|value| !value.trim().is_empty()),
        SpeakerExportFilter::NamedOnly => speaker.is_some_and(is_named_speaker),
    }
}

fn is_named_speaker(speaker: &str) -> bool {
    let normalized = speaker.trim().to_ascii_lowercase();
    !normalized.is_empty()
        && normalized != "speaker"
        && !normalized.strip_prefix("speaker ").is_some_and(|suffix| {
            suffix.len() == 1 && suffix.chars().all(|ch| ch.is_ascii_alphabetic())
        })
}

fn export_title_for_job(
    storage: &audraflow_storage::Storage,
    job_id: &str,
) -> Result<String, String> {
    let Some(job) = storage.get_job(job_id).map_err(|e| e.to_string())? else {
        return Ok(format!("Transcript {job_id}"));
    };

    Ok(Path::new(&job.file_path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("Transcript {job_id}")))
}

fn default_export_output_path(
    app_handle: &tauri::AppHandle,
    job_id: &str,
    extension: &str,
) -> Result<PathBuf, String> {
    let output_dir = app_handle
        .path()
        .download_dir()
        .or_else(|_| app_handle.path().app_data_dir())
        .map_err(|e| e.to_string())?
        .join("AudraFlow");
    Ok(output_dir.join(format!("transcript-{job_id}.{extension}")))
}

fn resolve_export_output_path(
    app_handle: &tauri::AppHandle,
    job_id: &str,
    extension: &str,
    output_path: Option<String>,
) -> Result<PathBuf, String> {
    let path = output_path
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or(default_export_output_path(app_handle, job_id, extension)?);

    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    Ok(path)
}

fn storage_segments_to_ipc_segments(
    storage: &audraflow_storage::Storage,
    segments: &[SegmentRow],
    include_marks: bool,
) -> Result<Vec<Segment>, String> {
    segments
        .iter()
        .map(|segment| storage_segment_to_ipc_segment(storage, segment, include_marks))
        .collect()
}

fn storage_segment_to_ipc_segment(
    storage: &audraflow_storage::Storage,
    segment: &SegmentRow,
    include_marks: bool,
) -> Result<Segment, String> {
    let low_confidence_reasons = storage
        .get_low_confidence_reasons(&segment.segment_id)
        .map_err(|e| e.to_string())?;
    let corrections = storage
        .get_corrections(&segment.segment_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|correction| Correction {
            field: correction.field,
            old_value: correction.old_value,
            new_value: correction.new_value,
            source: correction_source_from_storage(&correction.source),
            auto_applied: correction.auto_applied,
        })
        .collect();
    let marks = if include_marks {
        storage
            .get_marks(&segment.segment_id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|mark| TimestampMark {
                mark_ms: mark.mark_ms,
                label: mark.label,
                note: mark.note,
            })
            .collect()
    } else {
        Vec::new()
    };

    Ok(Segment {
        segment_id: segment.segment_id.clone(),
        start_ms: segment.start_ms,
        end_ms: segment.end_ms,
        speaker_id: segment.speaker_id.clone(),
        text: segment.text.clone(),
        raw_text: segment.raw_text.clone(),
        confidence: segment.confidence,
        low_confidence_reasons,
        corrections,
        marks,
    })
}

fn correction_source_from_storage(source: &str) -> CorrectionSource {
    match source.trim().to_ascii_lowercase().as_str() {
        "lexicon" => CorrectionSource::Lexicon,
        "merge" => CorrectionSource::Merge,
        _ => CorrectionSource::User,
    }
}

fn render_export(
    storage: &audraflow_storage::Storage,
    job_id: &str,
    segments: &[SegmentRow],
    format: &str,
    speaker_filter: SpeakerExportFilter,
    include_timestamps: bool,
    include_marks: bool,
) -> Result<String, String> {
    match format {
        "txt" => Ok(segments
            .iter()
            .map(|segment| render_plain_segment(segment, speaker_filter, include_timestamps))
            .collect::<Vec<_>>()
            .join("\n")),
        "md" => Ok(segments
            .iter()
            .map(|segment| {
                render_markdown_segment(
                    storage,
                    segment,
                    speaker_filter,
                    include_timestamps,
                    include_marks,
                )
            })
            .collect::<Result<Vec<_>, _>>()?
            .join("\n\n")),
        "srt" => Ok(segments
            .iter()
            .enumerate()
            .map(|(index, segment)| {
                format!(
                    "{}\n{} --> {}\n{}\n",
                    index + 1,
                    format_srt_time(segment.start_ms),
                    format_srt_time(segment.end_ms),
                    render_segment_text(segment, speaker_filter)
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")),
        "vtt" => Ok(format!(
            "WEBVTT\n\n{}",
            segments
                .iter()
                .map(|segment| {
                    format!(
                        "{} --> {}\n{}\n",
                        format_vtt_time(segment.start_ms),
                        format_vtt_time(segment.end_ms),
                        render_segment_text(segment, speaker_filter)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        )),
        "json" => {
            let export_segments =
                storage_segments_to_ipc_segments(storage, segments, include_marks)?;
            let title = export_title_for_job(storage, job_id)?;
            let source_hash = storage
                .get_job(job_id)
                .map_err(|e| e.to_string())?
                .map(|job| job.file_hash)
                .unwrap_or_default();
            let export = audraflow_export::TranscriptExport::new(
                job_id.to_string(),
                source_hash,
                title,
                export_segments,
                audraflow_export::ExportOptions {
                    include_timestamps,
                    include_speakers: speaker_filter.includes_any_speaker(),
                    include_marks,
                    speaker_filter: speaker_filter.to_export_filter(),
                },
            );
            Ok(audraflow_export::export_transcript_json(&export))
        }
        "obsidian" => {
            let export_segments =
                storage_segments_to_ipc_segments(storage, segments, include_marks)?
                    .into_iter()
                    .map(|segment| apply_speaker_filter_to_segment(segment, speaker_filter))
                    .collect::<Vec<_>>();
            Ok(audraflow_export::export_obsidian_callout(&export_segments))
        }
        "notion" => {
            let export_segments =
                storage_segments_to_ipc_segments(storage, segments, include_marks)?
                    .into_iter()
                    .map(|segment| apply_speaker_filter_to_segment(segment, speaker_filter))
                    .collect::<Vec<_>>();
            Ok(audraflow_export::export_notion_toggle(&export_segments))
        }
        _ => Err(format!("Unsupported export format: {format}")),
    }
}

fn apply_speaker_filter_to_segment(
    mut segment: Segment,
    speaker_filter: SpeakerExportFilter,
) -> Segment {
    if !should_render_speaker(segment.speaker_id.as_deref(), speaker_filter) {
        segment.speaker_id = None;
    }
    segment
}

fn render_markdown_segment(
    storage: &audraflow_storage::Storage,
    segment: &SegmentRow,
    speaker_filter: SpeakerExportFilter,
    include_timestamps: bool,
    include_marks: bool,
) -> Result<String, String> {
    let mut text = render_plain_segment(segment, speaker_filter, include_timestamps);
    if include_marks {
        let marks = storage
            .get_marks(&segment.segment_id)
            .map_err(|e| e.to_string())?;
        for mark in marks {
            let label = mark.label.as_deref().unwrap_or("Mark");
            let note = mark.note.as_deref().unwrap_or("");
            if note.is_empty() {
                text.push_str(&format!(
                    "\n- [{}] {}",
                    format_clock_time(mark.mark_ms),
                    label
                ));
            } else {
                text.push_str(&format!(
                    "\n- [{}] {}: {}",
                    format_clock_time(mark.mark_ms),
                    label,
                    note
                ));
            }
        }
    }
    Ok(text)
}

fn render_plain_segment(
    segment: &SegmentRow,
    speaker_filter: SpeakerExportFilter,
    include_timestamps: bool,
) -> String {
    let mut parts = Vec::new();
    if include_timestamps {
        parts.push(format!("[{}]", format_clock_time(segment.start_ms)));
    }
    if should_render_speaker(segment.speaker_id.as_deref(), speaker_filter) {
        if let Some(speaker) = &segment.speaker_id {
            parts.push(format!("{speaker}:"));
        }
    }
    parts.push(segment.text.clone());
    parts.join(" ")
}

fn render_segment_text(segment: &SegmentRow, speaker_filter: SpeakerExportFilter) -> String {
    if should_render_speaker(segment.speaker_id.as_deref(), speaker_filter) {
        if let Some(speaker) = &segment.speaker_id {
            return format!("{speaker}: {}", segment.text);
        }
    }
    segment.text.clone()
}

fn format_clock_time(ms: i64) -> String {
    let total_seconds = ms.max(0) / 1000;
    let h = total_seconds / 3600;
    let m = (total_seconds % 3600) / 60;
    let s = total_seconds % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

fn format_srt_time(ms: i64) -> String {
    let total_ms = ms.max(0);
    let h = total_ms / 3_600_000;
    let m = (total_ms % 3_600_000) / 60_000;
    let s = (total_ms % 60_000) / 1000;
    let milli = total_ms % 1000;
    format!("{h:02}:{m:02}:{s:02},{milli:03}")
}

fn format_vtt_time(ms: i64) -> String {
    format_srt_time(ms).replace(',', ".")
}

/// Estimate processing time using the adaptive scheduler.
/// Called when user toggles "极致准确" to show estimated duration change.
///
/// Input: audio duration (seconds), extreme accuracy flag.
/// Output: scheduler plan with estimated seconds.
#[tauri::command]
async fn cmd_estimate_job(
    audio_duration_s: f64,
    extreme_accuracy: bool,
) -> Result<JobPlan, String> {
    let diagnostics = detect_device_diagnostics();
    let device_tier = DeviceTier::classify(
        diagnostics.cuda_available,
        diagnostics.vram_gb,
        diagnostics.cpu_cores,
    );
    let input = SchedulerInput {
        duration_seconds: audio_duration_s,
        snr_db: None,
        speech_density: None,
        estimated_speaker_count: 1,
        is_high_noise: false,
        device_tier,
        cuda_available: diagnostics.cuda_available,
        vram_gb: diagnostics.vram_gb,
        cpu_cores: diagnostics.cpu_cores,
        extreme_accuracy,
        model_cached: true,
        cold_start_seconds: None,
    };

    let plan = Scheduler::plan(&input);
    log::info!(
        "Estimate: extreme={}, duration={:.0}s → est={:.0}s, model={:?}",
        extreme_accuracy,
        audio_duration_s,
        plan.estimated_duration_seconds,
        plan.model_size,
    );

    Ok(JobPlan {
        job_id: String::new(), // No job created yet
        plan_id: plan.plan_id,
        model_size: format!("{:?}", plan.model_size),
        estimated_seconds: plan.estimated_duration_seconds,
        explanation: plan.explanation,
        fallback_reason: plan.fallback_reason,
    })
}

/// Activate a license key.
#[tauri::command]
async fn cmd_activate_license(
    app_handle: tauri::AppHandle,
    license_key: String,
) -> Result<String, String> {
    let app_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?;

    let mut manager = LicenseManager::new(app_dir).map_err(|e| e.to_string())?;
    manager.activate(&license_key).map_err(|e| e.to_string())?;

    Ok("License activated successfully".into())
}

/// Get current license state (trial days remaining, activation status).
#[tauri::command]
async fn cmd_get_license_state(app_handle: tauri::AppHandle) -> Result<serde_json::Value, String> {
    use LicenseState::*;

    let app_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?;

    let manager = LicenseManager::new(app_dir).map_err(|e| e.to_string())?;
    let state = manager.state();

    let json = match state {
        NotActivated => serde_json::json!({
            "state": "not_activated",
            "is_usable": false,
            "trial_days_remaining": 0
        }),
        Trial {
            days_remaining,
            expires_at,
            ..
        } => serde_json::json!({
            "state": "trial",
            "is_usable": true,
            "trial_days_remaining": days_remaining,
            "expires_at": expires_at
        }),
        Activated {
            model_updates_until,
            ..
        } => serde_json::json!({
            "state": "activated",
            "is_usable": true,
            "trial_days_remaining": 0,
            "model_updates_until": model_updates_until
        }),
        TrialExpired { .. } => serde_json::json!({
            "state": "trial_expired",
            "is_usable": false,
            "trial_days_remaining": 0
        }),
        Invalid(reason) => serde_json::json!({
            "state": "invalid",
            "is_usable": false,
            "trial_days_remaining": 0,
            "reason": reason
        }),
    };

    Ok(json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_media_folder_filters_supported_files_recursively() {
        let root = std::env::temp_dir().join(format!("audraflow-scan-{}", uuid::Uuid::new_v4()));
        let nested = root.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join("audio.mp3"), b"stub").unwrap();
        std::fs::write(root.join("notes.txt"), b"ignore").unwrap();
        std::fs::write(nested.join("video.mp4"), b"stub").unwrap();

        let files = scan_media_folder(&root).unwrap();

        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|path| path.ends_with("audio.mp3")));
        assert!(files.iter().any(|path| path.ends_with("video.mp4")));
        assert!(!files.iter().any(|path| path.ends_with("notes.txt")));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn inspect_media_file_reports_basic_metadata() {
        let root = std::env::temp_dir().join(format!("audraflow-inspect-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("sample.mp3");
        std::fs::write(&path, b"stub").unwrap();

        let info = inspect_media_file(&path).unwrap();

        assert_eq!(info.file_name, "sample.mp3");
        assert_eq!(info.format, "MP3");
        assert_eq!(info.size_bytes, 4);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn telemetry_record_hashes_user_identifiers() {
        let record = telemetry_request_to_record(TelemetryEventRequest {
            event_type: "playback_seek".into(),
            job_id: Some("job-secret".into()),
            segment_id: Some("segment-secret".into()),
            audio_hours: None,
            transcript_chars: None,
            active_seconds: None,
            inactive_seconds: None,
            completed_ratio: None,
            op_type: None,
            chars_before: None,
            chars_after: None,
            source: None,
            from_ms: Some(1200),
            to_ms: Some(3000),
            trigger: Some("click_segment".into()),
            mark_ms: None,
            label_type: None,
            format: None,
            include_timestamps: None,
            include_speakers: None,
            include_marks: None,
        })
        .unwrap();

        let json = serde_json::to_string(&record).unwrap();

        assert!(!json.contains("job-secret"));
        assert!(!json.contains("segment-secret"));
        assert!(json.contains("jobIdHash"));
        assert!(json.contains("segmentIdHash"));
        assert!(json.contains("playback_seek"));
    }

    #[test]
    fn telemetry_rejects_unknown_event_type() {
        let error = telemetry_request_to_record(TelemetryEventRequest {
            event_type: "raw_transcript_upload".into(),
            job_id: None,
            segment_id: None,
            audio_hours: None,
            transcript_chars: None,
            active_seconds: None,
            inactive_seconds: None,
            completed_ratio: None,
            op_type: None,
            chars_before: None,
            chars_after: None,
            source: None,
            from_ms: None,
            to_ms: None,
            trigger: None,
            mark_ms: None,
            label_type: None,
            format: None,
            include_timestamps: None,
            include_speakers: None,
            include_marks: None,
        })
        .unwrap_err();

        assert!(error.contains("Unsupported telemetry event type"));
    }

    #[test]
    fn telemetry_consent_defaults_to_disabled_and_undecided() {
        let path = std::env::temp_dir().join(format!(
            "audraflow-consent-missing-{}.json",
            uuid::Uuid::new_v4()
        ));

        let state = read_telemetry_consent_from_path(&path);

        assert!(!state.enabled);
        assert!(!state.decided);
        assert!(state.updated_at_ms.is_none());
    }

    #[test]
    fn telemetry_consent_roundtrips_enabled_state() {
        let root = std::env::temp_dir().join(format!("audraflow-consent-{}", uuid::Uuid::new_v4()));
        let path = root.join("privacy").join("telemetry-consent.json");

        let written = write_telemetry_consent_to_path(&path, true).unwrap();
        let loaded = read_telemetry_consent_from_path(&path);

        assert!(written.enabled);
        assert!(written.decided);
        assert!(loaded.enabled);
        assert!(loaded.decided);
        assert!(loaded.updated_at_ms.is_some());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn direct_media_urls_are_not_treated_as_platform_links() {
        let direct = reqwest::Url::parse("https://cdn.example.com/audio/file.mp3").unwrap();
        let youtube_direct =
            reqwest::Url::parse("https://youtube.com/downloads/archive.wav").unwrap();

        assert!(is_probable_direct_media_url(&direct));
        assert!(!is_probable_platform_url(&direct));
        assert!(is_probable_direct_media_url(&youtube_direct));
        assert!(!is_probable_platform_url(&youtube_direct));
    }

    #[test]
    fn known_platform_urls_require_platform_resolution() {
        let youtube = reqwest::Url::parse("https://www.youtube.com/watch?v=test").unwrap();
        let bilibili = reqwest::Url::parse("https://www.bilibili.com/video/BV123").unwrap();

        assert!(is_probable_platform_url(&youtube));
        assert!(is_probable_platform_url(&bilibili));
    }

    #[test]
    fn runtime_command_available_rejects_missing_absolute_path() {
        let missing = std::env::temp_dir().join(format!(
            "audraflow-missing-command-{}",
            uuid::Uuid::new_v4()
        ));

        assert!(!is_runtime_command_available(&missing));
    }

    #[test]
    fn directory_size_and_clear_children_work_recursively() {
        let root = std::env::temp_dir().join(format!("audraflow-cache-{}", uuid::Uuid::new_v4()));
        let nested = root.join("model-v1");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("model.bin"), [1u8; 7]).unwrap();
        std::fs::write(root.join("manifest.json"), [2u8; 3]).unwrap();

        let size = directory_size_bytes(&root).unwrap();
        let removed = clear_directory_children(&root).unwrap();

        assert_eq!(size, 10);
        assert_eq!(removed, 2);
        assert_eq!(count_directory_children(&root).unwrap(), 0);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn model_component_normalization_keeps_safe_path_parts() {
        assert_eq!(
            normalize_model_component(" ../ggml-base.bin ", "fallback"),
            "..ggml-base.bin"
        );
        assert_eq!(normalize_model_component("   ", "fallback"), "fallback");
    }

    #[test]
    fn default_transcription_language_uses_auto_for_multilingual_models() {
        assert_eq!(default_transcription_language("auto"), "auto");
        assert_eq!(default_transcription_language(" multilingual "), "auto");
        assert_eq!(default_transcription_language(""), "auto");
    }

    #[test]
    fn default_transcription_language_preserves_explicit_languages() {
        assert_eq!(default_transcription_language("en"), "en");
        assert_eq!(default_transcription_language(" ZH "), "zh");
    }

    #[test]
    fn normalize_transcription_language_accepts_explicit_audio_language() {
        assert_eq!(
            normalize_transcription_language(Some("english"), None),
            "en"
        );
        assert_eq!(normalize_transcription_language(Some("中文"), None), "zh");
        assert_eq!(normalize_transcription_language(Some("zh"), None), "zh");
        assert_eq!(normalize_transcription_language(Some("auto"), None), "auto");
        assert_eq!(normalize_transcription_language(None, None), "auto");
    }

    #[test]
    fn normalize_asr_engine_defaults_to_auto() {
        assert_eq!(normalize_asr_engine(None), "auto");
        assert_eq!(normalize_asr_engine(Some("auto")), "auto");
        assert_eq!(normalize_asr_engine(Some("funasr")), "funasr");
        assert_eq!(normalize_asr_engine(Some("fun-asr-nano")), "funasr");
        assert_eq!(normalize_asr_engine(Some("whisper.cpp")), "whisper");
    }

    #[test]
    fn resolve_asr_engine_prefers_whisper_when_available() {
        assert_eq!(resolve_asr_engine("auto", "music", true), "whisper");
        assert_eq!(resolve_asr_engine("auto", "music", false), "sensevoice");
        assert_eq!(resolve_asr_engine("auto", "speech", true), "whisper");
        assert_eq!(resolve_asr_engine("auto", "speech", false), "sensevoice");
        assert_eq!(
            resolve_asr_engine("sensevoice", "music", true),
            "sensevoice"
        );
        assert_eq!(resolve_asr_engine("funasr", "music", true), "funasr");
        assert_eq!(resolve_asr_engine("whisper", "speech", false), "whisper");
    }

    fn installed_whisper_model(name: &str) -> audraflow_model_manager::InstalledModel {
        audraflow_model_manager::InstalledModel {
            info: audraflow_model_manager::ModelInfo {
                name: name.into(),
                version: "test".into(),
                language: "auto".into(),
                size_bytes: 1,
                sha256: "hash".into(),
                download_url: "https://example.com/model.bin".into(),
                model_type: audraflow_model_manager::ModelType::WhisperCpp,
            },
            path: PathBuf::from(format!("/models/{name}.bin")),
            installed_at_ms: 1,
        }
    }

    #[test]
    fn preferred_lyrics_whisper_model_respects_selected_music_model() {
        let base = installed_whisper_model("base");
        let small = installed_whisper_model("small");
        let large = installed_whisper_model("large-v3-turbo-q8_0");
        let installed = vec![base, small.clone(), large.clone()];

        let preferred = preferred_lyrics_whisper_model(&installed, Some(&large), false).unwrap();

        assert_eq!(preferred.info.name, "large-v3-turbo-q8_0");
        assert_eq!(preferred.path, large.path);
    }

    #[test]
    fn preferred_lyrics_whisper_model_prefers_large_for_extreme_music_without_selection() {
        let base = installed_whisper_model("base");
        let small = installed_whisper_model("small");
        let large = installed_whisper_model("large-v3-turbo-q8_0");
        let installed = vec![base, small.clone(), large.clone()];

        let preferred = preferred_lyrics_whisper_model(&installed, None, true).unwrap();

        assert_eq!(preferred.info.name, "large-v3-turbo-q8_0");
        assert_eq!(preferred.path, large.path);
    }

    #[test]
    fn preferred_lyrics_whisper_model_prefers_small_for_balanced_music_without_selection() {
        let base = installed_whisper_model("base");
        let small = installed_whisper_model("small");
        let large = installed_whisper_model("large-v3-turbo-q8_0");
        let installed = vec![base, small.clone(), large.clone()];

        let preferred = preferred_lyrics_whisper_model(&installed, None, false).unwrap();

        assert_eq!(preferred.info.name, "small");
    }

    #[test]
    fn resolve_whisper_model_for_music_respects_selected_model_when_extreme() {
        let base = installed_whisper_model("base");
        let small = installed_whisper_model("small");
        let large = installed_whisper_model("large-v3-turbo-q8_0");
        let installed = vec![base, small.clone(), large.clone()];

        let resolved =
            resolve_whisper_model_for_job("whisper", "music", Some(small), &installed, true)
                .unwrap();

        assert_eq!(resolved.info.name, "small");
    }

    #[test]
    fn resolve_whisper_model_for_music_uses_large_when_extreme_without_selection() {
        let base = installed_whisper_model("base");
        let small = installed_whisper_model("small");
        let large = installed_whisper_model("large-v3-turbo-q8_0");
        let installed = vec![base, small, large.clone()];

        let resolved =
            resolve_whisper_model_for_job("whisper", "music", None, &installed, true).unwrap();

        assert_eq!(resolved.info.name, "large-v3-turbo-q8_0");
    }

    #[test]
    fn normalize_audio_mode_preserves_music_only() {
        assert_eq!(normalize_audio_mode(Some("music")), "music");
        assert_eq!(normalize_audio_mode(Some("lyrics")), "music");
        assert_eq!(normalize_audio_mode(Some("speech")), "speech");
        assert_eq!(normalize_audio_mode(None), "speech");
    }

    #[test]
    fn normalize_vocal_separation_requires_music_mode() {
        assert_eq!(
            normalize_vocal_separation(Some("demucs"), "music"),
            "demucs"
        );
        assert_eq!(normalize_vocal_separation(Some("on"), "music"), "demucs");
        assert_eq!(normalize_vocal_separation(Some("demucs"), "speech"), "off");
        assert_eq!(normalize_vocal_separation(None, "music"), "off");
    }

    #[test]
    fn infer_model_name_strips_common_ggml_prefix() {
        let path = PathBuf::from("C:\\models\\ggml-tiny.bin");

        assert_eq!(infer_model_name(&path, None), "tiny");
        assert_eq!(
            infer_model_name(&path, Some("custom/model".into())),
            "custommodel"
        );
    }

    #[test]
    fn infer_model_name_from_url_strips_common_ggml_prefix() {
        assert_eq!(
            infer_model_name_from_url("https://example.com/models/ggml-base.bin", None),
            "base"
        );
        assert_eq!(
            infer_model_name_from_url("https://example.com/model.bin", Some("custom/model".into())),
            "custommodel"
        );
    }

    #[test]
    fn device_diagnostics_are_always_reportable() {
        let diagnostics = detect_device_diagnostics();

        assert!(diagnostics.cpu_cores > 0);
        assert!(!diagnostics.device_tier.is_empty());
        if !diagnostics.cuda_available {
            assert!(diagnostics.fallback_message.is_some());
        }
    }

    #[test]
    fn export_conversion_preserves_json_metadata_and_docx_bytes() {
        let storage = audraflow_storage::Storage::open_in_memory().unwrap();
        storage
            .create_job("job-export", "C:\\media\\meeting.wav", "hash", false)
            .unwrap();
        storage
            .insert_segments(
                "job-export",
                &[Segment {
                    segment_id: "seg-1".into(),
                    start_ms: 1_000,
                    end_ms: 4_000,
                    speaker_id: Some("Speaker A".into()),
                    text: "corrected text".into(),
                    raw_text: "raw text".into(),
                    confidence: 0.62,
                    low_confidence_reasons: vec!["low_snr".into()],
                    corrections: vec![],
                    marks: vec![],
                }],
            )
            .unwrap();
        storage
            .record_correction(
                "seg-1",
                "text",
                "raw text",
                "corrected text",
                &CorrectionSource::User,
                false,
            )
            .unwrap();
        storage
            .add_mark("seg-1", 2_500, Some("review"), Some("check this"))
            .unwrap();

        let rows = storage.get_segments("job-export").unwrap();
        let json = render_export(
            &storage,
            "job-export",
            &rows,
            "json",
            SpeakerExportFilter::All,
            true,
            true,
        )
        .unwrap();

        assert!(json.contains("\"lowConfidenceReasons\""));
        assert!(json.contains("\"low_snr\""));
        assert!(json.contains("\"corrections\""));
        assert!(json.contains("\"oldValue\": \"raw text\""));
        assert!(json.contains("\"marks\""));
        assert!(json.contains("\"markMs\": 2500"));

        let obsidian = render_export(
            &storage,
            "job-export",
            &rows,
            "obsidian",
            SpeakerExportFilter::All,
            true,
            true,
        )
        .unwrap();
        assert!(obsidian.contains("> [!quote]"));
        assert!(obsidian.contains("Speaker A"));

        let notion = render_export(
            &storage,
            "job-export",
            &rows,
            "notion",
            SpeakerExportFilter::All,
            true,
            true,
        )
        .unwrap();
        assert!(notion.contains("<details>"));
        assert!(notion.contains("<summary>Speaker A"));

        let export_segments = storage_segments_to_ipc_segments(&storage, &rows, true).unwrap();
        let docx = audraflow_export::export_docx(
            &export_segments,
            "meeting",
            &audraflow_export::ExportOptions {
                include_timestamps: true,
                include_speakers: true,
                include_marks: true,
                speaker_filter: audraflow_export::SpeakerFilter::All,
            },
        )
        .unwrap();

        assert_eq!(&docx[0..2], b"PK");
    }

    #[test]
    fn speaker_filter_controls_exported_speaker_labels() {
        let named = SegmentRow {
            segment_id: "named".into(),
            start_ms: 0,
            end_ms: 1_000,
            speaker_id: Some("Alice".into()),
            text: "named speaker".into(),
            raw_text: "named speaker".into(),
            confidence: 0.9,
        };
        let placeholder = SegmentRow {
            segment_id: "placeholder".into(),
            start_ms: 1_000,
            end_ms: 2_000,
            speaker_id: Some("Speaker A".into()),
            text: "placeholder speaker".into(),
            raw_text: "placeholder speaker".into(),
            confidence: 0.9,
        };

        let all = [named.clone(), placeholder.clone()]
            .iter()
            .map(|segment| render_plain_segment(segment, SpeakerExportFilter::All, false))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains("Alice: named speaker"));
        assert!(all.contains("Speaker A: placeholder speaker"));

        let named_only = [named, placeholder]
            .iter()
            .map(|segment| render_plain_segment(segment, SpeakerExportFilter::NamedOnly, false))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(named_only.contains("Alice: named speaker"));
        assert!(named_only.contains("placeholder speaker"));
        assert!(!named_only.contains("Speaker A:"));

        assert_eq!(
            render_segment_text(
                &SegmentRow {
                    segment_id: "hidden".into(),
                    start_ms: 0,
                    end_ms: 1_000,
                    speaker_id: Some("Alice".into()),
                    text: "speaker hidden".into(),
                    raw_text: "speaker hidden".into(),
                    confidence: 0.9,
                },
                SpeakerExportFilter::Hidden,
            ),
            "speaker hidden"
        );
    }
}
