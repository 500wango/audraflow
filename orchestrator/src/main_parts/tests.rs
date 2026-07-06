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
