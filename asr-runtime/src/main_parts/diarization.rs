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
