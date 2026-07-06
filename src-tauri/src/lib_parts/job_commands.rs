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
            cmd_get_runtime_components,
            cmd_download_runtime_component,
            cmd_delete_runtime_component,
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
