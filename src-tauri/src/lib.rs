//! AudraFlow Tauri Application
//!
//! Desktop app providing the UI layer for the AudraFlow product.
//! Communicates with the Orchestrator and ASR Runtime via local IPC.

use audraflow_ipc::{
    Correction, CorrectionSource, IpcEnvelope, IpcMessage, JobControl, JobCreate, JobPlan,
    JobState, JobStatus, Segment, TimestampMark,
};
use audraflow_licensing::{LicenseManager, LicenseState};
use audraflow_scheduler::{DeviceTier, Scheduler, SchedulerInput};
use audraflow_storage::SegmentRow;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
#[allow(unused_imports)]
use tauri::{Emitter, Manager};
use tokio::io::AsyncWriteExt;

#[cfg(target_os = "windows")]
pub(crate) const ORCHESTRATOR_PIPE: &str = r"\\.\pipe\audraflow-orchestrator";
/// Maximum remote media download size (2 GB).
pub(crate) const MAX_REMOTE_MEDIA_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Timeout for direct HTTP media downloads (5 minutes).
pub(crate) const REMOTE_MEDIA_TIMEOUT_SECS: u64 = 300;
/// Timeout for yt-dlp / platform media downloads (15 minutes).
pub(crate) const PLATFORM_DOWNLOAD_TIMEOUT_SECS: u64 = 900;
/// Maximum seconds to wait for orchestrator to become available.
pub(crate) const ORCHESTRATOR_STARTUP_TIMEOUT_SECS: u64 = 8;
/// Maximum intro-skip duration (12 hours — effective full-file skip).
pub(crate) const MAX_SKIP_START_SECONDS: f64 = 12.0 * 60.0 * 60.0;
/// Default preview duration for URL imports (2 minutes).
pub(crate) const DEFAULT_URL_PREVIEW_SECONDS: f64 = 120.0;
/// Maximum URL preview duration (5 minutes).
pub(crate) const MAX_URL_PREVIEW_SECONDS: f64 = 300.0;
/// Timeout for URL preview downloads (4 minutes).
pub(crate) const URL_PREVIEW_TIMEOUT_SECS: u64 = 240;
pub(crate) const WHISPER_CPP_MODEL_COMMIT: &str = "5359861c739e955e79d9a303bcbc70fb988958b1";
pub(crate) const WHISPER_CPP_MODEL_BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve";
pub(crate) const BUNDLED_DEFAULT_MODEL_RESOURCE: &str = "default-models/ggml-base.bin";
pub(crate) const DEFAULT_WHISPER_MODEL_NAME: &str = "base";
pub(crate) const DEFAULT_WHISPER_MODEL_SIZE_BYTES: u64 = 147_951_465;
pub(crate) const DEFAULT_WHISPER_MODEL_SHA256: &str =
    "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe";

/// Internal implementation module — items are re-exported into the crate root.
mod lib_parts {
    #![allow(unused_imports)]
    include!("lib_parts/dto.rs");
    include!("lib_parts/ipc_storage.rs");
    include!("lib_parts/telemetry_models.rs");
    include!("lib_parts/runtime_components.rs");
    include!("lib_parts/device_health.rs");
    include!("lib_parts/runtime_dependency_probes.rs");
    include!("lib_parts/python_runtime.rs");
    include!("lib_parts/tool_paths.rs");
    include!("lib_parts/media_support.rs");
    include!("lib_parts/media_download.rs");
    include!("lib_parts/job_commands.rs");
    include!("lib_parts/settings_commands.rs");
    include!("lib_parts/export_commands.rs");
    include!("lib_parts/license_commands.rs");
    include!("lib_parts/tests.rs");
}
pub use lib_parts::*;
