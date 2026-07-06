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
