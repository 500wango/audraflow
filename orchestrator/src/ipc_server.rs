//! Named Pipe IPC server for the Orchestrator.
//!
//! Windows: JSON over Named Pipe.
//! Unix: JSON over Unix Domain Socket.

use crate::{apply_gpu_oom_fallback, AppState, PlannedJob};
use audraflow_ipc::error_codes;
use audraflow_ipc::{IpcEnvelope, IpcMessage, JobPlan, JobState, JobStatus};
use audraflow_scheduler::{DeviceTier, Scheduler, SchedulerInput};
#[cfg(not(target_os = "windows"))]
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::batch_queue::{estimate_job_cost, QueueItem, QueueItemState};

#[cfg(target_os = "windows")]
use anyhow::Context;
#[cfg(target_os = "windows")]
use tokio::io::AsyncWriteExt;
#[cfg(target_os = "windows")]
use tokio::net::windows::named_pipe;
#[cfg(not(target_os = "windows"))]
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
};

pub fn default_ipc_endpoint() -> String {
    #[cfg(target_os = "windows")]
    {
        r"\\.\pipe\audraflow-orchestrator".to_string()
    }

    #[cfg(not(target_os = "windows"))]
    {
        orchestrator_socket_path().to_string_lossy().into_owned()
    }
}

/// Run the named pipe server. Each connection is handled in a separate task.
#[cfg(target_os = "windows")]
pub async fn run_named_pipe_server(
    state: Arc<Mutex<AppState>>,
    pipe_name: &str,
) -> anyhow::Result<()> {
    log::info!("Starting Named Pipe server on: {}", pipe_name);

    loop {
        let server = named_pipe::ServerOptions::new()
            .first_pipe_instance(false)
            .create(pipe_name)
            .context("Failed to create named pipe")?;

        server.connect().await?;
        log::debug!("Client connected");

        let state = state.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_connected_client(server, state).await {
                log::error!("Connection error: {}", e);
            }
        });
    }
}

#[cfg(target_os = "windows")]
async fn handle_connected_client(
    mut server: tokio::net::windows::named_pipe::NamedPipeServer,
    state: Arc<Mutex<AppState>>,
) -> anyhow::Result<()> {
    let mut buf = vec![0u8; 65536];

    loop {
        server.readable().await?;
        match server.try_read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let msg: IpcEnvelope = serde_json::from_slice(&buf[..n])?;
                let response = handle_message(state.clone(), msg).await?;
                let reply_json = serde_json::to_vec(&response)?;
                server.write_all(&reply_json).await?;
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(e) => {
                log::error!("Read error: {}", e);
                break;
            }
        }
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub async fn run_named_pipe_server(
    state: Arc<Mutex<AppState>>,
    socket_path: &str,
) -> anyhow::Result<()> {
    let socket_path = PathBuf::from(socket_path);
    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    if socket_path.exists() {
        let _ = tokio::fs::remove_file(&socket_path).await;
    }

    let listener = UnixListener::bind(&socket_path)?;
    log::info!("Starting Unix socket server on: {}", socket_path.display());

    loop {
        let (stream, _) = listener.accept().await?;
        log::debug!("Unix socket client connected");
        let state = state.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_connected_client(stream, state).await {
                log::error!("Connection error: {}", e);
            }
        });
    }
}

#[cfg(not(target_os = "windows"))]
async fn handle_connected_client(
    mut stream: UnixStream,
    state: Arc<Mutex<AppState>>,
) -> anyhow::Result<()> {
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await?;
    if buf.is_empty() {
        return Ok(());
    }

    let msg: IpcEnvelope = serde_json::from_slice(&buf)?;
    let response = handle_message(state, msg).await?;
    let reply_json = serde_json::to_vec(&response)?;
    stream.write_all(&reply_json).await?;
    stream.shutdown().await?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn orchestrator_socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("audraflow-orchestrator.sock")
}

// ── Message Router ─────────────────────────────────────────────────────────

async fn handle_message(
    state: Arc<Mutex<AppState>>,
    envelope: IpcEnvelope,
) -> anyhow::Result<IpcEnvelope> {
    match &envelope.payload {
        IpcMessage::JobCreate(req) => {
            let mut app = state.lock().await;

            // Persist job
            app.storage.lock().await.create_job(
                &req.job_id,
                &req.file_path,
                &req.file_hash,
                req.extreme_accuracy,
            )?;

            // Run scheduler
            let device_tier = DeviceTier::classify(false, None, num_cpus::get() as u32);
            let scheduler_input = SchedulerInput {
                duration_seconds: req.audio_duration_s.unwrap_or(0.0),
                snr_db: req.snr_db,
                speech_density: None,
                estimated_speaker_count: req.estimated_speakers.unwrap_or(1),
                is_high_noise: false,
                device_tier,
                cuda_available: false,
                vram_gb: None,
                cpu_cores: num_cpus::get() as u32,
                extreme_accuracy: req.extreme_accuracy,
                model_cached: req.model_path.is_some(),
                cold_start_seconds: None,
            };
            let plan = Scheduler::plan(&scheduler_input);
            app.job_plans.insert(
                req.job_id.clone(),
                PlannedJob {
                    scheduler_input: scheduler_input.clone(),
                    plan: plan.clone(),
                },
            );

            // Enqueue in batch queue
            let cost = estimate_job_cost(
                req.audio_duration_s.unwrap_or(0.0),
                req.extreme_accuracy,
                false,
            );
            let item = QueueItem {
                job_id: req.job_id.clone(),
                file_path: req.file_path.clone(),
                file_hash: req.file_hash.clone(),
                asr_engine: req.asr_engine.clone(),
                model_path: req.model_path.clone(),
                model_name: req.model_name.clone(),
                model_version: req.model_version.clone(),
                language: req.language.clone(),
                audio_mode: req.audio_mode.clone(),
                vocal_separation: req.vocal_separation.clone(),
                audio_duration_s: req.audio_duration_s.unwrap_or(0.0),
                extreme_accuracy: req.extreme_accuracy,
                state: QueueItemState::Pending,
                cost_estimate: cost,
                retry_count: 0,
                error_message: None,
            };
            app.queue.enqueue(item);

            log::info!(
                "Job created: {} ({:.0}s, extreme={}), plan={}, queue_size={}",
                req.job_id,
                req.audio_duration_s.unwrap_or(0.0),
                req.extreme_accuracy,
                plan.plan_id,
                app.queue.total_count(),
            );

            Ok(IpcEnvelope::new(IpcMessage::JobPlan(JobPlan {
                job_id: req.job_id.clone(),
                plan_id: plan.plan_id,
                model_size: format!("{:?}", plan.model_size),
                estimated_seconds: plan.estimated_duration_seconds,
                explanation: plan.explanation,
                fallback_reason: plan.fallback_reason,
            })))
        }

        IpcMessage::JobCancel(ctrl) => {
            let mut app = state.lock().await;
            if !app.queue.cancel_job(&ctrl.job_id) {
                return Ok(control_rejected_status(
                    &ctrl.job_id,
                    "Job cannot be cancelled",
                ));
            }
            if let Some(active) = app.active_jobs.get_mut(&ctrl.job_id) {
                active.state = JobState::Cancelled;
                active.status_message = Some("Cancelled".into());
            }
            app.storage
                .lock()
                .await
                .update_job_state(&ctrl.job_id, "cancelled")?;
            Ok(IpcEnvelope::new(IpcMessage::JobStatus(JobStatus {
                job_id: ctrl.job_id.clone(),
                state: JobState::Cancelled,
                progress_pct: 0.0,
                message: Some("Cancelled".into()),
                estimated_remaining_s: None,
                rtf_current: None,
                ttfv_s: None,
            })))
        }

        IpcMessage::JobPause(ctrl) => {
            let mut app = state.lock().await;
            if !app.queue.pause_job(&ctrl.job_id) {
                return Ok(control_rejected_status(
                    &ctrl.job_id,
                    "Job cannot be paused",
                ));
            }
            if let Some(active) = app.active_jobs.get_mut(&ctrl.job_id) {
                active.state = JobState::Paused;
                active.status_message = Some("Paused".into());
            }
            app.storage
                .lock()
                .await
                .update_job_state(&ctrl.job_id, "paused")?;
            Ok(IpcEnvelope::new(IpcMessage::JobStatus(JobStatus {
                job_id: ctrl.job_id.clone(),
                state: JobState::Paused,
                progress_pct: 0.0,
                message: Some("Paused".into()),
                estimated_remaining_s: None,
                rtf_current: None,
                ttfv_s: None,
            })))
        }

        IpcMessage::JobResume(ctrl) => {
            let mut app = state.lock().await;
            if !app.queue.resume_job(&ctrl.job_id) {
                return Ok(control_rejected_status(
                    &ctrl.job_id,
                    "Job cannot be resumed",
                ));
            }
            if let Some(active) = app.active_jobs.get_mut(&ctrl.job_id) {
                active.state = JobState::Pending;
                active.status_message = Some("Resume queued".into());
            }
            app.storage
                .lock()
                .await
                .update_job_state(&ctrl.job_id, "pending")?;
            Ok(IpcEnvelope::new(IpcMessage::JobStatus(JobStatus {
                job_id: ctrl.job_id.clone(),
                state: JobState::Pending,
                progress_pct: 0.0,
                message: Some("Resume queued".into()),
                estimated_remaining_s: None,
                rtf_current: None,
                ttfv_s: None,
            })))
        }

        IpcMessage::JobRetry(ctrl) => {
            let mut app = state.lock().await;
            if !app.queue.retry_job(&ctrl.job_id) {
                return Ok(control_rejected_status(
                    &ctrl.job_id,
                    "Job cannot be retried",
                ));
            }
            app.active_jobs.remove(&ctrl.job_id);
            app.storage
                .lock()
                .await
                .update_job_state(&ctrl.job_id, "pending")?;
            Ok(IpcEnvelope::new(IpcMessage::JobStatus(JobStatus {
                job_id: ctrl.job_id.clone(),
                state: JobState::Pending,
                progress_pct: 0.0,
                message: Some("Retry queued".into()),
                estimated_remaining_s: None,
                rtf_current: None,
                ttfv_s: None,
            })))
        }

        IpcMessage::JobSkip(ctrl) => {
            let mut app = state.lock().await;
            if !app.queue.skip_job(&ctrl.job_id) {
                return Ok(control_rejected_status(
                    &ctrl.job_id,
                    "Job cannot be skipped",
                ));
            }
            if let Some(active) = app.active_jobs.get_mut(&ctrl.job_id) {
                active.state = JobState::Cancelled;
                active.status_message = Some("Skipped".into());
            }
            app.storage
                .lock()
                .await
                .update_job_state(&ctrl.job_id, "cancelled")?;
            Ok(IpcEnvelope::new(IpcMessage::JobStatus(JobStatus {
                job_id: ctrl.job_id.clone(),
                state: JobState::Cancelled,
                progress_pct: 0.0,
                message: Some("Skipped".into()),
                estimated_remaining_s: None,
                rtf_current: None,
                ttfv_s: None,
            })))
        }

        IpcMessage::JobStatus(req) => {
            let app = state.lock().await;
            if let Some(active) = app.active_jobs.get(&req.job_id) {
                let progress_pct = if active.total_segments > 0 {
                    active.completed_segments as f64 / active.total_segments as f64 * 100.0
                } else {
                    0.0
                };
                return Ok(IpcEnvelope::new(IpcMessage::JobStatus(JobStatus {
                    job_id: req.job_id.clone(),
                    state: active.state.clone(),
                    progress_pct,
                    message: active.status_message.clone().or_else(|| {
                        active.last_segment_id.as_ref().map(|segment_id| {
                            format!(
                                "Transcribing segment {}/{} ({segment_id})",
                                active.completed_segments, active.total_segments
                            )
                        })
                    }),
                    estimated_remaining_s: active.estimated_seconds,
                    rtf_current: Some(active.rtf_estimate),
                    ttfv_s: None,
                })));
            }

            if let Some(item) = app.queue.get_item(&req.job_id) {
                return Ok(IpcEnvelope::new(IpcMessage::JobStatus(JobStatus {
                    job_id: req.job_id.clone(),
                    state: queue_state_to_job_state(item.state),
                    progress_pct: queue_progress_pct(item.state),
                    message: item.error_message.clone().or_else(|| {
                        Some(match item.state {
                            QueueItemState::Pending => "Queued".to_string(),
                            QueueItemState::Running => "Running".to_string(),
                            QueueItemState::Paused => "Paused".to_string(),
                            QueueItemState::Completed => "Completed".to_string(),
                            QueueItemState::Cancelled => "Cancelled".to_string(),
                            QueueItemState::Failed => "Failed".to_string(),
                            QueueItemState::Skipped => "Skipped".to_string(),
                        })
                    }),
                    estimated_remaining_s: None,
                    rtf_current: None,
                    ttfv_s: None,
                })));
            }

            let job = app.storage.lock().await.get_job(&req.job_id)?;
            if let Some(job) = job {
                let state = storage_state_to_job_state(&job.state);
                return Ok(IpcEnvelope::new(IpcMessage::JobStatus(JobStatus {
                    job_id: req.job_id.clone(),
                    progress_pct: if state == JobState::Completed {
                        100.0
                    } else {
                        0.0
                    },
                    message: Some(job.state),
                    state,
                    estimated_remaining_s: None,
                    rtf_current: None,
                    ttfv_s: None,
                })));
            }

            Ok(IpcEnvelope::new(IpcMessage::JobStatus(JobStatus {
                job_id: req.job_id.clone(),
                state: JobState::NotFound,
                progress_pct: 0.0,
                message: Some("Job not found".into()),
                estimated_remaining_s: None,
                rtf_current: None,
                ttfv_s: None,
            })))
        }

        IpcMessage::ErrorReport(report) => {
            if report.error_code == error_codes::GPU_OOM && report.recoverable {
                let mut app = state.lock().await;
                let reason = report
                    .fallback_action
                    .as_deref()
                    .unwrap_or("GPU out of memory");
                if apply_gpu_oom_fallback(&mut app, &report.job_id, reason) {
                    return Ok(IpcEnvelope::new(IpcMessage::JobStatus(JobStatus {
                        job_id: report.job_id.clone(),
                        state: JobState::Running,
                        progress_pct: app
                            .active_jobs
                            .get(&report.job_id)
                            .map(|active| {
                                if active.total_segments > 0 {
                                    active.completed_segments as f64 / active.total_segments as f64
                                        * 100.0
                                } else {
                                    0.0
                                }
                            })
                            .unwrap_or(0.0),
                        message: Some(format!(
                            "GPU memory fallback active: {}; continuing on CPU",
                            reason
                        )),
                        estimated_remaining_s: app
                            .active_jobs
                            .get(&report.job_id)
                            .and_then(|active| active.estimated_seconds),
                        rtf_current: None,
                        ttfv_s: None,
                    })));
                }
            }

            let mut app = state.lock().await;
            app.queue.fail_job(&report.job_id, &report.error_message);
            app.storage
                .lock()
                .await
                .update_job_state(&report.job_id, "failed")?;
            Ok(IpcEnvelope::new(IpcMessage::JobStatus(JobStatus {
                job_id: report.job_id.clone(),
                state: JobState::Failed,
                progress_pct: 0.0,
                message: Some(report.error_message.clone()),
                estimated_remaining_s: None,
                rtf_current: None,
                ttfv_s: None,
            })))
        }

        other => {
            log::debug!("Unhandled message: {:?}", other);
            Ok(envelope)
        }
    }
}

fn queue_state_to_job_state(state: QueueItemState) -> JobState {
    match state {
        QueueItemState::Pending => JobState::Pending,
        QueueItemState::Running => JobState::Running,
        QueueItemState::Paused => JobState::Paused,
        QueueItemState::Completed => JobState::Completed,
        QueueItemState::Cancelled => JobState::Cancelled,
        QueueItemState::Failed => JobState::Failed,
        QueueItemState::Skipped => JobState::Cancelled,
    }
}

fn queue_progress_pct(state: QueueItemState) -> f64 {
    match state {
        QueueItemState::Completed => 100.0,
        QueueItemState::Cancelled | QueueItemState::Failed | QueueItemState::Skipped => 0.0,
        QueueItemState::Running => 1.0,
        QueueItemState::Pending | QueueItemState::Paused => 0.0,
    }
}

fn control_rejected_status(job_id: &str, message: &str) -> IpcEnvelope {
    IpcEnvelope::new(IpcMessage::JobStatus(JobStatus {
        job_id: job_id.to_string(),
        state: JobState::NotFound,
        progress_pct: 0.0,
        message: Some(message.into()),
        estimated_remaining_s: None,
        rtf_current: None,
        ttfv_s: None,
    }))
}

fn storage_state_to_job_state(state: &str) -> JobState {
    match state {
        "pending" => JobState::Pending,
        "running" => JobState::Running,
        "paused" => JobState::Paused,
        "completed" => JobState::Completed,
        "cancelled" => JobState::Cancelled,
        "failed" => JobState::Failed,
        _ => JobState::NotFound,
    }
}
