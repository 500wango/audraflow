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
