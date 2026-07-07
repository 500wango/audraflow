use crate::*;
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn cmd_export_transcript(
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
pub(crate) async fn cmd_render_transcript_export(
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

pub(crate) fn normalize_text_export_format(format: &str) -> Result<String, String> {
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
pub(crate) enum SpeakerExportFilter {
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

pub(crate) fn normalize_speaker_filter(
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

pub(crate) fn should_render_speaker(speaker: Option<&str>, filter: SpeakerExportFilter) -> bool {
    match filter {
        SpeakerExportFilter::Hidden => false,
        SpeakerExportFilter::All => speaker.is_some_and(|value| !value.trim().is_empty()),
        SpeakerExportFilter::NamedOnly => {
            speaker.is_some_and(audraflow_export::is_named_speaker)
        }
    }
}

pub(crate) fn export_title_for_job(
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

pub(crate) fn default_export_output_path(
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

pub(crate) fn resolve_export_output_path(
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

pub(crate) fn storage_segments_to_ipc_segments(
    storage: &audraflow_storage::Storage,
    segments: &[SegmentRow],
    include_marks: bool,
) -> Result<Vec<Segment>, String> {
    segments
        .iter()
        .map(|segment| storage_segment_to_ipc_segment(storage, segment, include_marks))
        .collect()
}

pub(crate) fn storage_segment_to_ipc_segment(
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

pub(crate) fn correction_source_from_storage(source: &str) -> CorrectionSource {
    match source.trim().to_ascii_lowercase().as_str() {
        "lexicon" => CorrectionSource::Lexicon,
        "merge" => CorrectionSource::Merge,
        _ => CorrectionSource::User,
    }
}

pub(crate) fn render_export(
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

pub(crate) fn apply_speaker_filter_to_segment(
    mut segment: Segment,
    speaker_filter: SpeakerExportFilter,
) -> Segment {
    if !should_render_speaker(segment.speaker_id.as_deref(), speaker_filter) {
        segment.speaker_id = None;
    }
    segment
}

pub(crate) fn render_markdown_segment(
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

pub(crate) fn render_plain_segment(
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

pub(crate) fn render_segment_text(segment: &SegmentRow, speaker_filter: SpeakerExportFilter) -> String {
    if should_render_speaker(segment.speaker_id.as_deref(), speaker_filter) {
        if let Some(speaker) = &segment.speaker_id {
            return format!("{speaker}: {}", segment.text);
        }
    }
    segment.text.clone()
}

pub(crate) fn format_clock_time(ms: i64) -> String {
    let total_seconds = ms.max(0) / 1000;
    let h = total_seconds / 3600;
    let m = (total_seconds % 3600) / 60;
    let s = total_seconds % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

pub(crate) fn format_srt_time(ms: i64) -> String {
    let total_ms = ms.max(0);
    let h = total_ms / 3_600_000;
    let m = (total_ms % 3_600_000) / 60_000;
    let s = (total_ms % 60_000) / 1000;
    let milli = total_ms % 1000;
    format!("{h:02}:{m:02}:{s:02},{milli:03}")
}

pub(crate) fn format_vtt_time(ms: i64) -> String {
    format_srt_time(ms).replace(',', ".")
}
