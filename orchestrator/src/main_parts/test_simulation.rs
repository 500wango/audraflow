/// Simulate ASR job processing for recovery and control-flow tests.
#[cfg(test)]
async fn simulate_job_processing(
    job: &QueueItem,
    state: &Arc<Mutex<AppState>>,
) -> ProcessingOutcome {
    log::info!(
        "  Simulating: {} ({:.0}s audio, extreme={})",
        job.file_path,
        job.audio_duration_s,
        job.extreme_accuracy,
    );

    // Simulate processing time proportional to audio duration
    let sim_seconds = (job.audio_duration_s * 0.05).min(5.0); // Cap at 5s for demo
    let steps = simulated_segment_count(job);
    let step_delay = sim_seconds / steps as f64;

    for completed in 1..=steps {
        tokio::time::sleep(tokio::time::Duration::from_secs_f64(step_delay)).await;
        let last_segment_id = format!("sim-seg-{completed:03}");

        let (checkpoint, pause_for_low_disk, storage) = {
            let mut app = state.lock().await;
            if let Some(outcome) = job_control_outcome(&app, &job.job_id) {
                if let Some(active) = app.active_jobs.get_mut(&job.job_id) {
                    match outcome {
                        ProcessingOutcome::PausedUser => {
                            active.state = JobState::Paused;
                            active.status_message = Some("Paused".into());
                        }
                        ProcessingOutcome::Cancelled => {
                            active.state = JobState::Cancelled;
                            active.status_message = Some("Cancelled".into());
                        }
                        ProcessingOutcome::Skipped => {
                            active.state = JobState::Cancelled;
                            active.status_message = Some("Skipped".into());
                        }
                        ProcessingOutcome::Completed | ProcessingOutcome::PausedLowDisk => {}
                    }
                }
                log::info!("Job {} stopped by user control: {:?}", job.job_id, outcome);
                return outcome;
            }

            if let Some(active) = app.active_jobs.get_mut(&job.job_id) {
                active.completed_segments = completed;
                active.last_segment_id = Some(last_segment_id.clone());
                active.state = JobState::Running;
            }

            let low_disk_status = match app.disk_guard.check_path(&job.file_path) {
                Ok(status) if status.is_low() => Some(status),
                Ok(_) => None,
                Err(error) => {
                    log::warn!("Disk space check failed for {}: {error}", job.file_path);
                    None
                }
            };

            if let Some(status) = low_disk_status.as_ref() {
                pause_job_for_low_disk(&mut app, &job.job_id, status);
            }

            let should_save_checkpoint = low_disk_status.is_some()
                || app.checkpoints.should_save(completed)
                || completed == steps;

            let checkpoint = if should_save_checkpoint {
                Some((
                    app.checkpoints.clone(),
                    checkpoint_state_from_job(job, completed, steps, last_segment_id.clone()),
                    last_segment_id,
                ))
            } else {
                None
            };

            (checkpoint, low_disk_status.is_some(), app.storage.clone())
        };

        if let Some((checkpoints, checkpoint_state, last_segment_id)) = checkpoint {
            checkpoints
                .save_checkpoint(&job.job_id, &last_segment_id, &checkpoint_state)
                .await
                .ok();
        }

        if pause_for_low_disk {
            storage
                .lock()
                .await
                .update_job_state(&job.job_id, "paused")
                .ok();
            log::warn!("Job {} paused because disk space is low", job.job_id);
            return ProcessingOutcome::PausedLowDisk;
        }
    }

    log::info!("  Job {} completed (simulated)", job.job_id);
    ProcessingOutcome::Completed
}

fn job_control_outcome(app: &AppState, job_id: &str) -> Option<ProcessingOutcome> {
    match app.queue.get_item(job_id).map(|item| item.state) {
        Some(crate::batch_queue::QueueItemState::Paused) => Some(ProcessingOutcome::PausedUser),
        Some(crate::batch_queue::QueueItemState::Cancelled) => Some(ProcessingOutcome::Cancelled),
        Some(crate::batch_queue::QueueItemState::Skipped) => Some(ProcessingOutcome::Skipped),
        _ => None,
    }
}

fn validate_job_input(job: &QueueItem) -> Result<(), String> {
    let path = Path::new(&job.file_path);
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("Input file is not available: {}", error))?;
    if !metadata.is_file() {
        return Err("Input path is not a file".into());
    }
    if metadata.len() == 0 {
        return Err("Input file is empty".into());
    }
    std::fs::File::open(path)
        .map(|_| ())
        .map_err(|error| format!("Input file cannot be opened: {}", error))
}

#[cfg(test)]
fn simulated_segment_count(job: &QueueItem) -> u32 {
    let duration_based = (job.audio_duration_s / 30.0).ceil() as u32;
    duration_based.clamp(10, 120)
}

#[cfg(test)]
fn checkpoint_state_from_job(
    job: &QueueItem,
    completed_segments: u32,
    total_segments: u32,
    last_segment_id: String,
) -> JobCheckpointState {
    JobCheckpointState {
        job_id: job.job_id.clone(),
        file_path: job.file_path.clone(),
        completed_segment_ids: vec![last_segment_id],
        total_segments_processed: completed_segments,
        progress: if total_segments > 0 {
            completed_segments as f64 / total_segments as f64
        } else {
            0.0
        },
        rtf_estimate: 0.08,
    }
}

// ── Checkpoint Saver Loop ──────────────────────────────────────────────────
