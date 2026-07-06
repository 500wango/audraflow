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
