#![allow(dead_code)]

//! Batch Queue Manager
//!
//! PRD §6.2: Supports multi-file and folder import, pause/continue/retry/skip.
//! PRD §5: 100 files in queue without freezing; single failure does not block queue.
//!
//! Queue ordering: by estimated processing cost (shortest first),
//! considering model cold-start cost and audio duration.

use audraflow_storage::Storage;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::Mutex;

// ── Queue Item ─────────────────────────────────────────────────────────────

/// A single item in the processing queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueItem {
    pub job_id: String,
    pub file_path: String,
    pub file_hash: String,
    pub asr_engine: Option<String>,
    pub model_path: Option<String>,
    pub model_name: Option<String>,
    pub model_version: Option<String>,
    pub language: Option<String>,
    pub audio_mode: Option<String>,
    pub vocal_separation: Option<String>,
    pub audio_duration_s: f64,
    pub extreme_accuracy: bool,
    pub state: QueueItemState,
    /// Estimated processing cost (lower = processed first).
    pub cost_estimate: f64,
    /// Number of retry attempts.
    pub retry_count: u32,
    /// Error message if failed.
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum QueueItemState {
    Pending,
    Running,
    Paused,
    Completed,
    Cancelled,
    Failed,
    Skipped,
}

// ── Batch Queue ────────────────────────────────────────────────────────────

/// Manages a batch processing queue for multiple transcription jobs.
pub struct BatchQueue {
    items: VecDeque<QueueItem>,
    /// Maximum concurrent jobs.
    max_concurrent: usize,
    /// Storage handle for persistence.
    storage: Arc<Mutex<Storage>>,
}

impl BatchQueue {
    /// Create a new batch queue.
    pub fn new(storage: Arc<Mutex<Storage>>, max_concurrent: usize) -> Self {
        Self {
            items: VecDeque::new(),
            max_concurrent,
            storage,
        }
    }

    /// Add a job to the queue.
    pub fn enqueue(&mut self, item: QueueItem) {
        log::info!("Enqueued job {}: {}", item.job_id, item.file_path);
        self.items.push_back(item);
        self.reorder();
    }

    /// Add multiple jobs at once (e.g., from folder import).
    pub fn enqueue_batch(&mut self, items: Vec<QueueItem>) {
        log::info!("Enqueued {} jobs in batch", items.len());
        self.items.extend(items);
        self.reorder();
    }

    /// Remove a job from the queue.
    pub fn remove(&mut self, job_id: &str) -> Option<QueueItem> {
        if let Some(pos) = self.items.iter().position(|i| i.job_id == job_id) {
            let item = self.items.remove(pos).unwrap();
            log::info!("Removed job {} from queue", job_id);
            Some(item)
        } else {
            None
        }
    }

    /// Pause a specific job.
    pub fn pause_job(&mut self, job_id: &str) -> bool {
        if let Some(item) = self.items.iter_mut().find(|i| i.job_id == job_id) {
            if matches!(
                item.state,
                QueueItemState::Pending | QueueItemState::Running
            ) {
                item.state = QueueItemState::Paused;
                item.error_message = None;
                return true;
            }
        }
        false
    }

    /// Pause a specific job and keep a user-visible reason.
    pub fn pause_job_with_message(&mut self, job_id: &str, message: &str) -> bool {
        if let Some(item) = self.items.iter_mut().find(|i| i.job_id == job_id) {
            item.state = QueueItemState::Paused;
            item.error_message = Some(message.to_string());
            true
        } else {
            false
        }
    }

    /// Resume a paused job.
    pub fn resume_job(&mut self, job_id: &str) -> bool {
        if let Some(item) = self.items.iter_mut().find(|i| i.job_id == job_id) {
            if item.state == QueueItemState::Paused {
                item.state = QueueItemState::Pending;
                item.error_message = None;
                self.reorder();
                return true;
            }
        }
        false
    }

    /// Retry a failed job.
    pub fn retry_job(&mut self, job_id: &str) -> bool {
        if let Some(item) = self.items.iter_mut().find(|i| i.job_id == job_id) {
            if matches!(
                item.state,
                QueueItemState::Cancelled | QueueItemState::Failed | QueueItemState::Skipped
            ) {
                item.state = QueueItemState::Pending;
                item.retry_count += 1;
                item.error_message = None;
                log::info!("Retrying job {} (attempt {})", job_id, item.retry_count);
                self.reorder();
                return true;
            }
        }
        false
    }

    /// Cancel a job at the user's request.
    pub fn cancel_job(&mut self, job_id: &str) -> bool {
        if let Some(item) = self.items.iter_mut().find(|i| i.job_id == job_id) {
            if matches!(
                item.state,
                QueueItemState::Pending
                    | QueueItemState::Running
                    | QueueItemState::Paused
                    | QueueItemState::Failed
            ) {
                item.state = QueueItemState::Cancelled;
                item.error_message = Some("Cancelled".into());
                log::info!("Cancelled job {}", job_id);
                return true;
            }
        }
        false
    }

    /// Skip a failed job (mark as skipped, don't retry).
    pub fn skip_job(&mut self, job_id: &str) -> bool {
        if let Some(item) = self.items.iter_mut().find(|i| i.job_id == job_id) {
            if matches!(
                item.state,
                QueueItemState::Pending
                    | QueueItemState::Running
                    | QueueItemState::Paused
                    | QueueItemState::Failed
            ) {
                item.state = QueueItemState::Skipped;
                item.error_message = Some("Skipped".into());
                log::info!("Skipped job {}", job_id);
                return true;
            }
        }
        false
    }

    /// Get the next job to process (first pending item).
    pub fn dequeue_next(&mut self) -> Option<QueueItem> {
        if self.active_count() >= self.max_concurrent {
            return None; // At max concurrency
        }

        if let Some(pos) = self
            .items
            .iter()
            .position(|i| i.state == QueueItemState::Pending)
        {
            let mut item = self.items.remove(pos).unwrap();
            item.state = QueueItemState::Running;
            let job_id = item.job_id.clone();
            self.items.push_back(item);
            log::info!("Started job {} from queue", job_id);
            // Return the item that was started
            self.items.back().cloned()
        } else {
            None
        }
    }

    /// Mark a job as completed.
    pub fn complete_job(&mut self, job_id: &str) {
        if let Some(item) = self.items.iter_mut().find(|i| i.job_id == job_id) {
            if item.state == QueueItemState::Running {
                item.state = QueueItemState::Completed;
                log::info!("Job {} completed", job_id);
            }
        }
    }

    /// Mark a job as failed with an error message.
    pub fn fail_job(&mut self, job_id: &str, error: &str) {
        if let Some(item) = self.items.iter_mut().find(|i| i.job_id == job_id) {
            if matches!(
                item.state,
                QueueItemState::Completed | QueueItemState::Cancelled | QueueItemState::Skipped
            ) {
                return;
            }
            item.state = QueueItemState::Failed;
            item.error_message = Some(error.to_string());
        }
        log::warn!("Job {} failed: {}", job_id, error);
    }

    // ── Queries ────────────────────────────────────────────────────────────

    /// Number of jobs currently running.
    pub fn active_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| i.state == QueueItemState::Running)
            .count()
    }

    /// Total number of jobs in the queue.
    pub fn total_count(&self) -> usize {
        self.items.len()
    }

    /// Number of completed jobs.
    pub fn completed_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| i.state == QueueItemState::Completed)
            .count()
    }

    /// Number of failed jobs.
    pub fn failed_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| i.state == QueueItemState::Failed)
            .count()
    }

    /// Get all items (for UI display).
    pub fn all_items(&self) -> Vec<&QueueItem> {
        self.items.iter().collect()
    }

    /// Get one queue item by job id.
    pub fn get_item(&self, job_id: &str) -> Option<&QueueItem> {
        self.items.iter().find(|item| item.job_id == job_id)
    }

    /// Get batch statistics.
    pub fn batch_stats(&self) -> BatchStats {
        let total = self.total_count();
        let completed = self.completed_count();
        let failed = self.failed_count();
        let pending = self
            .items
            .iter()
            .filter(|i| i.state == QueueItemState::Pending)
            .count();
        let running = self.active_count();

        BatchStats {
            total,
            completed,
            failed,
            pending,
            running,
            progress_pct: if total > 0 {
                (completed as f64 / total as f64) * 100.0
            } else {
                0.0
            },
        }
    }

    /// Check if the queue is fully processed (all jobs completed, failed, or skipped).
    pub fn is_done(&self) -> bool {
        self.items.iter().all(|i| {
            matches!(
                i.state,
                QueueItemState::Completed
                    | QueueItemState::Cancelled
                    | QueueItemState::Failed
                    | QueueItemState::Skipped
            )
        })
    }

    // ── Private ─────────────────────────────────────────────────────────────

    fn update_state(&mut self, job_id: &str, state: QueueItemState) -> bool {
        if let Some(item) = self.items.iter_mut().find(|i| i.job_id == job_id) {
            item.state = state;
            true
        } else {
            false
        }
    }

    /// Reorder queue: shortest/cheapest jobs first.
    /// PRD §6.2: sort by duration, noise, device load, and model cold start cost.
    fn reorder(&mut self) {
        // Use a simple heuristic: shorter audio first
        // Pending items sorted by cost_estimate ascending
        let items: Vec<QueueItem> = self.items.drain(..).collect();

        // Stable sort: running jobs stay in place, pending jobs sorted by cost
        let (running_and_done, mut pending): (Vec<QueueItem>, Vec<QueueItem>) = items
            .into_iter()
            .partition(|i| i.state != QueueItemState::Pending);

        pending.sort_by(|a, b| {
            a.cost_estimate
                .partial_cmp(&b.cost_estimate)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Running/done items first (preserve order), then sorted pending
        self.items = VecDeque::from(running_and_done);
        self.items.extend(pending);
    }
}

// ── Batch Statistics ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchStats {
    pub total: usize,
    pub completed: usize,
    pub failed: usize,
    pub pending: usize,
    pub running: usize,
    pub progress_pct: f64,
}

// ── Folder Scanner ─────────────────────────────────────────────────────────

/// Scan a directory recursively for supported audio/video files.
pub fn scan_folder_for_media(folder_path: &str) -> Vec<String> {
    let supported_extensions = [
        "mp3", "wav", "m4a", "mp4", "mov", "aac", "flac", "ogg", "wma", "opus",
    ];

    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(folder_path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(scan_folder_for_media(&path.display().to_string()));
            } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if supported_extensions.contains(&ext.to_lowercase().as_str()) {
                    files.push(path.display().to_string());
                }
            }
        }
    }

    files
}

/// Estimate processing cost for a job (used for queue ordering).
/// Lower cost = processed first.
pub fn estimate_job_cost(audio_duration_s: f64, extreme_accuracy: bool, model_cached: bool) -> f64 {
    let base_cost = audio_duration_s;
    let accuracy_multiplier = if extreme_accuracy { 2.5 } else { 1.0 };
    let cold_start_penalty = if !model_cached { 30.0 } else { 0.0 }; // 30s penalty for model download
    base_cost * accuracy_multiplier + cold_start_penalty
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use audraflow_storage::Storage;

    fn make_item(id: &str, duration: f64, extreme: bool) -> QueueItem {
        QueueItem {
            job_id: id.to_string(),
            file_path: format!("/test/{}.mp3", id),
            file_hash: "abc123".to_string(),
            asr_engine: Some("whisper".into()),
            model_path: Some("model.bin".into()),
            model_name: Some("whisper-test".into()),
            model_version: Some("test".into()),
            language: Some("zh".into()),
            audio_mode: Some("speech".into()),
            vocal_separation: None,
            audio_duration_s: duration,
            extreme_accuracy: extreme,
            state: QueueItemState::Pending,
            cost_estimate: estimate_job_cost(duration, extreme, true),
            retry_count: 0,
            error_message: None,
        }
    }

    #[tokio::test]
    async fn test_enqueue_and_reorder() {
        let storage = Storage::open_in_memory().unwrap();
        let storage = Arc::new(Mutex::new(storage));
        let mut queue = BatchQueue::new(storage, 2);

        // Add 3 jobs: long, medium, short
        queue.enqueue(make_item("j1", 3600.0, false)); // 1 hour
        queue.enqueue(make_item("j2", 300.0, false)); // 5 min
        queue.enqueue(make_item("j3", 1800.0, false)); // 30 min

        // Shortest should be first
        let items = queue.all_items();
        let first_pending = items.iter().find(|i| i.state == QueueItemState::Pending);
        assert!(first_pending.is_some());
        assert_eq!(first_pending.unwrap().job_id, "j2"); // 5 min job first
    }

    #[tokio::test]
    async fn test_enqueue_100_jobs_keeps_pending_sorted() {
        let storage = Storage::open_in_memory().unwrap();
        let storage = Arc::new(Mutex::new(storage));
        let mut queue = BatchQueue::new(storage, 4);

        let items = (0..100)
            .rev()
            .map(|i| make_item(&format!("j{i:03}"), (i + 1) as f64 * 10.0, false))
            .collect();
        queue.enqueue_batch(items);

        assert_eq!(queue.total_count(), 100);
        let pending_ids = queue
            .all_items()
            .into_iter()
            .filter(|item| item.state == QueueItemState::Pending)
            .map(|item| item.job_id.clone())
            .collect::<Vec<_>>();

        assert_eq!(pending_ids.first().map(String::as_str), Some("j000"));
        assert_eq!(pending_ids.last().map(String::as_str), Some("j099"));
    }

    #[tokio::test]
    async fn test_dequeue_next_respects_concurrency() {
        let storage = Storage::open_in_memory().unwrap();
        let storage = Arc::new(Mutex::new(storage));
        let mut queue = BatchQueue::new(storage, 1); // Max 1 concurrent

        queue.enqueue(make_item("j1", 100.0, false));
        queue.enqueue(make_item("j2", 200.0, false));

        let first = queue.dequeue_next();
        assert!(first.is_some());
        assert_eq!(first.unwrap().job_id, "j1");

        // Second should not start because max_concurrent=1
        let second = queue.dequeue_next();
        assert!(second.is_none());

        // Complete first, then second can start
        queue.complete_job("j1");
        let second = queue.dequeue_next();
        assert!(second.is_some());
        assert_eq!(second.unwrap().job_id, "j2");
    }

    #[tokio::test]
    async fn test_fail_job_does_not_block_queue() {
        let storage = Storage::open_in_memory().unwrap();
        let storage = Arc::new(Mutex::new(storage));
        let mut queue = BatchQueue::new(storage, 2);

        queue.enqueue(make_item("j1", 100.0, false));
        queue.enqueue(make_item("j2", 200.0, false));
        queue.enqueue(make_item("j3", 300.0, false));

        // Start j1
        queue.dequeue_next();
        queue.fail_job("j1", "Corrupt file");

        // j2 should still be pending and available
        let next = queue.dequeue_next();
        assert!(next.is_some());
        assert_eq!(next.unwrap().job_id, "j2");

        // Batch stats should show 1 failed, not blocked
        let stats = queue.batch_stats();
        assert_eq!(stats.failed, 1);
        assert!(stats.progress_pct >= 0.0);
    }

    #[tokio::test]
    async fn test_retry_failed_job() {
        let storage = Storage::open_in_memory().unwrap();
        let storage = Arc::new(Mutex::new(storage));
        let mut queue = BatchQueue::new(storage, 2);

        queue.enqueue(make_item("j1", 100.0, false));
        queue.dequeue_next();
        queue.fail_job("j1", "Timeout");

        assert!(queue.retry_job("j1"));
        let items = queue.all_items();
        let item = items.iter().find(|i| i.job_id == "j1").unwrap();
        assert_eq!(item.state, QueueItemState::Pending);
        assert_eq!(item.retry_count, 1);
    }

    #[tokio::test]
    async fn test_user_control_state_transitions_are_guarded() {
        let storage = Storage::open_in_memory().unwrap();
        let storage = Arc::new(Mutex::new(storage));
        let mut queue = BatchQueue::new(storage, 1);

        queue.enqueue(make_item("j1", 100.0, false));
        assert!(queue.pause_job("j1"));
        assert_eq!(queue.get_item("j1").unwrap().state, QueueItemState::Paused);

        assert!(queue.resume_job("j1"));
        assert_eq!(queue.get_item("j1").unwrap().state, QueueItemState::Pending);

        queue.dequeue_next();
        assert!(queue.cancel_job("j1"));
        assert_eq!(
            queue.get_item("j1").unwrap().state,
            QueueItemState::Cancelled
        );

        queue.complete_job("j1");
        assert_eq!(
            queue.get_item("j1").unwrap().state,
            QueueItemState::Cancelled
        );

        assert!(queue.retry_job("j1"));
        assert_eq!(queue.get_item("j1").unwrap().state, QueueItemState::Pending);

        queue.dequeue_next();
        assert!(queue.skip_job("j1"));
        assert_eq!(queue.get_item("j1").unwrap().state, QueueItemState::Skipped);
        assert!(!queue.pause_job("j1"));
        assert!(!queue.resume_job("j1"));
    }

    #[tokio::test]
    async fn test_get_item_returns_current_state() {
        let storage = Storage::open_in_memory().unwrap();
        let storage = Arc::new(Mutex::new(storage));
        let mut queue = BatchQueue::new(storage, 1);

        queue.enqueue(make_item("j1", 100.0, false));
        assert_eq!(queue.get_item("j1").unwrap().state, QueueItemState::Pending);
        queue.dequeue_next();
        assert_eq!(queue.get_item("j1").unwrap().state, QueueItemState::Running);
        assert!(queue.get_item("missing").is_none());
    }

    #[tokio::test]
    async fn test_batch_stats() {
        let storage = Storage::open_in_memory().unwrap();
        let storage = Arc::new(Mutex::new(storage));
        let mut queue = BatchQueue::new(storage, 2);

        queue.enqueue(make_item("j1", 100.0, false));
        queue.enqueue(make_item("j2", 200.0, false));
        queue.enqueue(make_item("j3", 300.0, false));

        queue.dequeue_next();
        queue.complete_job("j1");

        let stats = queue.batch_stats();
        assert_eq!(stats.total, 3);
        assert_eq!(stats.completed, 1);
        assert!((stats.progress_pct - 100.0 / 3.0).abs() < 0.01);
    }

    #[test]
    fn test_folder_scanner() {
        // Test with non-existent directory
        let files = scan_folder_for_media("/nonexistent/path");
        assert!(files.is_empty());
    }

    #[test]
    fn test_estimate_job_cost() {
        let cost_short = estimate_job_cost(300.0, false, true);
        let cost_long = estimate_job_cost(3600.0, false, true);
        assert!(cost_short < cost_long);

        let cost_extreme = estimate_job_cost(300.0, true, true);
        assert!(cost_extreme > cost_short);

        let cost_cold = estimate_job_cost(300.0, false, false);
        assert!(cost_cold > cost_short); // Cold start penalty
    }
}
