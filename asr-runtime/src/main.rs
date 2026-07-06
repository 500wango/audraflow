//! AudraFlow ASR Runtime
//!
//! Local speech-to-text engine wrapping whisper.cpp.
//!
//! Architecture:
//! - Receives transcription requests from the Orchestrator via the `transcribe` subcommand.
//! - Preprocesses audio (decode, normalize, VAD, chunk).
//! - Loads whisper.cpp model (GGML format), runs inference on GPU (CUDA) or CPU.
//! - Prints time-stamped segments as JSON for the Orchestrator to persist.
//! - Supports checkpointing for long audio recovery.
//!
//! Process: standalone binary; one child process per active transcription.
//! Crash resilience: Orchestrator supervises the child process and persists checkpoint state.

use anyhow::Context;
use audio_pipeline::VadSegment;
use audraflow_ipc::Segment;
use diarization::{DiarizationInput, DiarizationOutput, DiarizationWorker};
use serde::Serialize;
use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

mod audio_pipeline;
mod benchmark;
mod diarization;
mod funasr_engine;
mod sensevoice_engine;
mod whisper_engine;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args = std::env::args().collect::<Vec<_>>();
    if args.get(1).is_some_and(|arg| arg == "transcribe") {
        return cmd_transcribe(&args[2..]);
    }
    if args
        .get(1)
        .is_some_and(|arg| matches!(arg.as_str(), "help" | "--help" | "-h"))
    {
        print_usage();
        return Ok(());
    }

    log::info!("AudraFlow ASR Runtime v0.1.0 starting...");

    // ── Device diagnostics ─────────────────────────────────────────────────
    let device_info = whisper_engine::detect_device();
    log::info!(
        "Device: cuda={}, vram_gb={:?}, cpu_cores={}",
        device_info.cuda_available,
        device_info.vram_gb,
        device_info.cpu_cores,
    );

    // ── Initialize audio pipeline ──────────────────────────────────────────
    let _pipeline = audio_pipeline::AudioPipeline::new()?;
    log::info!("Audio pipeline initialized");

    // ── Initialize whisper engine ──────────────────────────────────────────
    let _engine = whisper_engine::WhisperEngine::new(&device_info)?;
    log::info!("Whisper engine initialized");

    log::info!("ASR Runtime ready");
    log::info!(
        "Use `audraflow-asr-runtime transcribe <audio> --model <model.bin>` for production transcription"
    );
    log::info!(
        "Awaiting optional diagnostics IPC on: {}",
        r"\\.\pipe\audraflow-asr"
    );

    run_runtime_ipc_loop().await
}

include!("main_parts/types.rs");
include!("main_parts/cli.rs");
include!("main_parts/runtime_ipc.rs");
include!("main_parts/whisper_pipeline.rs");
include!("main_parts/asr_engines.rs");
include!("main_parts/demucs.rs");
include!("main_parts/diarization.rs");
include!("main_parts/music_segments.rs");
include!("main_parts/tests.rs");
