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
    let mut info = match pipeline.analyze(file_path, file_hash) {
        Ok(info) => info,
        Err(e) => {
            let msg = format!("FATAL_ANALYZE: {e:#}");
            let _ = std::fs::write("/tmp/audraflow-crash.log", &msg);
            return Err(e);
        }
    };
    let _ = std::fs::write("/tmp/audraflow-step.log", "AFTER_ANALYZE_OK");

    // ── Step 2: Decode to 16kHz mono WAV ──────────────────────────────────
    log::info!("[2/4] Decoding {label}...");
    let wav_path = pipeline
        .decode_to_wav(file_path)
        .map_err(|e| {
            use std::io::Write;
            eprintln!("FATAL_DECODE: {e:#}");
            std::io::stderr().flush().ok();
            e
        })?;

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
