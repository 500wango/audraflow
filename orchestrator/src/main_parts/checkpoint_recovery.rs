#[cfg(test)]
async fn recover_job_from_checkpoint(
    state: Arc<Mutex<AppState>>,
    job_id: &str,
) -> anyhow::Result<()> {
    let checkpoints = {
        let app = state.lock().await;
        app.checkpoints.clone()
    };

    let Some((event, checkpoint_state)) = checkpoints.load_latest_checkpoint(job_id).await? else {
        let mut app = state.lock().await;
        if let Some(active) = app.active_jobs.get_mut(job_id) {
            active.state = JobState::Failed;
            active.status_message = Some("Runtime crashed and no checkpoint was available".into());
        }
        app.queue
            .fail_job(job_id, "Runtime crashed and no checkpoint was available");
        app.storage
            .lock()
            .await
            .update_job_state(job_id, "failed")
            .ok();
        app.runtime_processes.remove(job_id);
        return Ok(());
    };

    checkpoints.log_runtime_restart(job_id, &event.checkpoint_id);
    let mut app = state.lock().await;
    if let Some(active) = app.active_jobs.get_mut(job_id) {
        active.completed_segments = checkpoint_state.total_segments_processed;
        active.last_segment_id = Some(event.last_segment_id.clone());
        active.rtf_estimate = checkpoint_state.rtf_estimate;
        active.recoveries += 1;
        active.state = JobState::Running;
        active.status_message = Some(format!(
            "Recovered from checkpoint {} at {}",
            event.checkpoint_id, event.last_segment_id
        ));
    }

    app.runtime_processes.remove(job_id);
    app.storage
        .lock()
        .await
        .update_job_state(job_id, "running")
        .ok();

    log::warn!(
        "Recovered job {} from checkpoint {} ({})",
        job_id,
        event.checkpoint_id,
        event.last_segment_id
    );
    Ok(())
}

pub fn apply_gpu_oom_fallback(app: &mut AppState, job_id: &str, reason: &str) -> bool {
    let Some(planned) = app.job_plans.get(job_id).cloned() else {
        return false;
    };

    let fallback_plan =
        audraflow_scheduler::Scheduler::plan_cpu_fallback(&planned.scheduler_input, reason);
    app.job_plans.insert(
        job_id.to_string(),
        PlannedJob {
            scheduler_input: fallback_plan.input_signals.clone(),
            plan: fallback_plan.clone(),
        },
    );

    if let Some(active) = app.active_jobs.get_mut(job_id) {
        active.plan_id = fallback_plan.plan_id.clone();
        active.scheduler_input = Some(fallback_plan.input_signals.clone());
        active.model_size = Some(format!("{:?}", fallback_plan.model_size));
        active.estimated_seconds = Some(fallback_plan.estimated_duration_seconds);
        active.fallback_reason = fallback_plan.fallback_reason.clone();
        active.status_message = Some(format!(
            "GPU memory fallback: {}; using CPU plan {}",
            reason, fallback_plan.plan_id
        ));
        active.rtf_estimate = fallback_plan.estimated_duration_seconds.max(1.0);
    }

    log::warn!(
        "Applied GPU fallback for {}: {} plan={} model={:?} est={:.0}s",
        job_id,
        reason,
        fallback_plan.plan_id,
        fallback_plan.model_size,
        fallback_plan.estimated_duration_seconds
    );
    true
}

pub fn pause_job_for_low_disk(app: &mut AppState, job_id: &str, status: &DiskSpaceStatus) -> bool {
    let message = low_disk_message(status);
    let mut paused = app.queue.pause_job_with_message(job_id, &message);
    if let Some(active) = app.active_jobs.get_mut(job_id) {
        active.state = JobState::Paused;
        active.status_message = Some(message.clone());
        paused = true;
    }

    log::warn!(
        "Paused job {} because free disk space is low: available={} bytes min={} bytes target={}",
        job_id,
        status.available_bytes,
        status.min_free_bytes,
        status.target.display()
    );
    paused
}
