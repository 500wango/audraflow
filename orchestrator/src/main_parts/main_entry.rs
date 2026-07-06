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
