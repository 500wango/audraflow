#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeTranscribeOutput {
    segments: Vec<Segment>,
    audio_duration_s: f64,
    rtf: f64,
    ttfv_s: f64,
    chunk_count: u32,
    #[serde(default)]
    preprocess_messages: Vec<String>,
}

async fn read_child_output<R>(reader: Option<R>) -> std::io::Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let mut output = Vec::new();
    if let Some(mut reader) = reader {
        reader.read_to_end(&mut output).await?;
    }
    Ok(output)
}

async fn process_job_with_runtime(
    job: &QueueItem,
    state: &Arc<Mutex<AppState>>,
) -> Result<ProcessingOutcome, String> {
    if let Some(outcome) = {
        let app = state.lock().await;
        job_control_outcome(&app, &job.job_id)
    } {
        return Ok(outcome);
    }

    let asr_engine = normalize_asr_engine_hint(job.asr_engine.as_deref());
    if asr_engine == "whisper" {
        let model_path = job
            .model_path
            .as_deref()
            .ok_or_else(|| "No ASR model path was attached to this Whisper job".to_string())?;
        validate_runtime_model_path(model_path)?;
    } else if let Some(model_path) = job.model_path.as_deref() {
        validate_runtime_model_path(model_path)?;
    }

    loop {
        match run_runtime_attempt(job, state).await {
            Ok(outcome) => return Ok(outcome),
            Err(error) => {
                if !recover_runtime_failure(job, state, &error).await? {
                    return Err(error);
                }
            }
        }
    }
}

async fn run_runtime_attempt(
    job: &QueueItem,
    state: &Arc<Mutex<AppState>>,
) -> Result<ProcessingOutcome, String> {
    update_active_status(
        state,
        ActiveStatusUpdate {
            job_id: &job.job_id,
            state: JobState::Running,
            status_message: Some(starting_runtime_message(job)),
            completed_segments: 0,
            total_segments: 1,
            last_segment_id: None,
            rtf_estimate: None,
        },
    )
    .await;

    let checkpoints = {
        let app = state.lock().await;
        app.checkpoints.clone()
    };
    checkpoints
        .save_checkpoint(
            &job.job_id,
            "start",
            &JobCheckpointState {
                job_id: job.job_id.clone(),
                file_path: job.file_path.clone(),
                completed_segment_ids: Vec::new(),
                total_segments_processed: 0,
                progress: 0.0,
                rtf_estimate: 0.0,
            },
        )
        .await
        .ok();

    let mut command = runtime_transcribe_command(job)?;
    let started_at = std::time::Instant::now();
    let mut child = command
        .spawn()
        .map_err(|error| format!("Failed to start ASR runtime: {error}"))?;
    let runtime_pid = child.id();
    if let Some(pid) = runtime_pid {
        let mut app = state.lock().await;
        app.runtime_processes.insert(job.job_id.clone(), pid);
    }

    let stdout_task = tokio::spawn(read_child_output(child.stdout.take()));
    let stderr_task = tokio::spawn(read_child_output(child.stderr.take()));
    let status = loop {
        if let Some(outcome) = {
            let app = state.lock().await;
            job_control_outcome(&app, &job.job_id)
        } {
            if let Err(error) = child.kill().await {
                log::debug!("Failed to kill ASR runtime for {}: {}", job.job_id, error);
            }
            let _ = child.wait().await;
            let _ = stdout_task.await;
            let _ = stderr_task.await;
            let mut app = state.lock().await;
            app.runtime_processes.remove(&job.job_id);
            return Ok(outcome);
        }

        match child
            .try_wait()
            .map_err(|error| format!("Failed to poll ASR runtime: {error}"))?
        {
            Some(status) => break status,
            None => tokio::time::sleep(tokio::time::Duration::from_millis(250)).await,
        }
    };

    let mut app = state.lock().await;
    app.runtime_processes.remove(&job.job_id);
    drop(app);

    let stdout = stdout_task
        .await
        .map_err(|error| format!("Failed to join ASR runtime stdout reader: {error}"))?
        .map_err(|error| format!("Failed to read ASR runtime stdout: {error}"))?;
    let stderr = stderr_task
        .await
        .map_err(|error| format!("Failed to join ASR runtime stderr reader: {error}"))?
        .map_err(|error| format!("Failed to read ASR runtime stderr: {error}"))?;

    if let Some(outcome) = {
        let app = state.lock().await;
        job_control_outcome(&app, &job.job_id)
    } {
        return Ok(outcome);
    }

    if !status.success() {
        return Err(format!(
            "ASR runtime exited with {}. stderr: {} stdout: {}",
            status,
            preview_bytes(&stderr),
            preview_bytes(&stdout),
        ));
    }

    let parsed: RuntimeTranscribeOutput = serde_json::from_slice(&stdout).map_err(|error| {
        format!(
            "ASR runtime returned invalid JSON: {error}. stdout: {} stderr: {}",
            preview_bytes(&stdout),
            preview_bytes(&stderr)
        )
    })?;
    if parsed.segments.is_empty() {
        return Err("ASR runtime produced no transcript segments".into());
    }

    let storage = {
        let app = state.lock().await;
        app.storage.clone()
    };
    let segments = {
        let storage = storage.lock().await;
        apply_glossary_to_segments(&storage, parsed.segments)?
    };
    let total_segments = segments.len() as u32;
    let last_segment_id = segments
        .last()
        .map(|segment| segment.segment_id.clone())
        .unwrap_or_else(|| "complete".into());
    storage
        .lock()
        .await
        .insert_segments(&job.job_id, &segments)
        .map_err(|error| format!("Failed to persist transcript segments: {error}"))?;

    let elapsed_s = started_at.elapsed().as_secs_f64();
    let rtf = if parsed.rtf.is_finite() && parsed.rtf > 0.0 {
        parsed.rtf
    } else if parsed.audio_duration_s > 0.0 {
        elapsed_s / parsed.audio_duration_s
    } else {
        0.0
    };

    update_active_status(
        state,
        ActiveStatusUpdate {
            job_id: &job.job_id,
            state: JobState::Running,
            status_message: Some(runtime_complete_message(
                total_segments,
                parsed.chunk_count,
                rtf,
                &parsed.preprocess_messages,
            )),
            completed_segments: total_segments,
            total_segments,
            last_segment_id: Some(last_segment_id.clone()),
            rtf_estimate: Some(rtf),
        },
    )
    .await;

    let checkpoint_state = JobCheckpointState {
        job_id: job.job_id.clone(),
        file_path: job.file_path.clone(),
        completed_segment_ids: vec![last_segment_id.clone()],
        total_segments_processed: total_segments,
        progress: 1.0,
        rtf_estimate: rtf,
    };
    let checkpoints = {
        let app = state.lock().await;
        app.checkpoints.clone()
    };
    checkpoints
        .save_checkpoint(&job.job_id, &last_segment_id, &checkpoint_state)
        .await
        .ok();

    log::info!(
        "Job {} completed by ASR runtime: segments={} chunks={} rtf={:.3} ttfv={:.1}s elapsed={:.1}s",
        job.job_id,
        total_segments,
        parsed.chunk_count,
        rtf,
        parsed.ttfv_s,
        elapsed_s
    );

    Ok(ProcessingOutcome::Completed)
}

fn runtime_complete_message(
    total_segments: u32,
    chunk_count: u32,
    rtf: f64,
    preprocess_messages: &[String],
) -> String {
    let mut message = format!(
        "ASR complete: {} segments, {} chunks, RTF {:.2}",
        total_segments, chunk_count, rtf
    );
    if let Some(preprocess_message) = preprocess_messages.first() {
        message.push_str(" · ");
        message.push_str(preprocess_message);
    }
    message
}

async fn recover_runtime_failure(
    job: &QueueItem,
    state: &Arc<Mutex<AppState>>,
    error: &str,
) -> Result<bool, String> {
    let recovery = {
        let mut app = state.lock().await;
        app.runtime_processes.remove(&job.job_id);
        let checkpoints = app.checkpoints.clone();
        let storage = app.storage.clone();
        let Some(active) = app.active_jobs.get_mut(&job.job_id) else {
            return Ok(false);
        };

        if active.recoveries >= MAX_RUNTIME_RECOVERIES {
            active.status_message = Some(format!(
                "ASR runtime failed after {} recovery attempt(s): {}",
                active.recoveries, error
            ));
            return Ok(false);
        }

        active.recoveries += 1;
        active.state = JobState::Running;
        let last_segment_id = active
            .last_segment_id
            .clone()
            .unwrap_or_else(|| "start".into());
        let checkpoint_state = JobCheckpointState {
            job_id: job.job_id.clone(),
            file_path: active.file_path.clone(),
            completed_segment_ids: active
                .last_segment_id
                .clone()
                .map(|segment_id| vec![segment_id])
                .unwrap_or_default(),
            total_segments_processed: active.completed_segments,
            progress: if active.total_segments > 0 {
                active.completed_segments as f64 / active.total_segments as f64
            } else {
                0.0
            },
            rtf_estimate: active.rtf_estimate,
        };
        active.status_message = Some(format!(
            "ASR runtime failed; restarting from checkpoint {} ({}/{})",
            last_segment_id, active.recoveries, MAX_RUNTIME_RECOVERIES
        ));

        (
            checkpoints,
            storage,
            last_segment_id,
            checkpoint_state,
            active.recoveries,
        )
    };

    let (checkpoints, storage, last_segment_id, checkpoint_state, recovery_count) = recovery;
    checkpoints
        .save_checkpoint(&job.job_id, &last_segment_id, &checkpoint_state)
        .await
        .map_err(|error| format!("Failed to save recovery checkpoint: {error}"))?;
    storage
        .lock()
        .await
        .update_job_state(&job.job_id, "running")
        .ok();

    log::warn!(
        "Restarting ASR runtime for {} from checkpoint {} after failure ({}/{}): {}",
        job.job_id,
        last_segment_id,
        recovery_count,
        MAX_RUNTIME_RECOVERIES,
        error
    );

    Ok(true)
}
