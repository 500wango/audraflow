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

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeComponentProgressEvent {
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

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeComponentDto {
    id: String,
    status: String,
    kind: String,
    install_dir: String,
    download_url: Option<String>,
    download_size_bytes: u64,
    installed_size_bytes: u64,
    required_files: Vec<String>,
    detail: Option<String>,
    installable: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeComponentActionResultDto {
    id: String,
    message: String,
    components: Vec<RuntimeComponentDto>,
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
