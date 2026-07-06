/// Background worker that dequeues jobs from the batch queue and runs ASR jobs.
async fn job_processor_loop(state: Arc<Mutex<AppState>>) {
    log::info!("Job processor started");

    loop {
        // Check if there are pending jobs
        let maybe_job = {
            let mut app = state.lock().await;
            app.queue.dequeue_next()
        };

        if let Some(job) = maybe_job {
            let job_id = job.job_id.clone();
            log::info!("Processing job: {} ({})", job_id, job.file_path);

            let preflight_disk_status = {
                let app = state.lock().await;
                match app.disk_guard.check_path(&job.file_path) {
                    Ok(status) if status.is_low() => Some(status),
                    Ok(_) => None,
                    Err(error) => {
                        log::warn!("Disk space check failed for {}: {error}", job.file_path);
                        None
                    }
                }
            };

            if let Some(status) = preflight_disk_status {
                let storage = {
                    let mut app = state.lock().await;
                    pause_job_for_low_disk(&mut app, &job_id, &status);
                    app.storage.clone()
                };
                storage
                    .lock()
                    .await
                    .update_job_state(&job_id, "paused")
                    .ok();
                continue;
            }

            if let Err(error) = validate_job_input(&job) {
                let mut app = state.lock().await;
                app.queue.fail_job(&job_id, &error);
                app.storage
                    .lock()
                    .await
                    .update_job_state(&job_id, "failed")
                    .ok();
                log::warn!("Job {} failed preflight: {}", job_id, error);
                continue;
            }

            {
                let mut app = state.lock().await;
                let planned = app.job_plans.get(&job_id).cloned();
                app.active_jobs.insert(
                    job_id.clone(),
                    ActiveJob {
                        file_path: job.file_path.clone(),
                        model_path: job.model_path.clone(),
                        model_name: job.model_name.clone(),
                        model_version: job.model_version.clone(),
                        plan_id: planned
                            .as_ref()
                            .map(|planned| planned.plan.plan_id.clone())
                            .unwrap_or_else(|| "queued".into()),
                        scheduler_input: planned
                            .as_ref()
                            .map(|planned| planned.scheduler_input.clone()),
                        model_size: planned
                            .as_ref()
                            .map(|planned| format!("{:?}", planned.plan.model_size)),
                        estimated_seconds: planned
                            .as_ref()
                            .map(|planned| planned.plan.estimated_duration_seconds),
                        fallback_reason: planned
                            .as_ref()
                            .and_then(|planned| planned.plan.fallback_reason.clone()),
                        extreme_accuracy: job.extreme_accuracy,
                        total_segments: 1,
                        completed_segments: 0,
                        last_segment_id: None,
                        rtf_estimate: 0.08,
                        recoveries: 0,
                        status_message: None,
                        state: JobState::Running,
                    },
                );
                app.storage
                    .lock()
                    .await
                    .update_job_state(&job_id, "running")
                    .ok();
            }

            // ── Record telemetry ───────────────────────────────────────────
            {
                let app = state.lock().await;
                app.telemetry.record(TelemetryEvent::ProofreadSessionStart {
                    job_id: job_id.clone(),
                    audio_hours: job.audio_duration_s / 3600.0,
                    transcript_chars: 0, // Will be updated when segments arrive
                    app_version: "0.1.0".into(),
                    model_version: job
                        .model_version
                        .clone()
                        .or_else(|| job.model_name.clone())
                        .unwrap_or_else(|| "unknown".into()),
                    timestamp_ms: chrono::Utc::now().timestamp_millis(),
                });
            }

            // ── Simulate processing ────────────────────────────────────────
            // In production: spawn ASR Runtime process, stream segments via IPC
            let outcome = match process_job_with_runtime(&job, &state).await {
                Ok(outcome) => outcome,
                Err(error) => {
                    let mut app = state.lock().await;
                    app.queue.fail_job(&job_id, &error);
                    if let Some(active) = app.active_jobs.get_mut(&job_id) {
                        active.state = JobState::Failed;
                        active.status_message = Some(error.clone());
                    }
                    app.storage
                        .lock()
                        .await
                        .update_job_state(&job_id, "failed")
                        .ok();
                    app.active_jobs.remove(&job_id);
                    log::warn!(
                        "Job {} failed during ASR runtime processing: {}",
                        job_id,
                        error
                    );
                    continue;
                }
            };

            // ── Update queue state ─────────────────────────────────────────
            let mut app = state.lock().await;
            match outcome {
                ProcessingOutcome::Completed => {
                    app.queue.complete_job(&job_id);
                    app.storage.lock().await.complete_job(&job_id).ok();
                    app.active_jobs.remove(&job_id);
                }
                #[cfg(test)]
                ProcessingOutcome::PausedLowDisk => {
                    app.storage
                        .lock()
                        .await
                        .update_job_state(&job_id, "paused")
                        .ok();
                }
                ProcessingOutcome::PausedUser => {
                    app.storage
                        .lock()
                        .await
                        .update_job_state(&job_id, "paused")
                        .ok();
                }
                ProcessingOutcome::Cancelled => {
                    app.active_jobs.remove(&job_id);
                    app.storage
                        .lock()
                        .await
                        .update_job_state(&job_id, "cancelled")
                        .ok();
                }
                ProcessingOutcome::Skipped => {
                    app.active_jobs.remove(&job_id);
                    app.storage
                        .lock()
                        .await
                        .update_job_state(&job_id, "cancelled")
                        .ok();
                }
            }
        } else {
            // No jobs ready — wait before checking again
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
    }
}
