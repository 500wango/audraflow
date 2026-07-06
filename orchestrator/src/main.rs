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

include!("main_parts/state.rs");
include!("main_parts/main_entry.rs");
include!("main_parts/job_processor.rs");
include!("main_parts/runtime_execution.rs");
include!("main_parts/glossary.rs");
include!("main_parts/runtime_command.rs");
include!("main_parts/test_simulation.rs");
include!("main_parts/background_loops.rs");
include!("main_parts/checkpoint_recovery.rs");
include!("main_parts/disk_paths.rs");
include!("main_parts/tests.rs");
