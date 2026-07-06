/// Periodically saves checkpoints for actively running jobs.
async fn checkpoint_saver_loop(state: Arc<Mutex<AppState>>) {
    log::info!("Checkpoint saver started");

    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;

        let app = state.lock().await;
        let active_jobs: Vec<_> = app.active_jobs.keys().cloned().collect();

        for job_id in active_jobs {
            if let Some(job) = app.active_jobs.get(&job_id) {
                if job.state == JobState::Running {
                    let ckpt = JobCheckpointState {
                        job_id: job_id.clone(),
                        file_path: job.file_path.clone(),
                        completed_segment_ids: job
                            .last_segment_id
                            .clone()
                            .map(|segment_id| vec![segment_id])
                            .unwrap_or_default(),
                        total_segments_processed: job.completed_segments,
                        progress: if job.total_segments > 0 {
                            job.completed_segments as f64 / job.total_segments as f64
                        } else {
                            0.0
                        },
                        rtf_estimate: job.rtf_estimate,
                    };
                    app.checkpoints
                        .save_checkpoint(
                            &job_id,
                            job.last_segment_id.as_deref().unwrap_or("auto"),
                            &ckpt,
                        )
                        .await
                        .ok();
                }
            }
        }
    }
}

// ── Utility ────────────────────────────────────────────────────────────────

async fn runtime_monitor_loop(state: Arc<Mutex<AppState>>) {
    log::info!("Runtime monitor started");

    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

        let candidates = {
            let app = state.lock().await;
            app.runtime_processes
                .iter()
                .map(|(job_id, pid)| (job_id.clone(), *pid))
                .collect::<Vec<_>>()
        };

        for (job_id, runtime_pid) in candidates {
            let should_recover = {
                let app = state.lock().await;
                app.active_jobs
                    .get(&job_id)
                    .is_some_and(|job| job.state == JobState::Running)
                    && app
                        .checkpoints
                        .check_runtime_alive(Some(runtime_pid))
                        .is_err()
            };

            if should_recover {
                mark_runtime_exit_detected(state.clone(), &job_id).await;
            }
        }
    }
}

async fn mark_runtime_exit_detected(state: Arc<Mutex<AppState>>, job_id: &str) {
    let mut app = state.lock().await;
    app.runtime_processes.remove(job_id);
    if let Some(active) = app.active_jobs.get_mut(job_id) {
        active.status_message = Some("ASR runtime exited; preparing recovery".into());
    }
    log::warn!("Runtime monitor detected exited ASR process for {job_id}");
}
