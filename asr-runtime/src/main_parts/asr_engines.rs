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
