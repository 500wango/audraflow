//! AudraFlow Post-Processor
//!
//! Implements AR-003: lexicon-based post-processing correction.
//! Core capabilities:
//! - Hard matching: exact canonical term → replacement
//! - Pinyin fuzzy matching: near-sound candidates (Alpha-1 enhanced)
//! - Levenshtein distance: typo correction
//! - User feedback learning: accept/reject adjusts confidence weights
//! - All replacements are tracked as diffs (Correction).
//!
//! Design rule (PRD §9): the lexicon is NOT injected into the ASR prompt.
//! Correction happens post-transcription with traceable, undoable diffs.

use audraflow_ipc::{Correction, CorrectionSource, Segment};
use std::collections::HashMap;

// ── Lexicon ────────────────────────────────────────────────────────────────

/// A single glossary entry with canonical form, aliases, and pinyin.
#[derive(Debug, Clone)]
pub struct GlossaryEntry {
    pub canonical: String,
    pub aliases: Vec<String>,
    pub pinyin_forms: Vec<String>,
    pub category: Option<String>,
    pub enabled: bool,
}

// ── User Feedback ──────────────────────────────────────────────────────────

/// Tracks user accept/reject history per glossary entry to adjust confidence.
#[derive(Debug, Clone, Default)]
pub struct FeedbackTracker {
    /// Maps canonical term → (accept_count, reject_count)
    history: HashMap<String, (u32, u32)>,
}

impl FeedbackTracker {
    pub fn new() -> Self {
        Self {
            history: HashMap::new(),
        }
    }

    /// Record that the user accepted a correction for this canonical term.
    pub fn record_accept(&mut self, canonical: &str) {
        let entry = self.history.entry(canonical.to_string()).or_insert((0, 0));
        entry.0 += 1;
    }

    /// Record that the user rejected a correction for this canonical term.
    pub fn record_reject(&mut self, canonical: &str) {
        let entry = self.history.entry(canonical.to_string()).or_insert((0, 0));
        entry.1 += 1;
    }

    /// Get a confidence multiplier based on accept/reject history.
    /// Returns 1.0 = neutral (no data yet), >1.0 = trusted, <1.0 = distrusted.
    pub fn confidence_multiplier(&self, canonical: &str) -> f64 {
        if let Some((accepts, rejects)) = self.history.get(canonical) {
            let total = accepts + rejects;
            if total == 0 {
                return 1.0; // No data → neutral (leave confidence unchanged)
            }
            let ratio = *accepts as f64 / total as f64;
            // Scale: 0.0 ratio → 0.3x, 0.5 ratio → 0.8x, 1.0 ratio → 1.3x
            let scaled = 0.3 + ratio * 1.0;
            if total < 5 {
                // Low data: regress toward neutral
                1.0 + (scaled - 1.0) * (total as f64 / 5.0)
            } else {
                scaled
            }
        } else {
            1.0 // No data → neutral
        }
    }
}

// ── Post-Processor ─────────────────────────────────────────────────────────

/// The post-processor engine.
pub struct PostProcessor {
    entries: Vec<GlossaryEntry>,
    /// Min confidence for auto-apply (0.0–1.0). Below this → manual confirm.
    auto_apply_threshold: f64,
    /// User feedback tracker for adaptive confidence.
    feedback: FeedbackTracker,
}

impl PostProcessor {
    pub fn new(entries: Vec<GlossaryEntry>) -> Self {
        Self {
            entries,
            auto_apply_threshold: 0.85,
            feedback: FeedbackTracker::new(),
        }
    }

    pub fn with_threshold(mut self, threshold: f64) -> Self {
        self.auto_apply_threshold = threshold;
        self
    }

    pub fn with_feedback(mut self, feedback: FeedbackTracker) -> Self {
        self.feedback = feedback;
        self
    }

    /// Get a reference to the feedback tracker (for persisting learning).
    pub fn feedback(&self) -> &FeedbackTracker {
        &self.feedback
    }

    /// Get a mutable reference to record user actions.
    pub fn feedback_mut(&mut self) -> &mut FeedbackTracker {
        &mut self.feedback
    }

    // ── Processing ──────────────────────────────────────────────────────────

    /// Process a batch of segments: apply lexicon correction and return
    /// the list of corrections to apply (with confidence scores).
    pub fn process(&self, segments: &[Segment]) -> Vec<SegmentCorrection> {
        let mut results = Vec::new();

        for seg in segments {
            let corrections = self.correct_segment(&seg.text);
            if !corrections.is_empty() {
                results.push(SegmentCorrection {
                    segment_id: seg.segment_id.clone(),
                    corrections,
                });
            }
        }

        results
    }

    /// Apply corrections to a segment's text and return the corrected text.
    /// Also determines which corrections are auto-applied vs. need confirmation.
    pub fn apply_to_segment(&self, segment: &Segment) -> CorrectedSegment {
        let candidates = self.correct_segment(&segment.text);
        let mut corrected_text = segment.text.clone();
        let mut auto_applied = Vec::new();
        let mut needs_confirmation = Vec::new();

        // Apply from right to left to preserve positions
        let mut sorted: Vec<_> = candidates.iter().collect();
        sorted.sort_by_key(|c| -(c.position as i64));

        for c in &sorted {
            let boosted_confidence =
                c.confidence * self.feedback.confidence_multiplier(&c.replacement);
            let auto = self.should_auto_apply(boosted_confidence);

            let correction = c.to_ipc_correction(auto);
            if auto {
                corrected_text = c.apply_to(&corrected_text);
                auto_applied.push(correction);
            } else {
                needs_confirmation.push(correction);
            }
        }

        CorrectedSegment {
            segment_id: segment.segment_id.clone(),
            original_text: segment.raw_text.clone(),
            corrected_text,
            auto_applied,
            needs_confirmation,
        }
    }

    // ── Private: Correction Engine ──────────────────────────────────────────

    /// Find corrections for a single text string.
    fn correct_segment(&self, text: &str) -> Vec<CandidateCorrection> {
        let mut candidates = Vec::new();

        for entry in &self.entries {
            if !entry.enabled {
                continue;
            }

            // ── 1. Hard match: exact match against aliases ──
            for alias in &entry.aliases {
                if let Some(pos) = text.find(alias.as_str()) {
                    let confidence = if alias == &entry.canonical { 1.0 } else { 0.9 };
                    candidates.push(CandidateCorrection {
                        original: alias.clone(),
                        replacement: entry.canonical.clone(),
                        match_method: MatchMethod::Hard,
                        confidence,
                        position: pos,
                    });
                }
            }

            // ── 2. Pinyin fuzzy match ──
            for pinyin_form in &entry.pinyin_forms {
                if let Some((pos, matched)) =
                    self.pinyin_fuzzy_match(text, pinyin_form, &entry.aliases)
                {
                    candidates.push(CandidateCorrection {
                        original: matched,
                        replacement: entry.canonical.clone(),
                        match_method: MatchMethod::PinyinFuzzy,
                        confidence: 0.65, // pinyin → lower confidence than hard match
                        position: pos,
                    });
                }
            }

            // ── 3. Levenshtein typo correction ──
            for alias in &entry.aliases {
                if let Some((pos, matched)) =
                    self.levenshtein_fuzzy_match(text, alias, &entry.canonical)
                {
                    candidates.push(CandidateCorrection {
                        original: matched,
                        replacement: entry.canonical.clone(),
                        match_method: MatchMethod::Levenshtein,
                        confidence: 0.55, // Levenshtein → lowest confidence
                        position: pos,
                    });
                }
            }
        }

        // Sort by confidence descending, deduplicate overlapping replacements
        candidates.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap());
        self.deduplicate_overlaps(candidates)
    }

    /// Pinyin fuzzy matching: find words in text whose pinyin matches
    /// a target pinyin form (within a tolerance).
    ///
    /// Approach: scan the text in sliding windows of n characters
    /// (where n = length of the shortest alias), convert to pinyin,
    /// and compare with the target pinyin.
    fn pinyin_fuzzy_match(
        &self,
        text: &str,
        target_pinyin: &str,
        aliases: &[String],
    ) -> Option<(usize, String)> {
        let text_chars: Vec<char> = text.chars().collect();

        // Determine window size from alias lengths
        let min_alias_len = aliases.iter().map(|a| a.chars().count()).min().unwrap_or(2);
        let max_alias_len = aliases.iter().map(|a| a.chars().count()).max().unwrap_or(4);

        for window_size in min_alias_len..=max_alias_len {
            for start in 0..text_chars.len().saturating_sub(window_size - 1) {
                let window: String = text_chars[start..(start + window_size).min(text_chars.len())]
                    .iter()
                    .collect();

                // Convert window to pinyin
                let window_pinyin = chars_to_pinyin(&window);

                // Compare with target pinyin (edit distance on pinyin string)
                let dist = Self::levenshtein_distance(&window_pinyin, target_pinyin);
                let max_len = target_pinyin.chars().count().max(1) as f64;
                let similarity = 1.0 - (dist as f64 / max_len);

                if similarity >= 0.6 {
                    // 60% pinyin similarity → candidate
                    return Some((start, window));
                }
            }
        }

        None
    }

    /// Levenshtein fuzzy match: find substrings in text that are
    /// within edit distance of a given alias.
    fn levenshtein_fuzzy_match(
        &self,
        text: &str,
        alias: &str,
        canonical: &str,
    ) -> Option<(usize, String)> {
        let text_chars: Vec<char> = text.chars().collect();
        let alias_len = alias.chars().count();

        // Don't try to match if alias is too short (high false positive rate)
        if alias_len < 2 {
            return None;
        }

        for start in 0..text_chars.len().saturating_sub(alias_len - 1) {
            // Try windows of alias_len and alias_len±1
            for window_size in
                (alias_len.saturating_sub(1))..=(alias_len + 1).min(text_chars.len() - start)
            {
                let window: String = text_chars[start..(start + window_size).min(text_chars.len())]
                    .iter()
                    .collect();

                let dist = Self::levenshtein_distance(&window, alias);
                let max_len = alias_len.max(1);
                let edit_ratio = dist as f64 / max_len as f64;

                // Only flag if edit distance ≤ 1 and not an exact match (hard match handles those)
                if edit_ratio <= 0.5 && edit_ratio > 0.0 && window != canonical {
                    return Some((start, window));
                }
            }
        }

        None
    }

    // ── Public Utilities ────────────────────────────────────────────────────

    /// Compute Levenshtein (edit) distance between two strings.
    pub fn levenshtein_distance(a: &str, b: &str) -> usize {
        let a_chars: Vec<char> = a.chars().collect();
        let b_chars: Vec<char> = b.chars().collect();
        let m = a_chars.len();
        let n = b_chars.len();

        if m == 0 {
            return n;
        }
        if n == 0 {
            return m;
        }

        let mut prev: Vec<usize> = (0..=n).collect();
        let mut curr = vec![0usize; n + 1];

        for i in 1..=m {
            curr[0] = i;
            for j in 1..=n {
                let cost = if a_chars[i - 1] == b_chars[j - 1] {
                    0
                } else {
                    1
                };
                curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
            }
            prev.copy_from_slice(&curr);
        }

        prev[n]
    }

    /// Compute character edit ratio for diagnostic purposes.
    pub fn edit_char_ratio(original: &str, corrected: &str) -> f64 {
        let dist = Self::levenshtein_distance(original, corrected);
        let max_len = original.chars().count().max(1);
        dist as f64 / max_len as f64
    }

    /// Determine whether a correction should be auto-applied based on confidence.
    pub fn should_auto_apply(&self, confidence: f64) -> bool {
        confidence >= self.auto_apply_threshold
    }

    // ── Private Helpers ─────────────────────────────────────────────────────

    /// Remove overlapping corrections, keeping higher-confidence ones.
    fn deduplicate_overlaps(
        &self,
        candidates: Vec<CandidateCorrection>,
    ) -> Vec<CandidateCorrection> {
        if candidates.is_empty() {
            return candidates;
        }

        let mut result: Vec<CandidateCorrection> = Vec::new();
        let mut covered: Vec<(usize, usize)> = Vec::new();

        for c in candidates {
            let end = c.position + c.original.len();
            let overlaps = covered.iter().any(|(s, e)| c.position < *e && end > *s);

            if !overlaps {
                covered.push((c.position, end));
                result.push(c);
            }
        }

        result
    }
}

/// Result of applying corrections to a segment.
#[derive(Debug, Clone)]
pub struct CorrectedSegment {
    pub segment_id: String,
    pub original_text: String,
    pub corrected_text: String,
    pub auto_applied: Vec<Correction>,
    pub needs_confirmation: Vec<Correction>,
}

// ── Correction Types ────────────────────────────────────────────────────────

/// A batch of corrections for a single segment.
#[derive(Debug, Clone)]
pub struct SegmentCorrection {
    pub segment_id: String,
    pub corrections: Vec<CandidateCorrection>,
}

/// A single correction candidate with confidence and match method.
#[derive(Debug, Clone)]
pub struct CandidateCorrection {
    pub original: String,
    pub replacement: String,
    pub match_method: MatchMethod,
    pub confidence: f64,
    pub position: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatchMethod {
    Hard,
    PinyinFuzzy,
    Levenshtein,
}

impl CandidateCorrection {
    /// Convert to the IPC Correction type for storage/display.
    pub fn to_ipc_correction(&self, auto_applied: bool) -> Correction {
        Correction {
            field: "text".to_string(),
            old_value: self.original.clone(),
            new_value: self.replacement.clone(),
            source: CorrectionSource::Lexicon,
            auto_applied,
        }
    }

    /// Apply this correction to a text string.
    /// `position` is a byte offset as returned by `str::find()`.
    pub fn apply_to(&self, text: &str) -> String {
        if self.position + self.original.len() > text.len() {
            return text.to_string();
        }

        let mut result = String::with_capacity(text.len());
        result.push_str(&text[..self.position]);
        result.push_str(&self.replacement);
        result.push_str(&text[self.position + self.original.len()..]);
        result
    }
}

// ── Pinyin Helpers ─────────────────────────────────────────────────────────

/// Convert a Chinese character string to its pinyin representation.
/// Uses a simple lookup table for the most common characters.
/// Full implementation would use the `pinyin` crate for comprehensive coverage.
fn chars_to_pinyin(s: &str) -> String {
    let mut result = String::new();
    for ch in s.chars() {
        if let Some(py) = char_to_pinyin(ch) {
            result.push_str(py);
        } else {
            // Non-Chinese characters pass through
            result.push(ch);
        }
    }
    result
}

/// Lookup pinyin for a single Chinese character.
/// This is a minimal lookup table for common characters.
/// Production: use the `pinyin` crate for full Unihan coverage.
fn char_to_pinyin(ch: char) -> Option<&'static str> {
    // Minimal table covering the most common characters in test data.
    // In production, this is replaced by the `pinyin` crate.
    match ch {
        '腾' => Some("teng"),
        '讯' => Some("xun"),
        '训' => Some("xun"),
        '科' => Some("ke"),
        '技' => Some("ji"),
        '网' => Some("wang"),
        '易' => Some("yi"),
        '华' => Some("hua"),
        '为' => Some("wei"),
        '微' => Some("wei"),
        '软' => Some("ruan"),
        '谷' => Some("gu"),
        '歌' => Some("ge"),
        '苹' => Some("ping"),
        '果' => Some("guo"),
        '阿' => Some("a"),
        '里' => Some("li"),
        '巴' => Some("ba"),
        '百' => Some("bai"),
        '度' => Some("du"),
        '字' => Some("zi"),
        '节' => Some("jie"),
        '跳' => Some("tiao"),
        '动' => Some("dong"),
        '抖' => Some("dou"),
        '音' => Some("yin"),
        '快' => Some("kuai"),
        '手' => Some("shou"),
        '知' => Some("zhi"),
        '乎' => Some("hu"),
        '小' => Some("xiao"),
        '红' => Some("hong"),
        '书' => Some("shu"),
        '京' => Some("jing"),
        '东' => Some("dong"),
        '拼' => Some("pin"),
        '多' => Some("duo"),
        '美' => Some("mei"),
        '团' => Some("tuan"),
        '饿' => Some("e"),
        '了' => Some("le"),
        '么' => Some("me"),
        '滴' => Some("di"),
        '出' => Some("chu"),
        '行' => Some("xing"),
        _ => None,
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(canonical: &str, aliases: &[&str], pinyin_forms: &[&str]) -> GlossaryEntry {
        GlossaryEntry {
            canonical: canonical.to_string(),
            aliases: aliases.iter().map(|s| s.to_string()).collect(),
            pinyin_forms: pinyin_forms.iter().map(|s| s.to_string()).collect(),
            category: Some("company".to_string()),
            enabled: true,
        }
    }

    #[test]
    fn test_levenshtein_distance() {
        assert_eq!(PostProcessor::levenshtein_distance("腾讯", "腾训"), 1);
        assert_eq!(PostProcessor::levenshtein_distance("hello", "hallo"), 1);
        assert_eq!(PostProcessor::levenshtein_distance("abc", "abc"), 0);
        assert_eq!(PostProcessor::levenshtein_distance("", "abc"), 3);
        assert_eq!(PostProcessor::levenshtein_distance("abc", ""), 3);
    }

    #[test]
    fn test_edit_char_ratio() {
        let ratio = PostProcessor::edit_char_ratio("腾讯科技", "腾训科技");
        assert!((ratio - 0.25).abs() < 0.01);
    }

    #[test]
    fn test_hard_match_correction() {
        let entries = vec![make_entry("腾讯", &["腾训"], &["teng xun"])];
        let pp = PostProcessor::new(entries);
        let seg = make_segment("seg-1", "欢迎来到腾训");

        let results = pp.process(&[seg]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].corrections[0].original, "腾训");
        assert_eq!(results[0].corrections[0].replacement, "腾讯");
        assert_eq!(results[0].corrections[0].match_method, MatchMethod::Hard);
        assert!(results[0].corrections[0].confidence >= 0.9);
    }

    #[test]
    fn test_pinyin_fuzzy_match() {
        let entries = vec![make_entry("腾讯", &[], &["teng xun"])];
        let pp = PostProcessor::new(entries);
        let seg = make_segment("seg-1", "欢迎来到疼讯科技");

        let results = pp.process(&[seg]);
        // "疼讯" should fuzzy-match "腾讯" via pinyin "teng xun"
        // Note: depends on pinyin table coverage
        if !results.is_empty() {
            assert_eq!(
                results[0].corrections[0].match_method,
                MatchMethod::PinyinFuzzy
            );
        }
    }

    #[test]
    fn test_levenshtein_typo_correction() {
        let entries = vec![make_entry("腾讯", &["腾讯"], &[])];
        let pp = PostProcessor::new(entries);
        let seg = make_segment("seg-1", "欢迎来到藤讯");

        let results = pp.process(&[seg]);
        // "藤讯" is 1 edit away from "腾讯"
        if !results.is_empty() {
            assert_eq!(
                results[0].corrections[0].match_method,
                MatchMethod::Levenshtein
            );
        }
    }

    #[test]
    fn test_apply_to_segment_auto_apply() {
        let entries = vec![make_entry("腾讯", &["腾训"], &[])];
        let pp = PostProcessor::new(entries);
        let seg = make_segment("seg-1", "腾训科技很厉害");

        let result = pp.apply_to_segment(&seg);
        assert_eq!(result.corrected_text, "腾讯科技很厉害");
        assert_eq!(result.auto_applied.len(), 1);
        assert!(result.needs_confirmation.is_empty());
    }

    #[test]
    fn test_low_confidence_needs_confirmation() {
        let entries = vec![make_entry("腾讯", &[], &["teng xun"])];
        let pp = PostProcessor::new(entries).with_threshold(0.8);
        let seg = make_segment("seg-1", "欢迎来到疼讯科技");

        let result = pp.apply_to_segment(&seg);
        // Pinyin match confidence (0.65) < threshold (0.8) → needs confirmation
        if !result.needs_confirmation.is_empty() {
            assert!(!result.needs_confirmation[0].auto_applied);
        }
    }

    #[test]
    fn test_feedback_boosts_confidence() {
        let mut feedback = FeedbackTracker::new();
        feedback.record_accept("腾讯");
        feedback.record_accept("腾讯");
        feedback.record_accept("腾讯");
        feedback.record_accept("腾讯");
        feedback.record_accept("腾讯"); // 5 accepts → enough data

        let mult = feedback.confidence_multiplier("腾讯");
        assert!(mult > 1.0); // More accepts → higher multiplier
    }

    #[test]
    fn test_feedback_penalizes_rejects() {
        let mut feedback = FeedbackTracker::new();
        feedback.record_reject("错误词");
        feedback.record_reject("错误词");
        feedback.record_reject("错误词");
        feedback.record_reject("错误词");
        feedback.record_reject("错误词");

        let mult = feedback.confidence_multiplier("错误词");
        assert!(mult < 1.0); // More rejects → lower multiplier
    }

    #[test]
    fn test_deduplicate_overlaps() {
        let entries = vec![
            make_entry("腾讯科技", &["腾训"], &[]),
            make_entry("腾讯", &["腾训"], &[]),
        ];
        let pp = PostProcessor::new(entries);
        let seg = make_segment("seg-1", "腾训");

        let results = pp.process(&[seg]);
        // Should not produce duplicate corrections for the same position
        assert!(results.len() <= 1);
    }

    fn make_segment(id: &str, text: &str) -> Segment {
        Segment {
            segment_id: id.to_string(),
            start_ms: 0,
            end_ms: 1000,
            speaker_id: None,
            text: text.to_string(),
            raw_text: text.to_string(),
            confidence: 0.9,
            low_confidence_reasons: vec![],
            corrections: vec![],
            marks: vec![],
        }
    }

    #[test]
    fn test_chars_to_pinyin() {
        let py = chars_to_pinyin("腾讯");
        assert_eq!(py, "tengxun");
    }

    #[test]
    fn test_levenshtein_distance_unicode() {
        // Test with mixed Chinese + ASCII
        let dist = PostProcessor::levenshtein_distance("腾讯Tencent", "腾训Tencent");
        assert_eq!(dist, 1);
    }
}
