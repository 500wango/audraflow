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
