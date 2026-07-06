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

#[derive(Debug)]
struct RuntimeTranscribeConfig {
    input_path: PathBuf,
    model_path: Option<PathBuf>,
    whisper_cli: PathBuf,
    language: String,
    file_hash: String,
    asr_engine: AsrEngine,
    extreme_accuracy: bool,
    audio_mode: AudioMode,
    vocal_separation: VocalSeparationMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsrEngine {
    Whisper,
    SenseVoice,
    FunAsr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioMode {
    Speech,
    Music,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VocalSeparationMode {
    Off,
    Demucs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SenseVoiceChunkingPlan {
    max_chunk_ms: i64,
    overlap_ms: i64,
    internal_vad: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeTranscribeOutput {
    segments: Vec<Segment>,
    audio_duration_s: f64,
    rtf: f64,
    ttfv_s: f64,
    chunk_count: u32,
    preprocess_messages: Vec<String>,
}

fn print_usage() {
    eprintln!(
        "Usage: audraflow-asr-runtime transcribe <audio> [--engine whisper|sensevoice|funasr] [--model <model.bin>] [--whisper-cli <path>] [--language auto|zh|en] [--file-hash <sha256>] [--extreme-accuracy] [--audio-mode speech|music] [--vocal-separation off|demucs]"
    );
}

fn cmd_transcribe(args: &[String]) -> anyhow::Result<()> {
    let config = parse_runtime_transcribe_args(args)?;
    let device_info = whisper_engine::detect_device();
    let pipeline = audio_pipeline::AudioPipeline::new()?;
    let result = match config.asr_engine {
        AsrEngine::FunAsr => transcribe_file_with_funasr_sync(
            &pipeline,
            &config.input_path,
            &config.file_hash,
            &config.language,
            config.extreme_accuracy,
            config.audio_mode,
            config.vocal_separation,
        )?,
        AsrEngine::SenseVoice => {
            match transcribe_file_with_sensevoice_sync(
                &pipeline,
                &config.input_path,
                &config.file_hash,
                &config.language,
                config.extreme_accuracy,
                config.audio_mode,
                config.vocal_separation,
            ) {
                Ok(result) => result,
                Err(error) if config.model_path.is_some() => {
                    log::warn!("SenseVoice failed; falling back to Whisper: {error}");
                    let mut result =
                        transcribe_file_with_whisper_sync(&device_info, &pipeline, &config)?;
                    result.preprocess_messages.insert(
                        0,
                        format!(
                            "SenseVoice failed; fell back to Whisper: {}",
                            truncate_message(&error.to_string(), 180)
                        ),
                    );
                    result
                }
                Err(error) => return Err(error),
            }
        }
        AsrEngine::Whisper => transcribe_file_with_whisper_sync(&device_info, &pipeline, &config)?,
    };
    let output = RuntimeTranscribeOutput {
        segments: result.segments,
        audio_duration_s: result.audio_info.duration_seconds,
        rtf: result.rtf,
        ttfv_s: result.ttfv_s,
        chunk_count: result.chunk_count,
        preprocess_messages: result.preprocess_messages,
    };
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

fn transcribe_file_with_whisper_sync(
    device_info: &whisper_engine::DeviceInfo,
    pipeline: &audio_pipeline::AudioPipeline,
    config: &RuntimeTranscribeConfig,
) -> anyhow::Result<TranscriptionResult> {
    let model_path = config
        .model_path
        .clone()
        .context("Missing --model <path> for Whisper engine")?;
    let engine = whisper_engine::WhisperEngine::new(device_info)?
        .with_model(model_path)
        .with_whisper_cli(config.whisper_cli.clone())
        .with_language(config.language.clone())
        .with_lyrics_mode(config.audio_mode == AudioMode::Music);
    transcribe_file_pipeline_sync(
        &engine,
        pipeline,
        &config.input_path,
        &config.file_hash,
        config.extreme_accuracy,
        config.audio_mode,
        config.vocal_separation,
    )
}

fn parse_runtime_transcribe_args(args: &[String]) -> anyhow::Result<RuntimeTranscribeConfig> {
    let mut input: Option<String> = None;
    let mut model: Option<String> = None;
    let mut whisper_cli: Option<String> = None;
    let mut language = "auto".to_string();
    let mut file_hash = String::new();
    let mut asr_engine = AsrEngine::SenseVoice;
    let mut extreme_accuracy = false;
    let mut audio_mode = AudioMode::Speech;
    let mut vocal_separation = VocalSeparationMode::Off;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--engine" | "--asr-engine" => {
                asr_engine = parse_asr_engine(&take_arg_value(args, &mut i, "--engine")?)?;
            }
            "--model" | "-m" => model = Some(take_arg_value(args, &mut i, "--model")?),
            "--whisper-cli" => {
                whisper_cli = Some(take_arg_value(args, &mut i, "--whisper-cli")?);
            }
            "--language" | "-l" => {
                language = take_arg_value(args, &mut i, "--language")?;
            }
            "--file-hash" => {
                file_hash = take_arg_value(args, &mut i, "--file-hash")?;
            }
            "--extreme-accuracy" => extreme_accuracy = true,
            "--audio-mode" => {
                audio_mode = parse_audio_mode(&take_arg_value(args, &mut i, "--audio-mode")?)?;
            }
            "--vocal-separation" => {
                vocal_separation = parse_vocal_separation_mode(&take_arg_value(
                    args,
                    &mut i,
                    "--vocal-separation",
                )?)?;
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            arg if arg.starts_with('-') => anyhow::bail!("Unknown transcribe option: {arg}"),
            arg if input.is_none() => input = Some(arg.to_string()),
            arg => anyhow::bail!("Unexpected transcribe argument: {arg}"),
        }
        i += 1;
    }

    let input_path = PathBuf::from(input.context("Missing input audio path")?);
    ensure_file(&input_path, "input audio")?;
    let model_path = model.map(PathBuf::from);
    if asr_engine == AsrEngine::Whisper {
        let model_path = model_path
            .as_ref()
            .context("Missing --model <path> for Whisper engine")?;
        ensure_file(model_path, "ASR model")?;
    } else if let Some(model_path) = model_path.as_ref() {
        ensure_file(model_path, "fallback ASR model")?;
    }

    Ok(RuntimeTranscribeConfig {
        input_path,
        model_path,
        whisper_cli: whisper_engine::resolve_whisper_cli(whisper_cli.map(PathBuf::from)),
        language,
        file_hash,
        asr_engine,
        extreme_accuracy,
        audio_mode,
        vocal_separation,
    })
}

fn take_arg_value(args: &[String], index: &mut usize, flag: &str) -> anyhow::Result<String> {
    *index += 1;
    let value = args
        .get(*index)
        .with_context(|| format!("Missing value for {flag}"))?;
    if value.starts_with('-') {
        anyhow::bail!("Missing value for {flag}");
    }
    Ok(value.clone())
}

fn parse_audio_mode(value: &str) -> anyhow::Result<AudioMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "speech" | "default" => Ok(AudioMode::Speech),
        "music" | "lyrics" | "lyric" => Ok(AudioMode::Music),
        other => anyhow::bail!("Unsupported audio mode: {other}"),
    }
}

fn parse_asr_engine(value: &str) -> anyhow::Result<AsrEngine> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "sensevoice" | "sense_voice" => Ok(AsrEngine::SenseVoice),
        "whisper" | "whispercpp" | "whisper.cpp" => Ok(AsrEngine::Whisper),
        "funasr" | "fun-asr" | "fun_asr" | "funasr-nano" | "fun-asr-nano" => Ok(AsrEngine::FunAsr),
        other => anyhow::bail!("Unsupported ASR engine: {other}"),
    }
}

fn parse_vocal_separation_mode(value: &str) -> anyhow::Result<VocalSeparationMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "off" | "none" | "false" | "disabled" => Ok(VocalSeparationMode::Off),
        "demucs" | "vocal" | "vocals" | "on" | "true" => Ok(VocalSeparationMode::Demucs),
        other => anyhow::bail!("Unsupported vocal separation mode: {other}"),
    }
}

fn sensevoice_chunking_plan(
    audio_mode: AudioMode,
    extreme_accuracy: bool,
    vocals_isolated: bool,
) -> SenseVoiceChunkingPlan {
    let max_chunk_ms = if extreme_accuracy { 20_000 } else { 30_000 };
    match audio_mode {
        AudioMode::Music => SenseVoiceChunkingPlan {
            max_chunk_ms,
            overlap_ms: 2_000,
            internal_vad: !vocals_isolated,
        },
        AudioMode::Speech => SenseVoiceChunkingPlan {
            max_chunk_ms,
            overlap_ms: 0,
            internal_vad: true,
        },
    }
}

fn music_chunking_plan(_extreme_accuracy: bool) -> (i64, i64) {
    (90_000, 0)
}

fn ensure_file(path: &Path, label: &str) -> anyhow::Result<()> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("{label} file not found: {}", path.display()))?;
    if !metadata.is_file() {
        anyhow::bail!("{label} path is not a file: {}", path.display());
    }
    if metadata.len() == 0 {
        anyhow::bail!("{label} file is empty: {}", path.display());
    }
    Ok(())
}

#[cfg(target_os = "windows")]
async fn run_runtime_ipc_loop() -> anyhow::Result<()> {
    use audraflow_ipc::{DiagnosticsRequest, IpcEnvelope, IpcMessage};
    use tokio::io::AsyncWriteExt;
    use tokio::net::windows::named_pipe;

    let pipe_name = r"\\.\pipe\audraflow-asr";

    loop {
        let mut server = named_pipe::ServerOptions::new()
            .first_pipe_instance(false)
            .create(pipe_name)?;

        server.connect().await?;
        log::debug!("Runtime IPC client connected");

        tokio::spawn(async move {
            let mut buf = vec![0u8; 65536];
            loop {
                if let Err(e) = server.readable().await {
                    log::debug!("Runtime IPC client disconnected: {}", e);
                    break;
                }

                match server.try_read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let reply = match serde_json::from_slice::<IpcEnvelope>(&buf[..n]) {
                            Ok(envelope) => match envelope.payload {
                                IpcMessage::DiagnosticsRequest(req) => IpcEnvelope::new(
                                    IpcMessage::DiagnosticsRequest(DiagnosticsRequest {
                                        include_logs: req.include_logs,
                                        include_config: req.include_config,
                                        include_device_info: true,
                                    }),
                                ),
                                other => IpcEnvelope::new(other),
                            },
                            Err(e) => {
                                log::warn!("Invalid runtime IPC message: {}", e);
                                continue;
                            }
                        };

                        match serde_json::to_vec(&reply) {
                            Ok(bytes) => {
                                if let Err(e) = server.write_all(&bytes).await {
                                    log::debug!("Runtime IPC write failed: {}", e);
                                    break;
                                }
                            }
                            Err(e) => log::warn!("Runtime IPC serialization failed: {}", e),
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                    Err(e) => {
                        log::debug!("Runtime IPC read failed: {}", e);
                        break;
                    }
                }
            }
        });
    }
}

#[cfg(not(target_os = "windows"))]
async fn run_runtime_ipc_loop() -> anyhow::Result<()> {
    std::future::pending::<()>().await;
    Ok(())
}

/// Full end-to-end transcription pipeline:
///   audio file → decode → VAD → chunk → transcribe → merge → segments
#[allow(dead_code)]
pub async fn transcribe_file_pipeline(
    engine: &whisper_engine::WhisperEngine,
    pipeline: &audio_pipeline::AudioPipeline,
    file_path: &Path,
    file_hash: &str,
    extreme_accuracy: bool,
) -> anyhow::Result<TranscriptionResult> {
    transcribe_file_pipeline_sync(
        engine,
        pipeline,
        file_path,
        file_hash,
        extreme_accuracy,
        AudioMode::Speech,
        VocalSeparationMode::Off,
    )
}

/// Synchronous implementation used by benchmark and the async IPC wrapper.
#[allow(dead_code)]
pub fn transcribe_file_pipeline_sync(
    engine: &whisper_engine::WhisperEngine,
    pipeline: &audio_pipeline::AudioPipeline,
    file_path: &Path,
    file_hash: &str,
    extreme_accuracy: bool,
    audio_mode: AudioMode,
    vocal_separation: VocalSeparationMode,
) -> anyhow::Result<TranscriptionResult> {
    let start = Instant::now();
    let ttfv_start = Instant::now();
    let mut preprocess_messages = Vec::new();

    if audio_mode == AudioMode::Music && extreme_accuracy {
        preprocess_messages
            .push("Lyrics high-accuracy mode: original-audio Whisper candidate".into());
        let original_pass = run_whisper_pipeline_pass(
            engine,
            pipeline,
            file_path,
            file_hash,
            extreme_accuracy,
            audio_mode,
            "original audio",
        )?;
        let mut audio_info = original_pass.audio_info;
        let mut segments = original_pass.segments;
        let mut chunk_count = original_pass.chunk_count;

        match run_demucs_vocal_separation(file_path, pipeline.temp_dir()) {
            Ok(vocals_path) => {
                log::info!(
                    "Lyrics high-accuracy Demucs candidate ready: {}",
                    vocals_path.display()
                );
                preprocess_messages
                    .push("Lyrics high-accuracy mode: Demucs vocals candidate".into());
                match run_whisper_pipeline_pass(
                    engine,
                    pipeline,
                    &vocals_path,
                    file_hash,
                    extreme_accuracy,
                    audio_mode,
                    "Demucs vocals",
                ) {
                    Ok(vocals_pass) => {
                        let original_count = segments.len();
                        let vocals_count = vocals_pass.segments.len();
                        chunk_count += vocals_pass.chunk_count;
                        segments = merge_music_candidate_segments(segments, vocals_pass.segments);
                        preprocess_messages.push(format!(
                            "Lyrics candidates merged: original {original_count} segment(s), vocals {vocals_count} segment(s), final {} segment(s)",
                            segments.len()
                        ));
                    }
                    Err(error) => {
                        log::warn!(
                            "Demucs vocals candidate failed; using original-audio candidate: {error}"
                        );
                        preprocess_messages.push(format!(
                            "Demucs vocals candidate failed; used original audio: {}",
                            truncate_message(&error.to_string(), 220)
                        ));
                    }
                }
            }
            Err(error) => {
                log::warn!(
                    "Demucs vocal separation unavailable; using original-audio lyrics candidate: {error}"
                );
                preprocess_messages.push(format!(
                    "Demucs unavailable; used original-audio lyrics candidate: {}",
                    truncate_message(&error.to_string(), 220)
                ));
            }
        }

        audio_info.estimated_speakers = Some(1);
        return finish_transcription_result(
            start,
            ttfv_start,
            pipeline,
            audio_info,
            segments,
            chunk_count,
            preprocess_messages,
        );
    }

    let mut active_file_path = file_path.to_path_buf();
    if audio_mode == AudioMode::Music && vocal_separation == VocalSeparationMode::Demucs {
        match run_demucs_vocal_separation(file_path, pipeline.temp_dir()) {
            Ok(vocals_path) => {
                log::info!(
                    "Demucs vocal separation complete: {}",
                    vocals_path.display()
                );
                preprocess_messages.push("Demucs vocal separation complete".into());
                active_file_path = vocals_path;
            }
            Err(error) => {
                log::warn!(
                    "Demucs vocal separation unavailable; falling back to original audio: {error}"
                );
                preprocess_messages.push(format!(
                    "Demucs unavailable; used original audio: {}",
                    truncate_message(&error.to_string(), 220)
                ));
            }
        }
    }

    let pass = run_whisper_pipeline_pass(
        engine,
        pipeline,
        &active_file_path,
        file_hash,
        extreme_accuracy,
        audio_mode,
        "main",
    )?;

    finish_transcription_result(
        start,
        ttfv_start,
        pipeline,
        pass.audio_info,
        pass.segments,
        pass.chunk_count,
        preprocess_messages,
    )
}

#[derive(Debug, Clone)]
struct WhisperPipelinePass {
    segments: Vec<Segment>,
    audio_info: audio_pipeline::AudioInfo,
    chunk_count: u32,
}

fn run_whisper_pipeline_pass(
    engine: &whisper_engine::WhisperEngine,
    pipeline: &audio_pipeline::AudioPipeline,
    file_path: &Path,
    file_hash: &str,
    extreme_accuracy: bool,
    audio_mode: AudioMode,
    label: &str,
) -> anyhow::Result<WhisperPipelinePass> {
    // ── Step 1: Analyze audio metadata ────────────────────────────────────
    log::info!("[1/4] Analyzing {label}: {}", file_path.display());
    let mut info = pipeline.analyze(file_path, file_hash)?;

    // ── Step 2: Decode to 16kHz mono WAV ──────────────────────────────────
    log::info!("[2/4] Decoding {label}...");
    let wav_path = pipeline.decode_to_wav(file_path)?;

    // ── Step 3: VAD + chunking ────────────────────────────────────────────
    log::info!("[3/4] Chunking {label}...");
    let chunks = if audio_mode == AudioMode::Music {
        let (max_chunk, overlap) = music_chunking_plan(extreme_accuracy);
        pipeline.chunk_full_audio(&wav_path, max_chunk, overlap)?
    } else {
        let min_chunk = if extreme_accuracy { 15_000 } else { 30_000 }; // ms
        let max_chunk = if extreme_accuracy { 30_000 } else { 60_000 };
        pipeline.vad_and_chunk(&wav_path, min_chunk, max_chunk)?
    };
    log::info!("  Produced {} chunks", chunks.len());

    let diarization = run_diarization(pipeline, &wav_path, &chunks, audio_mode);
    info.estimated_speakers = Some(diarization.speaker_count_estimate);

    // ── Step 4: Transcribe each chunk ─────────────────────────────────────
    log::info!("[4/4] Transcribing {label}: {} chunks...", chunks.len());
    let mut all_segments: Vec<Segment> = Vec::new();

    for chunk in &chunks {
        let chunk_segments = engine.transcribe(&chunk.wav_path)?;
        append_chunk_segments(&mut all_segments, chunk, chunk_segments);
    }

    // Sort by start time
    all_segments.sort_by_key(|s| s.start_ms);
    if audio_mode == AudioMode::Music {
        all_segments = clean_music_segments(all_segments);
    }
    apply_diarization_to_segments(&mut all_segments, &diarization);

    Ok(WhisperPipelinePass {
        segments: all_segments,
        audio_info: info,
        chunk_count: chunks.len() as u32,
    })
}

fn finish_transcription_result(
    start: Instant,
    ttfv_start: Instant,
    pipeline: &audio_pipeline::AudioPipeline,
    audio_info: audio_pipeline::AudioInfo,
    segments: Vec<Segment>,
    chunk_count: u32,
    preprocess_messages: Vec<String>,
) -> anyhow::Result<TranscriptionResult> {
    let elapsed = start.elapsed();
    let ttfv = ttfv_start.elapsed();
    let rtf = elapsed.as_secs_f64() / audio_info.duration_seconds;
    let ttfv_s = ttfv.as_secs_f64();

    // ── Cleanup temp files ────────────────────────────────────────────────
    let _ = pipeline.cleanup();

    log::info!(
        "Transcription pipeline complete: {} segments in {:.1}s (RTF={:.3}, TTFV={:.1}s)",
        segments.len(),
        elapsed.as_secs_f64(),
        rtf,
        ttfv_s,
    );

    Ok(TranscriptionResult {
        segments,
        audio_info,
        rtf,
        ttfv_s,
        chunk_count,
        preprocess_messages,
    })
}

fn transcribe_file_with_funasr_sync(
    pipeline: &audio_pipeline::AudioPipeline,
    file_path: &Path,
    file_hash: &str,
    language: &str,
    _extreme_accuracy: bool,
    audio_mode: AudioMode,
    vocal_separation: VocalSeparationMode,
) -> anyhow::Result<TranscriptionResult> {
    let start = Instant::now();
    let ttfv_start = Instant::now();
    let mut active_file_path = file_path.to_path_buf();
    let mut preprocess_messages = vec![
        "Fun-ASR-Nano experimental engine".to_string(),
        "Fun-ASR timestamps are chunk-level timestamps".to_string(),
    ];

    if audio_mode == AudioMode::Music {
        preprocess_messages.push("Fun-ASR music mode: fixed 15-second chunks".into());
    }

    if audio_mode == AudioMode::Music && vocal_separation == VocalSeparationMode::Demucs {
        match run_demucs_vocal_separation(file_path, pipeline.temp_dir()) {
            Ok(vocals_path) => {
                log::info!(
                    "Demucs vocal separation complete for Fun-ASR: {}",
                    vocals_path.display()
                );
                preprocess_messages.push("Demucs vocal separation complete".into());
                active_file_path = vocals_path;
            }
            Err(error) => {
                log::warn!(
                    "Demucs vocal separation unavailable; falling back to original audio: {error}"
                );
                preprocess_messages.push(format!(
                    "Demucs unavailable; used original audio: {}",
                    truncate_message(&error.to_string(), 220)
                ));
            }
        }
    }

    log::info!(
        "[1/3] Analyzing for Fun-ASR: {}",
        active_file_path.display()
    );
    let mut info = pipeline.analyze(&active_file_path, file_hash)?;

    log::info!("[2/3] Decoding for Fun-ASR...");
    let wav_path = pipeline.decode_to_wav(&active_file_path)?;

    log::info!("[3/3] Running Fun-ASR-Nano...");
    let funasr = funasr_engine::FunAsrEngine::new(language.to_string())?;
    let mut segments = funasr.transcribe_wav(&wav_path, audio_mode == AudioMode::Music)?;
    segments.sort_by_key(|segment| segment.start_ms);
    if audio_mode == AudioMode::Music {
        segments = clean_music_segments(segments);
    }
    let chunk_count = segments.len() as u32;
    info.estimated_speakers = Some(1);

    finish_transcription_result(
        start,
        ttfv_start,
        pipeline,
        info,
        segments,
        chunk_count,
        preprocess_messages,
    )
}

fn transcribe_file_with_sensevoice_sync(
    pipeline: &audio_pipeline::AudioPipeline,
    file_path: &Path,
    file_hash: &str,
    language: &str,
    extreme_accuracy: bool,
    audio_mode: AudioMode,
    vocal_separation: VocalSeparationMode,
) -> anyhow::Result<TranscriptionResult> {
    let start = Instant::now();
    let ttfv_start = Instant::now();
    let mut active_file_path = file_path.to_path_buf();
    let mut preprocess_messages = vec!["SenseVoice engine".to_string()];
    let mut vocals_isolated = false;

    if audio_mode == AudioMode::Music && vocal_separation == VocalSeparationMode::Demucs {
        match run_demucs_vocal_separation(file_path, pipeline.temp_dir()) {
            Ok(vocals_path) => {
                log::info!(
                    "Demucs vocal separation complete: {}",
                    vocals_path.display()
                );
                preprocess_messages.push("Demucs vocal separation complete".into());
                active_file_path = vocals_path;
                vocals_isolated = true;
            }
            Err(error) => {
                log::warn!(
                    "Demucs vocal separation unavailable; falling back to original audio: {error}"
                );
                preprocess_messages.push(format!(
                    "Demucs unavailable; used original audio: {}",
                    truncate_message(&error.to_string(), 220)
                ));
            }
        }
    }

    log::info!(
        "[1/4] Analyzing for SenseVoice: {}",
        active_file_path.display()
    );
    let mut info = pipeline.analyze(&active_file_path, file_hash)?;

    log::info!("[2/4] Decoding for SenseVoice...");
    let wav_path = pipeline.decode_to_wav(&active_file_path)?;

    log::info!("[3/4] Chunking for SenseVoice...");
    let chunking_plan = sensevoice_chunking_plan(audio_mode, extreme_accuracy, vocals_isolated);
    if !chunking_plan.internal_vad {
        preprocess_messages.push("Preserve-content mode: SenseVoice internal VAD disabled".into());
    }
    let chunks = pipeline.chunk_full_audio(
        &wav_path,
        chunking_plan.max_chunk_ms,
        chunking_plan.overlap_ms,
    )?;
    log::info!("  Produced {} SenseVoice chunks", chunks.len());

    let diarization = run_diarization(pipeline, &wav_path, &chunks, audio_mode);
    info.estimated_speakers = Some(diarization.speaker_count_estimate);

    log::info!("[4/4] Running SenseVoice on {} chunks...", chunks.len());
    let sensevoice = sensevoice_engine::SenseVoiceEngine::new(language.to_string())?
        .with_internal_vad(chunking_plan.internal_vad);
    let mut all_segments = sensevoice.transcribe_chunks(&chunks)?;
    all_segments.sort_by_key(|segment| segment.start_ms);
    if audio_mode == AudioMode::Music {
        all_segments = clean_music_segments(all_segments);
    }
    apply_diarization_to_segments(&mut all_segments, &diarization);

    let elapsed = start.elapsed();
    let ttfv = ttfv_start.elapsed();
    let rtf = elapsed.as_secs_f64() / info.duration_seconds;
    let ttfv_s = ttfv.as_secs_f64();

    let _ = pipeline.cleanup();

    log::info!(
        "SenseVoice pipeline complete: {} segments in {:.1}s (RTF={:.3}, TTFV={:.1}s)",
        all_segments.len(),
        elapsed.as_secs_f64(),
        rtf,
        ttfv_s,
    );

    Ok(TranscriptionResult {
        segments: all_segments,
        audio_info: info,
        rtf,
        ttfv_s,
        chunk_count: chunks.len() as u32,
        preprocess_messages,
    })
}

#[derive(Debug, Clone)]
struct DemucsInvocation {
    program: OsString,
    base_args: Vec<OsString>,
    label: String,
}

fn run_demucs_vocal_separation(input_path: &Path, temp_dir: &Path) -> anyhow::Result<PathBuf> {
    let demucs = resolve_demucs_invocation()
        .context("demucs was not found in PATH; install demucs or set AUDRAFLOW_DEMUCS_BIN")?;
    let output_dir = temp_dir.join("demucs");
    std::fs::create_dir_all(&output_dir)?;

    log::info!(
        "Running Demucs vocal separation with {}: {}",
        demucs.label,
        input_path.display()
    );

    let output = Command::new(&demucs.program)
        .args(&demucs.base_args)
        .arg("--two-stems")
        .arg("vocals")
        .arg("--out")
        .arg(&output_dir)
        .arg(input_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("failed to start {}", demucs.label))?;

    if !output.status.success() {
        anyhow::bail!(
            "{} exited with {}. stderr: {} stdout: {}",
            demucs.label,
            output.status,
            preview_text(&output.stderr),
            preview_text(&output.stdout)
        );
    }

    find_vocals_output(&output_dir).with_context(|| {
        format!(
            "demucs did not produce vocals.wav under {}",
            output_dir.display()
        )
    })
}

fn resolve_demucs_invocation() -> Option<DemucsInvocation> {
    if let Some(program) = demucs_env_override() {
        let invocation = DemucsInvocation {
            label: program.to_string_lossy().into_owned(),
            program,
            base_args: Vec::new(),
        };
        if probe_demucs(&invocation) {
            return Some(invocation);
        }
        log::warn!("AUDRAFLOW_DEMUCS_BIN is set but is not runnable as demucs");
    }

    let candidates = [
        DemucsInvocation {
            program: OsString::from("demucs"),
            base_args: Vec::new(),
            label: "demucs".into(),
        },
        DemucsInvocation {
            program: OsString::from("python3"),
            base_args: vec![OsString::from("-m"), OsString::from("demucs")],
            label: "python3 -m demucs".into(),
        },
        DemucsInvocation {
            program: OsString::from("python"),
            base_args: vec![OsString::from("-m"), OsString::from("demucs")],
            label: "python -m demucs".into(),
        },
        DemucsInvocation {
            program: OsString::from("py"),
            base_args: vec![
                OsString::from("-3"),
                OsString::from("-m"),
                OsString::from("demucs"),
            ],
            label: "py -3 -m demucs".into(),
        },
    ];

    candidates.into_iter().find(probe_demucs)
}

fn demucs_env_override() -> Option<OsString> {
    std::env::var_os("AUDRAFLOW_DEMUCS_BIN")
        .or_else(|| std::env::var_os("DEMUCS_BIN"))
        .filter(|value| !value.is_empty())
}

fn probe_demucs(invocation: &DemucsInvocation) -> bool {
    Command::new(&invocation.program)
        .args(&invocation.base_args)
        .arg("--help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn find_vocals_output(output_dir: &Path) -> anyhow::Result<PathBuf> {
    let mut candidates = Vec::new();
    collect_vocals_outputs(output_dir, &mut candidates)?;
    candidates.sort_by_key(|path| {
        std::fs::metadata(path)
            .map(|metadata| std::cmp::Reverse(metadata.len()))
            .unwrap_or(std::cmp::Reverse(0))
    });
    candidates
        .into_iter()
        .next()
        .context("vocals.wav was not found")
}

fn collect_vocals_outputs(dir: &Path, candidates: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    if !dir.exists() {
        return Ok(());
    }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_vocals_outputs(&path, candidates)?;
            continue;
        }

        let file_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        if file_name == "vocals.wav" || (stem == "vocals" && extension == "wav") {
            candidates.push(path);
        }
    }

    Ok(())
}

fn preview_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    if trimmed.chars().count() <= 500 {
        trimmed.to_string()
    } else {
        format!("{}...", trimmed.chars().take(500).collect::<String>())
    }
}

fn truncate_message(message: &str, max_chars: usize) -> String {
    if message.chars().count() <= max_chars {
        message.to_string()
    } else {
        format!("{}...", message.chars().take(max_chars).collect::<String>())
    }
}

fn run_diarization(
    pipeline: &audio_pipeline::AudioPipeline,
    wav_path: &Path,
    chunks: &[audio_pipeline::AudioChunk],
    audio_mode: AudioMode,
) -> DiarizationOutput {
    let vad_segments = if audio_mode == AudioMode::Speech {
        match pipeline.detect_speech_for_diarization(wav_path) {
            Ok(segments) => segments,
            Err(error) => {
                log::warn!(
                    "Diarization VAD failed for {}; falling back to ASR chunks: {error}",
                    wav_path.display()
                );
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    let speech_segments = prepare_diarization_segments(&vad_segments, chunks);
    let input = DiarizationInput {
        speech_segments,
        wav_path: wav_path.to_path_buf(),
        max_speakers: 4,
    };

    DiarizationWorker::new().run(&input)
}

fn prepare_diarization_segments(
    vad_segments: &[VadSegment],
    chunks: &[audio_pipeline::AudioChunk],
) -> Vec<VadSegment> {
    const MAX_DIARIZATION_SEGMENT_MS: i64 = 3_000;
    const MIN_DIARIZATION_TAIL_MS: i64 = 800;

    let mut source = vad_segments
        .iter()
        .filter(|segment| segment.has_speech && segment.end_ms > segment.start_ms)
        .cloned()
        .collect::<Vec<_>>();

    if source.is_empty() {
        source = chunks_to_vad_segments(chunks);
    }

    let mut output = Vec::new();
    for segment in source {
        let duration_ms = segment.end_ms - segment.start_ms;
        if duration_ms <= MAX_DIARIZATION_SEGMENT_MS {
            output.push(segment);
            continue;
        }

        let mut start_ms = segment.start_ms;
        while start_ms < segment.end_ms {
            let end_ms = (start_ms + MAX_DIARIZATION_SEGMENT_MS).min(segment.end_ms);
            if end_ms - start_ms < MIN_DIARIZATION_TAIL_MS {
                if let Some(previous) = output.last_mut() {
                    previous.end_ms = segment.end_ms;
                } else {
                    output.push(VadSegment {
                        start_ms,
                        end_ms: segment.end_ms,
                        has_speech: true,
                        snr_db: segment.snr_db,
                    });
                }
                break;
            }

            output.push(VadSegment {
                start_ms,
                end_ms,
                has_speech: true,
                snr_db: segment.snr_db,
            });
            start_ms = end_ms;
        }
    }

    output
}

fn chunks_to_vad_segments(chunks: &[audio_pipeline::AudioChunk]) -> Vec<VadSegment> {
    chunks
        .iter()
        .filter(|chunk| chunk.end_ms > chunk.start_ms)
        .map(|chunk| VadSegment {
            start_ms: chunk.start_ms,
            end_ms: chunk.end_ms,
            has_speech: true,
            snr_db: chunk.snr_db,
        })
        .collect()
}

fn append_chunk_segments(
    all_segments: &mut Vec<Segment>,
    chunk: &audio_pipeline::AudioChunk,
    chunk_segments: Vec<Segment>,
) {
    let chunk_duration_ms = (chunk.end_ms - chunk.start_ms).max(0);
    for mut segment in chunk_segments {
        let relative_start_ms = segment.start_ms.clamp(0, chunk_duration_ms);
        let relative_end_ms = segment.end_ms.clamp(relative_start_ms, chunk_duration_ms);
        if relative_end_ms <= relative_start_ms {
            continue;
        }

        segment.start_ms = chunk.start_ms + relative_start_ms;
        segment.end_ms = chunk.start_ms + relative_end_ms;
        segment.segment_id = format!("chunk{:04}-{}", chunk.index, segment.segment_id);
        all_segments.push(segment);
    }
}

fn merge_music_candidate_segments(original: Vec<Segment>, vocals: Vec<Segment>) -> Vec<Segment> {
    if original.is_empty() {
        return clean_music_segments(vocals);
    }
    if vocals.is_empty() {
        return clean_music_segments(original);
    }

    let original_score = music_candidate_score(&original);
    let vocals_score = music_candidate_score(&vocals);
    let prefer_vocals = vocals_score >= original_score * 0.90;
    let allow_vocal_replacement = vocals_score >= original_score * 0.50;
    let (mut merged, candidates, allow_not_shorter_replacement) = if prefer_vocals {
        (vocals, original, false)
    } else {
        (original, vocals, allow_vocal_replacement)
    };

    for candidate in candidates {
        merge_music_candidate_segment(&mut merged, candidate, allow_not_shorter_replacement);
    }

    merged.sort_by_key(|segment| segment.start_ms);
    clean_music_segments(merged)
}

fn music_candidate_score(segments: &[Segment]) -> f64 {
    let text_chars = segments
        .iter()
        .map(|segment| normalize_transcript_text(&segment.text).chars().count())
        .sum::<usize>() as f64;
    let coverage_s = segments
        .iter()
        .map(|segment| (segment.end_ms - segment.start_ms).max(0) as f64 / 1000.0)
        .sum::<f64>();
    text_chars + coverage_s * 0.5 + segments.len() as f64
}

fn merge_music_candidate_segment(
    merged: &mut Vec<Segment>,
    candidate: Segment,
    allow_not_shorter_replacement: bool,
) {
    let candidate_text_len = normalized_segment_text_len(&candidate);
    if candidate_text_len == 0 || is_music_metadata_hallucination(&candidate.text) {
        return;
    }

    let overlapping_indices = merged
        .iter()
        .enumerate()
        .filter_map(|(index, existing)| {
            let overlap_ms = overlap_duration_ms(
                candidate.start_ms,
                candidate.end_ms,
                existing.start_ms,
                existing.end_ms,
            );
            (overlap_ms >= 1_000).then_some(index)
        })
        .collect::<Vec<_>>();

    if overlapping_indices.is_empty() {
        merged.push(candidate);
        return;
    }

    let existing_text_len = overlapping_indices
        .iter()
        .map(|index| normalized_segment_text_len(&merged[*index]))
        .sum::<usize>();
    let total_overlap_ms = overlapping_indices
        .iter()
        .map(|index| {
            let existing = &merged[*index];
            overlap_duration_ms(
                candidate.start_ms,
                candidate.end_ms,
                existing.start_ms,
                existing.end_ms,
            )
        })
        .sum::<i64>();
    let candidate_duration_ms = segment_duration_ms(&candidate);
    let candidate_is_same_time_region =
        candidate_duration_ms == 0 || total_overlap_ms * 2 >= candidate_duration_ms;

    if candidate_text_len >= existing_text_len.saturating_add(12)
        || (candidate_text_len as f64) > (existing_text_len as f64 * 1.35)
        || (allow_not_shorter_replacement
            && candidate_is_same_time_region
            && candidate_text_len >= existing_text_len)
    {
        for index in overlapping_indices.into_iter().rev() {
            merged.remove(index);
        }
        merged.push(candidate);
    }
}

fn normalized_segment_text_len(segment: &Segment) -> usize {
    normalize_transcript_text(&segment.text).chars().count()
}

fn segment_duration_ms(segment: &Segment) -> i64 {
    (segment.end_ms - segment.start_ms).max(0)
}

fn overlap_duration_ms(a_start_ms: i64, a_end_ms: i64, b_start_ms: i64, b_end_ms: i64) -> i64 {
    (a_end_ms.min(b_end_ms) - a_start_ms.max(b_start_ms)).max(0)
}

fn clean_music_segments(segments: Vec<Segment>) -> Vec<Segment> {
    let mut kept = Vec::new();
    let mut recent: VecDeque<(i64, i64, String)> = VecDeque::new();
    let mut last_seen_text: Option<(i64, String)> = None;

    for mut segment in segments {
        segment.text = sanitize_music_segment_text(&segment.text);
        segment.raw_text = sanitize_music_segment_text(&segment.raw_text);
        let normalized = normalize_transcript_text(&segment.text);
        if normalized.is_empty()
            || is_non_lyric_music_annotation(&normalized)
            || is_music_metadata_hallucination(&segment.text)
        {
            continue;
        }

        let is_adjacent_runaway_repeat = last_seen_text.as_ref().is_some_and(|(end_ms, text)| {
            text == &normalized && segment.start_ms - *end_ms < 1_000
        });
        last_seen_text = Some((segment.end_ms, normalized.clone()));
        if is_adjacent_runaway_repeat {
            continue;
        }

        while recent
            .front()
            .is_some_and(|(_, end_ms, _)| segment.start_ms - *end_ms > 10_000)
        {
            recent.pop_front();
        }

        if recent.iter().any(|(start_ms, end_ms, text)| {
            text == &normalized
                && ranges_overlap(segment.start_ms, segment.end_ms, *start_ms, *end_ms)
        }) {
            continue;
        }

        recent.push_back((segment.start_ms, segment.end_ms, normalized));
        kept.push(segment);
    }

    kept
}

fn sanitize_music_segment_text(text: &str) -> String {
    text.trim()
        .trim_matches(|ch| matches!(ch, '♪' | '♫' | '♬' | '♩'))
        .trim()
        .to_string()
}

fn is_non_lyric_music_annotation(normalized: &str) -> bool {
    matches!(
        normalized,
        "music" | "instrumental" | "silence" | "noise" | "applause" | "纯音乐" | "音樂" | "音乐"
    )
}

fn ranges_overlap(a_start_ms: i64, a_end_ms: i64, b_start_ms: i64, b_end_ms: i64) -> bool {
    let overlap_ms = a_end_ms.min(b_end_ms) - a_start_ms.max(b_start_ms);
    overlap_ms > 250
}

fn normalize_transcript_text(text: &str) -> String {
    text.chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || is_cjk(*ch))
        .flat_map(char::to_lowercase)
        .collect()
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF
    )
}

fn is_music_metadata_hallucination(text: &str) -> bool {
    let compact = normalize_transcript_text(text);
    if compact.is_empty() {
        return true;
    }

    if is_music_watermark_hallucination(&compact) {
        return true;
    }

    let metadata_terms = [
        "cc字幕",
        "字幕制作",
        "字幕製作",
        "字幕",
        "作词",
        "作詞",
        "作曲",
        "编曲",
        "編曲",
        "混音",
        "母带",
        "母帶",
        "录音",
        "錄音",
        "翻译",
        "翻譯",
        "制作人",
        "製作人",
        "监制",
        "監製",
        "出品",
        "发行",
        "發行",
        "录制",
        "錄製",
        "后期",
        "後期",
        "剪辑",
        "剪輯",
        "封面",
        "特别鸣谢",
        "特別鳴謝",
    ];

    if compact.chars().count() <= 42 && metadata_terms.iter().any(|term| compact.contains(term)) {
        return true;
    }

    let copyright_terms = [
        "版权所有",
        "版權所有",
        "未经许可",
        "未經許可",
        "不得翻唱",
        "不得翻录",
        "不得翻錄",
        "翻录必究",
        "翻錄必究",
    ];
    compact.chars().count() <= 64 && copyright_terms.iter().any(|term| compact.contains(term))
}

fn is_music_watermark_hallucination(compact: &str) -> bool {
    let watermark_terms = [
        "优优独播",
        "優優獨播",
        "独播剧场",
        "獨播劇場",
        "优酷独播",
        "優酷獨播",
        "腾讯视频",
        "騰訊視頻",
        "爱奇艺",
        "愛奇藝",
        "芒果tv",
        "yoyotelevisionseriesexclusive",
        "yoyotelevision",
        "televisionseriesexclusive",
        "seriesexclusive",
    ];

    watermark_terms.iter().any(|term| compact.contains(term))
}

fn apply_diarization_to_segments(segments: &mut [Segment], diarization: &DiarizationOutput) {
    if diarization.speaker_segments.is_empty() {
        return;
    }

    for segment in segments {
        let midpoint = segment.start_ms + ((segment.end_ms - segment.start_ms).max(0) / 2);
        let speaker = diarization
            .speaker_segments
            .iter()
            .find(|speaker_segment| {
                midpoint >= speaker_segment.start_ms && midpoint <= speaker_segment.end_ms
            })
            .or_else(|| {
                diarization
                    .speaker_segments
                    .iter()
                    .min_by_key(|speaker_segment| {
                        if midpoint < speaker_segment.start_ms {
                            speaker_segment.start_ms - midpoint
                        } else if midpoint > speaker_segment.end_ms {
                            midpoint - speaker_segment.end_ms
                        } else {
                            0
                        }
                    })
            });

        if let Some(speaker) = speaker {
            segment.speaker_id = Some(speaker.speaker_id.clone());
            if speaker.is_overlap
                && !segment
                    .low_confidence_reasons
                    .iter()
                    .any(|reason| reason == "overlapping_speech")
            {
                segment
                    .low_confidence_reasons
                    .push("overlapping_speech".into());
            }
            if speaker.confidence < 0.65
                && !segment
                    .low_confidence_reasons
                    .iter()
                    .any(|reason| reason == "speaker_uncertain")
            {
                segment
                    .low_confidence_reasons
                    .push("speaker_uncertain".into());
            }
        }
    }
}

/// Result of a full transcription pipeline run.
#[derive(Debug, Clone)]
pub struct TranscriptionResult {
    pub segments: Vec<Segment>,
    pub audio_info: audio_pipeline::AudioInfo,
    pub rtf: f64,
    pub ttfv_s: f64,
    pub chunk_count: u32,
    pub preprocess_messages: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diarization::SpeakerSegment;

    fn segment(segment_id: &str, start_ms: i64, end_ms: i64) -> Segment {
        Segment {
            segment_id: segment_id.into(),
            start_ms,
            end_ms,
            speaker_id: None,
            text: "text".into(),
            raw_text: "text".into(),
            confidence: 0.9,
            low_confidence_reasons: Vec::new(),
            corrections: Vec::new(),
            marks: Vec::new(),
        }
    }

    fn segment_with_text(segment_id: &str, start_ms: i64, end_ms: i64, text: &str) -> Segment {
        Segment {
            text: text.into(),
            raw_text: text.into(),
            ..segment(segment_id, start_ms, end_ms)
        }
    }

    fn audio_chunk(index: usize, start_ms: i64, end_ms: i64) -> audio_pipeline::AudioChunk {
        audio_pipeline::AudioChunk {
            index,
            start_ms,
            end_ms,
            wav_path: PathBuf::from(format!("chunk-{index}.wav")),
            snr_db: 20.0,
        }
    }

    fn vad_segment(start_ms: i64, end_ms: i64) -> VadSegment {
        VadSegment {
            start_ms,
            end_ms,
            has_speech: true,
            snr_db: 20.0,
        }
    }

    #[test]
    fn parse_audio_mode_accepts_music_aliases() {
        assert_eq!(parse_audio_mode("music").unwrap(), AudioMode::Music);
        assert_eq!(parse_audio_mode("lyrics").unwrap(), AudioMode::Music);
        assert_eq!(parse_audio_mode("speech").unwrap(), AudioMode::Speech);
        assert!(parse_audio_mode("podcast").is_err());
    }

    #[test]
    fn parse_asr_engine_accepts_supported_engines() {
        assert_eq!(
            parse_asr_engine("sensevoice").unwrap(),
            AsrEngine::SenseVoice
        );
        assert_eq!(parse_asr_engine("funasr").unwrap(), AsrEngine::FunAsr);
        assert_eq!(parse_asr_engine("fun-asr-nano").unwrap(), AsrEngine::FunAsr);
        assert_eq!(parse_asr_engine("whisper.cpp").unwrap(), AsrEngine::Whisper);
        assert!(parse_asr_engine("unknown").is_err());
    }

    #[test]
    fn parse_runtime_transcribe_args_defaults_language_to_auto() {
        let input_path =
            std::env::temp_dir().join(format!("audraflow-runtime-args-{}.wav", std::process::id()));
        std::fs::write(&input_path, b"not real audio").unwrap();

        let args = vec![input_path.to_string_lossy().into_owned()];
        let config = parse_runtime_transcribe_args(&args).unwrap();

        assert_eq!(config.language, "auto");
        let _ = std::fs::remove_file(input_path);
    }

    #[test]
    fn sensevoice_music_chunking_disables_internal_vad_only_for_isolated_vocals() {
        let music_without_vocals = sensevoice_chunking_plan(AudioMode::Music, false, false);
        assert_eq!(music_without_vocals.max_chunk_ms, 30_000);
        assert_eq!(music_without_vocals.overlap_ms, 2_000);
        assert!(music_without_vocals.internal_vad);

        let music = sensevoice_chunking_plan(AudioMode::Music, false, true);
        assert_eq!(music.max_chunk_ms, 30_000);
        assert_eq!(music.overlap_ms, 2_000);
        assert!(!music.internal_vad);

        let speech = sensevoice_chunking_plan(AudioMode::Speech, false, false);
        assert_eq!(speech.max_chunk_ms, 30_000);
        assert_eq!(speech.overlap_ms, 0);
        assert!(speech.internal_vad);
    }

    #[test]
    fn music_chunking_plan_uses_bounded_context_without_overlap() {
        assert_eq!(music_chunking_plan(false), (90_000, 0));
        assert_eq!(music_chunking_plan(true), (90_000, 0));
    }

    #[test]
    fn parse_vocal_separation_mode_accepts_demucs_aliases() {
        assert_eq!(
            parse_vocal_separation_mode("demucs").unwrap(),
            VocalSeparationMode::Demucs
        );
        assert_eq!(
            parse_vocal_separation_mode("on").unwrap(),
            VocalSeparationMode::Demucs
        );
        assert_eq!(
            parse_vocal_separation_mode("off").unwrap(),
            VocalSeparationMode::Off
        );
        assert!(parse_vocal_separation_mode("uvr").is_err());
    }

    #[test]
    fn find_vocals_output_prefers_largest_vocals_wav() {
        let temp_dir = tempfile::tempdir().unwrap();
        let small_dir = temp_dir.path().join("htdemucs").join("short");
        let large_dir = temp_dir.path().join("htdemucs").join("long");
        std::fs::create_dir_all(&small_dir).unwrap();
        std::fs::create_dir_all(&large_dir).unwrap();
        std::fs::write(small_dir.join("vocals.wav"), [0_u8; 4]).unwrap();
        std::fs::write(large_dir.join("vocals.wav"), [0_u8; 12]).unwrap();

        let output = find_vocals_output(temp_dir.path()).unwrap();
        assert!(output.ends_with("long/vocals.wav"));
    }

    #[test]
    fn clean_music_segments_filters_metadata_but_keeps_repeated_lyrics() {
        let cleaned = clean_music_segments(vec![
            segment_with_text("meta", 1_000, 2_000, "作詞:莫洛娜"),
            segment_with_text(
                "watermark",
                2_000,
                2_500,
                "优优独播剧场——YoYo Television Series Exclusive",
            ),
            segment_with_text("a", 3_000, 4_000, "第一句歌词"),
            segment_with_text("dup", 6_000, 7_000, " 第一句歌词 "),
            segment_with_text("later", 20_000, 21_000, "第一句歌词"),
        ]);

        assert_eq!(cleaned.len(), 3);
        assert_eq!(cleaned[0].segment_id, "a");
        assert_eq!(cleaned[1].segment_id, "dup");
        assert_eq!(cleaned[2].segment_id, "later");
    }

    #[test]
    fn clean_music_segments_drops_only_overlapping_duplicates() {
        let cleaned = clean_music_segments(vec![
            segment_with_text("a", 10_000, 13_000, "same line"),
            segment_with_text("overlap", 12_000, 14_000, " same line "),
            segment_with_text("repeat", 15_000, 16_000, "same line"),
        ]);

        assert_eq!(cleaned.len(), 2);
        assert_eq!(cleaned[0].segment_id, "a");
        assert_eq!(cleaned[1].segment_id, "repeat");
    }

    #[test]
    fn clean_music_segments_drops_adjacent_runaway_repeats() {
        let cleaned = clean_music_segments(vec![
            segment_with_text("a", 10_000, 13_000, "lost between forgotten words"),
            segment_with_text("repeat-1", 13_000, 16_000, "lost between forgotten words"),
            segment_with_text("repeat-2", 16_500, 18_000, "lost between forgotten words"),
            segment_with_text("later", 25_000, 28_000, "lost between forgotten words"),
        ]);

        assert_eq!(
            cleaned
                .iter()
                .map(|segment| segment.segment_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "later"]
        );
    }

    #[test]
    fn clean_music_segments_filters_annotations_and_trims_music_notes() {
        let cleaned = clean_music_segments(vec![
            segment_with_text("music", 0, 2_000, "[Music]"),
            segment_with_text("lyric", 2_000, 4_000, "♪ Feel the call of silence ♪"),
            segment_with_text("instrumental", 4_000, 6_000, "纯音乐"),
        ]);

        assert_eq!(cleaned.len(), 1);
        assert_eq!(cleaned[0].segment_id, "lyric");
        assert_eq!(cleaned[0].text, "Feel the call of silence");
    }

    #[test]
    fn append_chunk_segments_clamps_to_chunk_duration() {
        let chunk = audio_pipeline::AudioChunk {
            index: 2,
            start_ms: 50_000,
            end_ms: 60_000,
            wav_path: PathBuf::from("chunk.wav"),
            snr_db: 20.0,
        };
        let mut output = Vec::new();

        append_chunk_segments(
            &mut output,
            &chunk,
            vec![
                segment("inside", 1_000, 3_000),
                segment("overrun", 0, 30_000),
                segment("outside", 12_000, 13_000),
            ],
        );

        assert_eq!(output.len(), 2);
        assert_eq!(output[0].segment_id, "chunk0002-inside");
        assert_eq!((output[0].start_ms, output[0].end_ms), (51_000, 53_000));
        assert_eq!(output[1].segment_id, "chunk0002-overrun");
        assert_eq!((output[1].start_ms, output[1].end_ms), (50_000, 60_000));
    }

    #[test]
    fn merge_music_candidate_segments_fills_timeline_gaps() {
        let merged = merge_music_candidate_segments(
            vec![
                segment_with_text("primary-a", 0, 5_000, "first line"),
                segment_with_text("primary-c", 20_000, 25_000, "third line"),
            ],
            vec![segment_with_text(
                "secondary-b",
                10_000,
                15_000,
                "second line",
            )],
        );

        assert_eq!(
            merged
                .iter()
                .map(|segment| segment.segment_id.as_str())
                .collect::<Vec<_>>(),
            vec!["primary-a", "secondary-b", "primary-c"]
        );
    }

    #[test]
    fn merge_music_candidate_segments_replaces_clearly_shorter_overlap() {
        let merged = merge_music_candidate_segments(
            vec![segment_with_text("short", 30_000, 40_000, "feel call")],
            vec![segment_with_text(
                "long",
                30_000,
                40_000,
                "feel the call of silence in the rain",
            )],
        );

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].segment_id, "long");
        assert_eq!(merged[0].text, "feel the call of silence in the rain");
    }

    #[test]
    fn merge_music_candidate_segments_prefers_not_shorter_vocal_overlap() {
        let merged = merge_music_candidate_segments(
            vec![segment_with_text(
                "original",
                24_000,
                36_000,
                "Heckles carved on stone so old, Tails of flame and hens grown cold.",
            )],
            vec![segment_with_text(
                "vocals",
                25_000,
                37_000,
                "Echoes carved on stone, so old, Tails of flame and hands grown cold.",
            )],
        );

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].segment_id, "vocals");
        assert!(merged[0].text.contains("Echoes"));
        assert!(merged[0].text.contains("hands"));
    }

    #[test]
    fn music_metadata_filter_catches_common_credits_and_watermarks() {
        for text in [
            "(CC字幕製作:貝爾)",
            "作詞:李宗盛",
            "作曲:李宗盛",
            "编曲:莫洛娜",
            "混音:莫洛娜",
            "优优独播剧场——YoYo Television Series Exclusive",
            "版權所有 未經許可不得翻錄",
        ] {
            assert!(is_music_metadata_hallucination(text), "{text}");
        }
    }

    #[test]
    fn music_metadata_filter_keeps_ambiguous_lyric_like_text() {
        for text in ["她的路上走 她會離開", "第一句歌词", "想和你一起看电视"]
        {
            assert!(!is_music_metadata_hallucination(text), "{text}");
        }
    }

    #[test]
    fn diarization_assigns_speakers_to_transcript_segments() {
        let mut segments = vec![segment("seg-a", 0, 900), segment("seg-b", 1_100, 1_900)];
        let diarization = DiarizationOutput {
            speaker_count_estimate: 2,
            speaker_segments: vec![
                SpeakerSegment {
                    segment_id: "spk-a".into(),
                    start_ms: 0,
                    end_ms: 1_000,
                    speaker_id: "Speaker A".into(),
                    confidence: 0.9,
                    is_overlap: false,
                },
                SpeakerSegment {
                    segment_id: "spk-b".into(),
                    start_ms: 1_000,
                    end_ms: 2_000,
                    speaker_id: "Speaker B".into(),
                    confidence: 0.6,
                    is_overlap: true,
                },
            ],
            clustering_quality: 0.7,
        };

        apply_diarization_to_segments(&mut segments, &diarization);

        assert_eq!(segments[0].speaker_id.as_deref(), Some("Speaker A"));
        assert_eq!(segments[1].speaker_id.as_deref(), Some("Speaker B"));
        assert!(segments[1]
            .low_confidence_reasons
            .contains(&"overlapping_speech".to_string()));
        assert!(segments[1]
            .low_confidence_reasons
            .contains(&"speaker_uncertain".to_string()));
    }

    #[test]
    fn diarization_input_prefers_vad_segments_over_asr_chunks() {
        let chunks = vec![audio_chunk(0, 0, 60_000)];
        let vad = vec![vad_segment(0, 1_000), vad_segment(1_300, 2_500)];

        let prepared = prepare_diarization_segments(&vad, &chunks);

        assert_eq!(prepared.len(), 2);
        assert_eq!(prepared[0].start_ms, 0);
        assert_eq!(prepared[0].end_ms, 1_000);
        assert_eq!(prepared[1].start_ms, 1_300);
        assert_eq!(prepared[1].end_ms, 2_500);
    }

    #[test]
    fn diarization_input_windows_long_continuous_speech() {
        let chunks = vec![audio_chunk(0, 0, 10_000)];
        let vad = vec![vad_segment(0, 10_000)];

        let prepared = prepare_diarization_segments(&vad, &chunks);

        assert_eq!(
            prepared
                .iter()
                .map(|segment| (segment.start_ms, segment.end_ms))
                .collect::<Vec<_>>(),
            vec![(0, 3_000), (3_000, 6_000), (6_000, 9_000), (9_000, 10_000)]
        );
    }

    #[test]
    fn diarization_input_falls_back_to_chunks_when_vad_is_empty() {
        let chunks = vec![audio_chunk(0, 0, 2_000), audio_chunk(1, 4_000, 6_000)];

        let prepared = prepare_diarization_segments(&[], &chunks);

        assert_eq!(
            prepared
                .iter()
                .map(|segment| (segment.start_ms, segment.end_ms))
                .collect::<Vec<_>>(),
            vec![(0, 2_000), (4_000, 6_000)]
        );
    }
}
