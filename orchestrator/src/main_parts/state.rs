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
