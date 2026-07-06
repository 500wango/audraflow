//! AudraFlow Task Orchestrator — Full Service
//!
//! The central daemon that coordinates all subsystems:
//! - IPC: Named Pipe server for UI ↔ Orchestrator communication
//! - BatchQueue: multi-job queue with concurrency control
//! - Scheduler: adaptive plan generation per job
//! - CheckpointManager: periodic state saves for crash recovery
//! - Telemetry: behavioral event collection (MCM/H)
//! - Storage: SQLite persistence for all data
//!
//! Process isolation: Orchestrator is a separate process from UI and ASR Runtime.

use audraflow_ipc::{Correction, Segment};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::Mutex;

mod batch_queue;
mod checkpoint;
mod ipc_server;
mod job_manager;
mod telemetry;

use audraflow_ipc::JobState;
use audraflow_post_processor::{GlossaryEntry, PostProcessor};
use audraflow_scheduler::{SchedulerInput, SchedulerPlan};
use audraflow_storage::{GlossaryEntryRow, Storage};
use batch_queue::{BatchQueue, QueueItem};
use checkpoint::CheckpointManager;
pub use checkpoint::JobCheckpointState;
use telemetry::{TelemetryCollectorStd, TelemetryEvent};

const MIN_FREE_DISK_BYTES: u64 = 500 * 1024 * 1024;
const MAX_RUNTIME_RECOVERIES: u32 = 2;

// ── Complete App State ─────────────────────────────────────────────────────

/// The full orchestrator state shared across all handlers and workers.
pub struct AppState {
    pub storage: Arc<Mutex<Storage>>,
    pub queue: BatchQueue,
    pub checkpoints: CheckpointManager,
    pub telemetry: TelemetryCollectorStd,
    pub active_jobs: HashMap<String, ActiveJob>,
    pub job_plans: HashMap<String, PlannedJob>,
    pub runtime_processes: HashMap<String, u32>,
    pub disk_guard: DiskSpaceGuard,
}

#[derive(Clone)]
pub struct PlannedJob {
    pub scheduler_input: SchedulerInput,
    pub plan: SchedulerPlan,
}

/// Information about an actively processing job.
pub struct ActiveJob {
    pub file_path: String,
    pub model_path: Option<String>,
    pub model_name: Option<String>,
    pub model_version: Option<String>,
    pub plan_id: String,
    pub scheduler_input: Option<SchedulerInput>,
    pub model_size: Option<String>,
    pub estimated_seconds: Option<f64>,
    pub fallback_reason: Option<String>,
    pub extreme_accuracy: bool,
    pub total_segments: u32,
    pub completed_segments: u32,
    pub last_segment_id: Option<String>,
    pub rtf_estimate: f64,
    pub recoveries: u32,
    pub status_message: Option<String>,
    pub state: JobState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessingOutcome {
    Completed,
    PausedUser,
    #[cfg(test)]
    PausedLowDisk,
    Cancelled,
    Skipped,
}

#[derive(Debug, Clone, Copy)]
pub struct DiskSpaceGuard {
    pub min_free_bytes: u64,
}

impl DiskSpaceGuard {
    pub fn new() -> Self {
        Self {
            min_free_bytes: MIN_FREE_DISK_BYTES,
        }
    }

    pub fn with_min_free_bytes(min_free_bytes: u64) -> Self {
        Self { min_free_bytes }
    }

    pub fn check_path(&self, path: impl AsRef<Path>) -> anyhow::Result<DiskSpaceStatus> {
        let target = disk_check_target(path.as_ref());
        let available_bytes = available_disk_bytes(&target)?;
        Ok(DiskSpaceStatus {
            target,
            available_bytes,
            min_free_bytes: self.min_free_bytes,
        })
    }
}

impl Default for DiskSpaceGuard {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskSpaceStatus {
    pub target: PathBuf,
    pub available_bytes: u64,
    pub min_free_bytes: u64,
}

impl DiskSpaceStatus {
    pub fn is_low(&self) -> bool {
        self.available_bytes < self.min_free_bytes
    }
}

// ── Main Entry Point ───────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    log::info!("═══════════════════════════════════════════════");
    log::info!("  AudraFlow Orchestrator v0.1.0");
    log::info!("  Local-first · Privacy-first · Adaptive ASR");
    log::info!("═══════════════════════════════════════════════");

    // ── Initialize Storage ─────────────────────────────────────────────────
    let db_path = get_db_path()?;
    let storage = Storage::open(&db_path)?;
    log::info!("Storage: {}", db_path.display());

    // ── Initialize Subsystems ──────────────────────────────────────────────
    let storage_arc = Arc::new(Mutex::new(storage));
    let queue = BatchQueue::new(storage_arc.clone(), 2); // Max 2 concurrent jobs
    let checkpoints = CheckpointManager::new(storage_arc.clone()).with_interval(10);
    let telemetry = TelemetryCollectorStd::new(false); // Disabled until user authorizes

    let state = Arc::new(Mutex::new(AppState {
        storage: storage_arc,
        queue,
        checkpoints,
        telemetry,
        active_jobs: HashMap::new(),
        job_plans: HashMap::new(),
        runtime_processes: HashMap::new(),
        disk_guard: DiskSpaceGuard::new(),
    }));

    log::info!("Subsystems initialized: queue, checkpoints, telemetry");

    // ── Start IPC Server ───────────────────────────────────────────────────
    let ipc_endpoint = ipc_server::default_ipc_endpoint();
    log::info!("IPC server: {}", ipc_endpoint);

    // The IPC server runs in its own task, accepting connections
    // Each connection is handled in a spawned task
    // Job processing happens in background workers

    // ── Background: Job Processor ─────────────────────────────────────────
    let processor_state = state.clone();
    tokio::spawn(async move {
        job_processor_loop(processor_state).await;
    });

    // ── Background: Checkpoint Saver ──────────────────────────────────────
    let checkpoint_state = state.clone();
    tokio::spawn(async move {
        checkpoint_saver_loop(checkpoint_state).await;
    });

    let runtime_monitor_state = state.clone();
    tokio::spawn(async move {
        runtime_monitor_loop(runtime_monitor_state).await;
    });

    // ── Run IPC Server (blocking) ─────────────────────────────────────────
    ipc_server::run_named_pipe_server(state.clone(), &ipc_endpoint).await?;

    Ok(())
}

// ── Job Processor Loop ─────────────────────────────────────────────────────

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

fn apply_glossary_to_segments(
    storage: &Storage,
    segments: Vec<Segment>,
) -> Result<Vec<Segment>, String> {
    let entries = storage
        .list_glossary_entries()
        .map_err(|error| format!("Failed to load glossary: {error}"))?
        .iter()
        .map(glossary_entry_to_processor)
        .filter(|entry| !entry.aliases.is_empty())
        .collect::<Vec<_>>();

    if entries.is_empty() {
        return Ok(segments);
    }

    let processor = PostProcessor::new(entries);
    Ok(segments
        .into_iter()
        .map(|segment| apply_glossary_to_segment(&processor, segment))
        .collect())
}

fn glossary_entry_to_processor(entry: &GlossaryEntryRow) -> GlossaryEntry {
    GlossaryEntry {
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

fn apply_glossary_to_segment(processor: &PostProcessor, mut segment: Segment) -> Segment {
    let corrected = processor.apply_to_segment(&segment);
    if corrected.corrected_text != segment.text {
        let mut corrections = corrected.auto_applied;
        corrections.extend(term_conflict_corrections(corrected.needs_confirmation));
        segment.corrections.extend(corrections);
        segment.text = corrected.corrected_text;
    } else if !corrected.needs_confirmation.is_empty()
        && !segment
            .low_confidence_reasons
            .iter()
            .any(|reason| reason == "term_conflict")
    {
        segment.low_confidence_reasons.push("term_conflict".into());
    }
    segment
}

fn term_conflict_corrections(corrections: Vec<Correction>) -> Vec<Correction> {
    corrections
        .into_iter()
        .map(|mut correction| {
            correction.auto_applied = false;
            correction
        })
        .collect()
}

struct ActiveStatusUpdate<'a> {
    job_id: &'a str,
    state: JobState,
    status_message: Option<String>,
    completed_segments: u32,
    total_segments: u32,
    last_segment_id: Option<String>,
    rtf_estimate: Option<f64>,
}

async fn update_active_status(state: &Arc<Mutex<AppState>>, update: ActiveStatusUpdate<'_>) {
    let mut app = state.lock().await;
    if let Some(active) = app.active_jobs.get_mut(update.job_id) {
        active.state = update.state;
        active.completed_segments = update.completed_segments;
        active.total_segments = update.total_segments.max(1);
        if let Some(message) = update.status_message {
            active.status_message = Some(message);
        }
        if let Some(segment_id) = update.last_segment_id {
            active.last_segment_id = Some(segment_id);
        }
        if let Some(rtf) = update.rtf_estimate {
            active.rtf_estimate = rtf;
        }
    }
}

fn validate_runtime_model_path(model_path: &str) -> Result<(), String> {
    let path = Path::new(model_path);
    let metadata =
        std::fs::metadata(path).map_err(|error| format!("ASR model is not available: {error}"))?;
    if !metadata.is_file() {
        return Err("ASR model path is not a file".into());
    }
    if metadata.len() == 0 {
        return Err("ASR model file is empty".into());
    }
    Ok(())
}

fn runtime_transcribe_command(job: &QueueItem) -> Result<tokio::process::Command, String> {
    let asr_engine = normalize_asr_engine_hint(job.asr_engine.as_deref());
    let mut command = runtime_base_command();
    command
        .arg("transcribe")
        .arg(&job.file_path)
        .arg("--engine")
        .arg(asr_engine)
        .arg("--file-hash")
        .arg(&job.file_hash)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(model_path) = job.model_path.as_deref() {
        command.arg("--model").arg(model_path);
    }
    if job.extreme_accuracy {
        command.arg("--extreme-accuracy");
    }
    if let Some(whisper_cli) = resolve_whisper_cli() {
        command.arg("--whisper-cli").arg(whisper_cli);
    }
    if let Some(language) = job.language.as_deref().and_then(normalize_language_hint) {
        command.arg("--language").arg(language);
    }
    if let Some(audio_mode) = job
        .audio_mode
        .as_deref()
        .and_then(normalize_audio_mode_hint)
    {
        command.arg("--audio-mode").arg(audio_mode);
    }
    if let Some(vocal_separation) = job
        .vocal_separation
        .as_deref()
        .and_then(normalize_vocal_separation_hint)
    {
        command.arg("--vocal-separation").arg(vocal_separation);
    }
    Ok(command)
}

fn normalize_asr_engine_hint(asr_engine: Option<&str>) -> &'static str {
    match asr_engine
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "sensevoice" | "sense_voice" => "sensevoice",
        "funasr" | "fun-asr" | "fun_asr" | "funasr-nano" | "fun-asr-nano" => "funasr",
        _ => "whisper",
    }
}

fn starting_runtime_message(job: &QueueItem) -> String {
    if job
        .vocal_separation
        .as_deref()
        .and_then(normalize_vocal_separation_hint)
        .is_some()
    {
        "Starting ASR runtime; Demucs vocal separation may run first".into()
    } else {
        "Starting ASR runtime".into()
    }
}

fn normalize_language_hint(language: &str) -> Option<String> {
    let normalized = language.trim().to_ascii_lowercase();
    if normalized.is_empty() || matches!(normalized.as_str(), "auto" | "detect" | "auto_detect") {
        None
    } else {
        Some(normalized)
    }
}

fn normalize_audio_mode_hint(audio_mode: &str) -> Option<&'static str> {
    match audio_mode.trim().to_ascii_lowercase().as_str() {
        "music" | "lyrics" | "lyric" => Some("music"),
        _ => None,
    }
}

fn normalize_vocal_separation_hint(vocal_separation: &str) -> Option<&'static str> {
    match vocal_separation.trim().to_ascii_lowercase().as_str() {
        "demucs" | "vocal" | "vocals" | "on" | "true" => Some("demucs"),
        _ => None,
    }
}

fn runtime_base_command() -> tokio::process::Command {
    if let Ok(path) =
        std::env::var("AUDRAFLOW_ASR_RUNTIME_BIN").or_else(|_| std::env::var("FT_ASR_RUNTIME_BIN"))
    {
        return tokio::process::Command::new(path);
    }

    if let Some(path) = sibling_runtime_exe() {
        return tokio::process::Command::new(path);
    }

    if let Some(workspace_root) = find_workspace_root() {
        let mut command = tokio::process::Command::new("cargo");
        command
            .current_dir(workspace_root)
            .arg("run")
            .arg("--quiet")
            .arg("--bin")
            .arg("audraflow-asr-runtime")
            .arg("--");
        return command;
    }

    tokio::process::Command::new("audraflow-asr-runtime")
}

fn sibling_runtime_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let runtime_name = if cfg!(windows) {
        "audraflow-asr-runtime.exe"
    } else {
        "audraflow-asr-runtime"
    };
    let candidate = exe.parent()?.join(runtime_name);
    candidate.is_file().then_some(candidate)
}

fn find_workspace_root() -> Option<PathBuf> {
    let mut current = std::env::current_dir().ok()?;
    loop {
        if current.join("Cargo.toml").is_file() && current.join("asr-runtime").is_dir() {
            return Some(current);
        }
        if !current.pop() {
            break;
        }
    }
    None
}

fn resolve_whisper_cli() -> Option<PathBuf> {
    whisper_cli_override()
        .map(PathBuf::from)
        .or_else(|| managed_component_binary("whisper", whisper_cli_binary_name()))
        .or_else(find_bundled_whisper_cli)
        .or_else(|| which::which(whisper_cli_binary_name()).ok())
}

fn whisper_cli_override() -> Option<String> {
    std::env::var("AUDRAFLOW_WHISPER_CLI")
        .ok()
        .or_else(|| std::env::var("FT_WHISPER_CLI").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.trim().is_empty())
}

fn find_bundled_whisper_cli() -> Option<PathBuf> {
    let mut roots = Vec::new();
    if let Some(resource_dir) = std::env::var_os("AUDRAFLOW_RESOURCE_DIR") {
        roots.push(PathBuf::from(resource_dir));
    }
    if let Ok(exe) = std::env::current_exe() {
        roots.extend(exe.ancestors().map(Path::to_path_buf));
    }
    if let Ok(cwd) = std::env::current_dir() {
        roots.extend(cwd.ancestors().map(Path::to_path_buf));
    }

    for root in roots {
        for candidate in whisper_cli_candidates(&root) {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

fn whisper_cli_candidates(root: &Path) -> Vec<PathBuf> {
    vec![
        root.join("bin").join(whisper_cli_binary_name()),
        root.join("resources")
            .join("bin")
            .join(whisper_cli_binary_name()),
        root.join("resources").join(whisper_cli_binary_name()),
        root.join(whisper_cli_binary_name()),
        root.join("external")
            .join("whisper.cpp")
            .join("build-linux")
            .join("bin")
            .join(whisper_cli_binary_name()),
        root.join("external")
            .join("whisper.cpp")
            .join("build")
            .join("bin")
            .join(whisper_cli_binary_name()),
        root.join("whisper.cpp")
            .join("build-linux")
            .join("bin")
            .join(whisper_cli_binary_name()),
        root.join("whisper.cpp")
            .join("build")
            .join("bin")
            .join(whisper_cli_binary_name()),
    ]
}

fn whisper_cli_binary_name() -> &'static str {
    if cfg!(windows) {
        "whisper-cli.exe"
    } else {
        "whisper-cli"
    }
}

fn managed_component_binary(component_id: &str, file_name: &str) -> Option<PathBuf> {
    let path = runtime_app_data_dir()
        .join("runtime")
        .join("components")
        .join(component_id)
        .join("bin")
        .join(file_name);
    path.is_file().then_some(path)
}

fn runtime_app_data_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("AUDRAFLOW_APP_DATA_DIR")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
    {
        return path;
    }

    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("com.audraflow.app");
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

fn preview_bytes(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    if trimmed.chars().count() <= 500 {
        trimmed.to_string()
    } else {
        format!("{}...", trimmed.chars().take(500).collect::<String>())
    }
}

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

fn low_disk_message(status: &DiskSpaceStatus) -> String {
    format!(
        "Paused: free disk space is below {:.0} MB (available {:.0} MB). Free disk space and resume the job.",
        bytes_to_mb(status.min_free_bytes),
        bytes_to_mb(status.available_bytes)
    )
}

fn bytes_to_mb(bytes: u64) -> f64 {
    bytes as f64 / 1024.0 / 1024.0
}

fn disk_check_target(path: &Path) -> PathBuf {
    if path.is_dir() {
        return path.to_path_buf();
    }

    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(target_os = "windows")]
fn available_disk_bytes(target: &Path) -> anyhow::Result<u64> {
    use anyhow::Context as _;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let wide: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut available_bytes = 0u64;
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available_bytes,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };

    if ok == 0 {
        Err(std::io::Error::last_os_error())
            .with_context(|| format!("failed to read free disk space for {}", target.display()))
    } else {
        Ok(available_bytes)
    }
}

#[cfg(unix)]
fn available_disk_bytes(target: &Path) -> anyhow::Result<u64> {
    use anyhow::Context as _;
    use std::ffi::CString;
    use std::mem::MaybeUninit;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(target.as_os_str().as_bytes())
        .with_context(|| format!("disk check path contains a NUL byte: {}", target.display()))?;
    let mut stat = MaybeUninit::<libc::statvfs>::uninit();
    let rc = unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) };

    if rc != 0 {
        Err(std::io::Error::last_os_error())
            .with_context(|| format!("failed to read free disk space for {}", target.display()))
    } else {
        let stat = unsafe { stat.assume_init() };
        let available = (stat.f_bavail as u128).saturating_mul(stat.f_frsize as u128);
        Ok(available.min(u64::MAX as u128) as u64)
    }
}

#[cfg(all(not(target_os = "windows"), not(unix)))]
fn available_disk_bytes(_target: &Path) -> anyhow::Result<u64> {
    Ok(u64::MAX)
}

fn get_db_path() -> anyhow::Result<PathBuf> {
    let dir = app_data_dir();
    std::fs::create_dir_all(&dir)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch_queue::{estimate_job_cost, QueueItem, QueueItemState};
    use crate::checkpoint::CheckpointManager;
    use audraflow_storage::Storage;
    use std::collections::HashMap;

    fn make_item(duration: f64) -> QueueItem {
        QueueItem {
            job_id: "job-1".into(),
            file_path: "sample.wav".into(),
            file_hash: "hash".into(),
            asr_engine: Some("whisper".into()),
            model_path: Some("model.bin".into()),
            model_name: Some("whisper-test".into()),
            model_version: Some("test".into()),
            language: Some("zh".into()),
            audio_mode: Some("speech".into()),
            vocal_separation: None,
            audio_duration_s: duration,
            extreme_accuracy: false,
            state: QueueItemState::Pending,
            cost_estimate: estimate_job_cost(duration, false, true),
            retry_count: 0,
            error_message: None,
        }
    }

    fn make_active_job() -> ActiveJob {
        ActiveJob {
            file_path: "sample.wav".into(),
            model_path: Some("model.bin".into()),
            model_name: Some("whisper-test".into()),
            model_version: Some("test".into()),
            plan_id: "test-plan".into(),
            scheduler_input: None,
            model_size: None,
            estimated_seconds: None,
            fallback_reason: None,
            extreme_accuracy: false,
            total_segments: 20,
            completed_segments: 0,
            last_segment_id: None,
            rtf_estimate: 0.08,
            recoveries: 0,
            status_message: None,
            state: JobState::Running,
        }
    }

    fn make_state(storage: Arc<Mutex<Storage>>) -> Arc<Mutex<AppState>> {
        Arc::new(Mutex::new(AppState {
            storage: storage.clone(),
            queue: BatchQueue::new(storage.clone(), 1),
            checkpoints: CheckpointManager::new(storage).with_interval(10),
            telemetry: TelemetryCollectorStd::new(false),
            active_jobs: HashMap::new(),
            job_plans: HashMap::new(),
            runtime_processes: HashMap::new(),
            disk_guard: DiskSpaceGuard::new(),
        }))
    }

    #[test]
    fn simulated_segment_count_is_bounded() {
        assert_eq!(simulated_segment_count(&make_item(0.0)), 10);
        assert_eq!(simulated_segment_count(&make_item(60.0)), 10);
        assert_eq!(simulated_segment_count(&make_item(3_600.0)), 120);
        assert_eq!(simulated_segment_count(&make_item(7_200.0)), 120);
    }

    #[test]
    fn normalize_language_hint_omits_auto_detection() {
        assert_eq!(normalize_language_hint(""), None);
        assert_eq!(normalize_language_hint(" auto "), None);
        assert_eq!(normalize_language_hint("detect"), None);
        assert_eq!(normalize_language_hint("EN"), Some("en".into()));
        assert_eq!(normalize_language_hint("zh"), Some("zh".into()));
    }

    #[test]
    fn checkpoint_state_tracks_latest_segment_progress() {
        let item = make_item(600.0);
        let state = checkpoint_state_from_job(&item, 10, 20, "sim-seg-010".into());

        assert_eq!(state.job_id, "job-1");
        assert_eq!(state.completed_segment_ids, vec!["sim-seg-010"]);
        assert_eq!(state.total_segments_processed, 10);
        assert!((state.progress - 0.5).abs() < 0.01);
    }

    #[test]
    fn validate_job_input_reports_missing_file() {
        let mut item = make_item(60.0);
        item.file_path = "definitely-missing-audraflow-input.wav".into();

        let error = validate_job_input(&item).unwrap_err();

        assert!(error.contains("Input file is not available"));
    }

    #[test]
    fn validate_job_input_reports_empty_file() {
        let mut item = make_item(60.0);
        let path = std::env::temp_dir().join(format!(
            "audraflow-empty-{}.wav",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, []).unwrap();
        item.file_path = path.to_string_lossy().into_owned();

        let error = validate_job_input(&item).unwrap_err();

        std::fs::remove_file(path).ok();
        assert!(error.contains("Input file is empty"));
    }

    #[tokio::test]
    async fn low_disk_pauses_job_and_saves_checkpoint() {
        let storage = Arc::new(Mutex::new(Storage::open_in_memory().unwrap()));
        {
            let storage = storage.lock().await;
            storage
                .create_job("job-1", "sample.wav", "hash", false)
                .unwrap();
        }
        let state = make_state(storage);
        let item = make_item(0.0);
        {
            let mut app = state.lock().await;
            app.disk_guard = DiskSpaceGuard::with_min_free_bytes(u64::MAX);
            app.queue.enqueue(item.clone());
            app.queue.dequeue_next();
            app.active_jobs.insert("job-1".into(), make_active_job());
        }

        let outcome = simulate_job_processing(&item, &state).await;

        assert_eq!(outcome, ProcessingOutcome::PausedLowDisk);
        let checkpoints = {
            let app = state.lock().await;
            let active = app.active_jobs.get("job-1").unwrap();
            assert_eq!(active.state, JobState::Paused);
            assert_eq!(active.completed_segments, 1);
            assert!(active
                .status_message
                .as_deref()
                .unwrap()
                .contains("free disk space"));

            let queued = app.queue.get_item("job-1").unwrap();
            assert_eq!(queued.state, QueueItemState::Paused);
            assert!(queued
                .error_message
                .as_deref()
                .unwrap()
                .contains("free disk space"));
            app.checkpoints.clone()
        };

        let (_, checkpoint_state) = checkpoints
            .load_latest_checkpoint("job-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(checkpoint_state.total_segments_processed, 1);
        assert_eq!(checkpoint_state.completed_segment_ids, vec!["sim-seg-001"]);
    }

    #[tokio::test]
    async fn user_cancel_stops_running_job_without_completion() {
        let storage = Arc::new(Mutex::new(Storage::open_in_memory().unwrap()));
        {
            let storage = storage.lock().await;
            storage
                .create_job("job-1", "sample.wav", "hash", false)
                .unwrap();
        }
        let state = make_state(storage);
        let item = make_item(0.0);
        {
            let mut app = state.lock().await;
            app.queue.enqueue(item.clone());
            app.queue.dequeue_next();
            app.active_jobs.insert("job-1".into(), make_active_job());
            assert!(app.queue.cancel_job("job-1"));
        }

        let outcome = simulate_job_processing(&item, &state).await;

        assert_eq!(outcome, ProcessingOutcome::Cancelled);
        let app = state.lock().await;
        assert_eq!(
            app.queue.get_item("job-1").unwrap().state,
            QueueItemState::Cancelled
        );
        let active = app.active_jobs.get("job-1").unwrap();
        assert_eq!(active.state, JobState::Cancelled);
        assert_eq!(active.completed_segments, 0);
    }

    #[tokio::test]
    async fn recover_job_restores_latest_checkpoint() {
        let storage = Arc::new(Mutex::new(Storage::open_in_memory().unwrap()));
        {
            let storage = storage.lock().await;
            storage
                .create_job("job-1", "sample.wav", "hash", false)
                .unwrap();
        }
        let state = make_state(storage.clone());
        let checkpoint_state = JobCheckpointState {
            job_id: "job-1".into(),
            file_path: "sample.wav".into(),
            completed_segment_ids: vec!["sim-seg-010".into()],
            total_segments_processed: 10,
            progress: 0.5,
            rtf_estimate: 0.12,
        };
        {
            let mut app = state.lock().await;
            app.active_jobs.insert("job-1".into(), make_active_job());
            app.runtime_processes.insert("job-1".into(), u32::MAX);
            app.checkpoints
                .save_checkpoint("job-1", "sim-seg-010", &checkpoint_state)
                .await
                .unwrap();
        }

        recover_job_from_checkpoint(state.clone(), "job-1")
            .await
            .unwrap();

        let app = state.lock().await;
        let active = app.active_jobs.get("job-1").unwrap();
        assert_eq!(active.completed_segments, 10);
        assert_eq!(active.last_segment_id.as_deref(), Some("sim-seg-010"));
        assert_eq!(active.recoveries, 1);
        assert!(active
            .status_message
            .as_deref()
            .unwrap()
            .contains("Recovered"));
        assert!(!app.runtime_processes.contains_key("job-1"));
    }

    #[tokio::test]
    async fn two_hour_audio_recovery_uses_bounded_checkpoint_state() {
        let storage = Arc::new(Mutex::new(Storage::open_in_memory().unwrap()));
        {
            let storage = storage.lock().await;
            storage
                .create_job("job-1", "two-hour.wav", "hash", false)
                .unwrap();
        }
        let state = make_state(storage.clone());
        let mut item = make_item(7_200.0);
        item.file_path = "two-hour.wav".into();
        let total_segments = simulated_segment_count(&item);
        assert_eq!(total_segments, 120);

        let checkpoint_state =
            checkpoint_state_from_job(&item, 96, total_segments, "sim-seg-096".into());
        assert_eq!(checkpoint_state.completed_segment_ids.len(), 1);
        assert!((checkpoint_state.progress - 0.8).abs() < 0.01);

        {
            let mut active = make_active_job();
            active.file_path = "two-hour.wav".into();
            active.total_segments = total_segments;
            active.completed_segments = 40;

            let mut app = state.lock().await;
            app.active_jobs.insert("job-1".into(), active);
            app.runtime_processes.insert("job-1".into(), u32::MAX);
            app.checkpoints
                .save_checkpoint("job-1", "sim-seg-096", &checkpoint_state)
                .await
                .unwrap();
        }

        recover_job_from_checkpoint(state.clone(), "job-1")
            .await
            .unwrap();

        let app = state.lock().await;
        let active = app.active_jobs.get("job-1").unwrap();
        assert_eq!(active.total_segments, 120);
        assert_eq!(active.completed_segments, 96);
        assert_eq!(active.last_segment_id.as_deref(), Some("sim-seg-096"));
        assert_eq!(active.recoveries, 1);
        assert!(active
            .status_message
            .as_deref()
            .unwrap()
            .contains("Recovered"));
    }

    #[tokio::test]
    async fn recover_job_without_checkpoint_fails_job() {
        let storage = Arc::new(Mutex::new(Storage::open_in_memory().unwrap()));
        {
            let storage = storage.lock().await;
            storage
                .create_job("job-1", "sample.wav", "hash", false)
                .unwrap();
        }
        let state = make_state(storage);
        {
            let mut app = state.lock().await;
            app.queue.enqueue(make_item(60.0));
            app.queue.dequeue_next();
            app.active_jobs.insert("job-1".into(), make_active_job());
            app.runtime_processes.insert("job-1".into(), u32::MAX);
        }

        recover_job_from_checkpoint(state.clone(), "job-1")
            .await
            .unwrap();

        let app = state.lock().await;
        let active = app.active_jobs.get("job-1").unwrap();
        assert_eq!(active.state, JobState::Failed);
        assert!(active
            .status_message
            .as_deref()
            .unwrap()
            .contains("no checkpoint"));
        assert_eq!(
            app.queue.get_item("job-1").unwrap().state,
            QueueItemState::Failed
        );
        assert!(!app.runtime_processes.contains_key("job-1"));
    }

    #[tokio::test]
    async fn gpu_oom_fallback_updates_active_plan() {
        let storage = Arc::new(Mutex::new(Storage::open_in_memory().unwrap()));
        let state = make_state(storage);
        let mut app = state.lock().await;

        let input = audraflow_scheduler::SchedulerInput {
            duration_seconds: 600.0,
            snr_db: Some(30.0),
            speech_density: Some(0.85),
            estimated_speaker_count: 1,
            is_high_noise: false,
            device_tier: audraflow_scheduler::DeviceTier::GpuStandard,
            cuda_available: true,
            vram_gb: Some(8.0),
            cpu_cores: 8,
            extreme_accuracy: false,
            model_cached: true,
            cold_start_seconds: None,
        };
        let plan = audraflow_scheduler::Scheduler::plan(&input);
        app.job_plans.insert(
            "job-1".into(),
            PlannedJob {
                scheduler_input: input,
                plan,
            },
        );
        app.active_jobs.insert("job-1".into(), make_active_job());

        assert!(apply_gpu_oom_fallback(&mut app, "job-1", "GPU OOM"));

        let active = app.active_jobs.get("job-1").unwrap();
        assert_eq!(active.fallback_reason.as_deref(), Some("GPU OOM"));
        assert!(active
            .status_message
            .as_deref()
            .unwrap()
            .contains("GPU memory"));
        assert_eq!(
            active.scheduler_input.as_ref().unwrap().device_tier,
            audraflow_scheduler::DeviceTier::CpuOnly
        );

        let planned = app.job_plans.get("job-1").unwrap();
        assert_eq!(planned.plan.fallback_reason.as_deref(), Some("GPU OOM"));
        assert_eq!(
            planned.scheduler_input.device_tier,
            audraflow_scheduler::DeviceTier::CpuOnly
        );
        assert!(!planned.scheduler_input.cuda_available);
    }
}
