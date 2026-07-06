//! Checkpoint Manager — Crash Recovery
//!
//! PRD §13.3: Orchestrator persists job state at regular intervals.
//! On ASR Runtime crash, the Orchestrator detects the crash,
//! restarts the Runtime, and resumes from the latest checkpoint.
//!
//! Checkpoint data (PRD §13.4):
//! - job_id, checkpoint_id, last_segment_id, timestamp
//! - Serialized job state for recovery

use audraflow_ipc::CheckpointEvent;
use audraflow_storage::Storage;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

// ── Checkpoint State ───────────────────────────────────────────────────────

/// Serializable job state saved at each checkpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobCheckpointState {
    pub job_id: String,
    pub file_path: String,
    /// Segments completed so far (for recovery: skip these on restart).
    pub completed_segment_ids: Vec<String>,
    /// Total segments processed.
    pub total_segments_processed: u32,
    /// Progress percentage (0.0–1.0).
    pub progress: f64,
    /// Current RTF estimate.
    pub rtf_estimate: f64,
}

// ── Checkpoint Manager ─────────────────────────────────────────────────────

/// Manages checkpoint save and restore for the Orchestrator.
#[derive(Clone)]
pub struct CheckpointManager {
    storage: Arc<Mutex<Storage>>,
    /// How often to save checkpoints (in number of segments processed).
    save_interval_segments: u32,
}

impl CheckpointManager {
    pub fn new(storage: Arc<Mutex<Storage>>) -> Self {
        Self {
            storage,
            save_interval_segments: 10, // Save every 10 segments
        }
    }

    pub fn with_interval(mut self, interval: u32) -> Self {
        self.save_interval_segments = interval;
        self
    }

    /// Determine if a checkpoint should be saved based on segment count.
    pub fn should_save(&self, segments_processed: u32) -> bool {
        self.save_interval_segments > 0
            && segments_processed > 0
            && segments_processed.is_multiple_of(self.save_interval_segments)
    }

    /// Save a checkpoint for a job.
    pub async fn save_checkpoint(
        &self,
        job_id: &str,
        last_segment_id: &str,
        state: &JobCheckpointState,
    ) -> anyhow::Result<String> {
        let checkpoint_id = format!(
            "{}-{}",
            job_id,
            uuid::Uuid::new_v4()
                .to_string()
                .split('-')
                .next()
                .unwrap_or("ckpt")
        );

        let state_blob = serde_json::to_vec(state)?;

        let storage = self.storage.lock().await;
        storage.save_checkpoint(&checkpoint_id, job_id, last_segment_id, &state_blob)?;

        log::info!(
            "Checkpoint saved: {} (segments: {}, progress: {:.1}%)",
            checkpoint_id,
            state.total_segments_processed,
            state.progress * 100.0,
        );

        Ok(checkpoint_id)
    }

    /// Load the latest checkpoint for a job.
    pub async fn load_latest_checkpoint(
        &self,
        job_id: &str,
    ) -> anyhow::Result<Option<(CheckpointEvent, JobCheckpointState)>> {
        let storage = self.storage.lock().await;
        let record = storage.get_latest_checkpoint(job_id)?;

        match record {
            Some(rec) => {
                let state: JobCheckpointState = serde_json::from_slice(&rec.state_blob)?;

                let event = CheckpointEvent {
                    job_id: job_id.to_string(),
                    checkpoint_id: rec.checkpoint_id,
                    last_segment_id: rec.last_segment_id,
                    timestamp: chrono::Utc::now().timestamp_millis(),
                };

                log::info!(
                    "Checkpoint loaded: {} (segments: {}, progress: {:.1}%)",
                    event.checkpoint_id,
                    state.total_segments_processed,
                    state.progress * 100.0,
                );

                Ok(Some((event, state)))
            }
            None => {
                log::info!("No checkpoint found for job: {}", job_id);
                Ok(None)
            }
        }
    }

    /// Check if the ASR Runtime process is still alive.
    /// Returns Ok if alive, Err if crashed or unresponsive.
    pub fn check_runtime_alive(&self, runtime_pid: Option<u32>) -> anyhow::Result<()> {
        if let Some(pid) = runtime_pid {
            #[cfg(target_os = "windows")]
            {
                // On Windows, check if process handle is signaled
                let handle = unsafe {
                    windows_sys::Win32::System::Threading::OpenProcess(
                        windows_sys::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION,
                        0,
                        pid,
                    )
                };
                if handle.is_null() {
                    anyhow::bail!("Runtime process {} not found", pid);
                }
                unsafe {
                    let _ = windows_sys::Win32::Foundation::CloseHandle(handle);
                }
                Ok(())
            }
            #[cfg(not(target_os = "windows"))]
            {
                // Unix: check /proc
                let path = format!("/proc/{}", pid);
                if std::path::Path::new(&path).exists() {
                    Ok(())
                } else {
                    anyhow::bail!("Runtime process {} not found", pid)
                }
            }
        } else {
            // No PID tracked → assume alive
            Ok(())
        }
    }

    /// Per-job ASR runtimes are restarted by the job processor after it has
    /// persisted the latest checkpoint state.
    pub fn log_runtime_restart(&self, job_id: &str, checkpoint_id: &str) {
        log::info!(
            "Runtime restart requested for job {} from checkpoint {}",
            job_id,
            checkpoint_id
        );
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use audraflow_storage::Storage;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[tokio::test]
    async fn test_save_and_load_checkpoint() {
        let storage = Storage::open_in_memory().unwrap();
        let storage = Arc::new(Mutex::new(storage));

        // Create a job first
        {
            let s = storage.lock().await;
            s.create_job("job-1", "test.wav", "abc123", false).unwrap();
        }

        let manager = CheckpointManager::new(storage.clone());

        let state = JobCheckpointState {
            job_id: "job-1".to_string(),
            file_path: "test.wav".to_string(),
            completed_segment_ids: vec!["seg-00".to_string(), "seg-01".to_string()],
            total_segments_processed: 20,
            progress: 0.5,
            rtf_estimate: 0.08,
        };

        let ckpt_id = manager
            .save_checkpoint("job-1", "seg-19", &state)
            .await
            .unwrap();
        assert!(!ckpt_id.is_empty());

        let loaded = manager.load_latest_checkpoint("job-1").await.unwrap();
        assert!(loaded.is_some());
        let (event, restored_state) = loaded.unwrap();
        assert_eq!(event.job_id, "job-1");
        assert_eq!(event.last_segment_id, "seg-19");
        assert_eq!(restored_state.total_segments_processed, 20);
        assert!((restored_state.progress - 0.5).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_no_checkpoint_for_new_job() {
        let storage = Storage::open_in_memory().unwrap();
        let storage = Arc::new(Mutex::new(storage));
        {
            let s = storage.lock().await;
            s.create_job("job-2", "test2.wav", "def456", false).unwrap();
        }

        let manager = CheckpointManager::new(storage);
        let loaded = manager.load_latest_checkpoint("job-2").await.unwrap();
        assert!(loaded.is_none());
    }

    #[test]
    fn test_should_save_interval() {
        let storage = Storage::open_in_memory().unwrap();
        let storage = Arc::new(Mutex::new(storage));
        let manager = CheckpointManager::new(storage).with_interval(5);

        assert!(!manager.should_save(0));
        assert!(!manager.should_save(3));
        assert!(manager.should_save(5));
        assert!(manager.should_save(10));
        assert!(!manager.should_save(11));
        assert!(manager.should_save(15));
    }
}
