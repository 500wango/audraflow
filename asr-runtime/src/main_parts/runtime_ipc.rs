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
