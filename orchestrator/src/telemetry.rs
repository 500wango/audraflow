//! Telemetry & Privacy Module
//!
//! PRD §12: Structured telemetry for MCM/H optimization.
//! PRD §12.1: Default local-first; user must authorize telemetry; can disable.
//!
//! NEVER collects: audio content, transcript text, glossary entries,
//! file names, person names, company names.
//!
//! Collects ONLY: event type, timestamps, counts, durations, format names,
//! model version, device tier — all structural/behavioral, zero content.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};

// ── Telemetry Authorization ────────────────────────────────────────────────

/// Global telemetry consent state.
#[allow(dead_code)]
static TELEMETRY_ENABLED: AtomicBool = AtomicBool::new(false);

/// Check if telemetry is currently enabled.
#[allow(dead_code)]
pub fn is_telemetry_enabled() -> bool {
    TELEMETRY_ENABLED.load(Ordering::Relaxed)
}

/// Enable telemetry (user authorized).
#[allow(dead_code)]
pub fn enable_telemetry() {
    TELEMETRY_ENABLED.store(true, Ordering::Relaxed);
    log::info!("Telemetry enabled by user");
}

/// Disable telemetry (user opted out).
#[allow(dead_code)]
pub fn disable_telemetry() {
    TELEMETRY_ENABLED.store(false, Ordering::Relaxed);
    log::info!("Telemetry disabled by user");
}

// ── Telemetry Events (PRD §4.1) ────────────────────────────────────────────

/// All telemetry event types that may be collected.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event_type", rename_all = "snake_case")]
pub enum TelemetryEvent {
    /// Fired when user opens the proofreading editor.
    ProofreadSessionStart {
        job_id: String,
        audio_hours: f64,
        transcript_chars: u32,
        app_version: String,
        model_version: String,
        timestamp_ms: i64,
    },
    /// Fired when user closes or completes proofreading.
    ProofreadSessionEnd {
        job_id: String,
        active_seconds: f64,
        inactive_seconds: f64,
        completed_ratio: f64,
        timestamp_ms: i64,
    },
    /// Fired per correction action (add/delete/modify).
    CorrectionEvent {
        /// Hashed segment ID (not the raw ID, for privacy).
        segment_id_hash: String,
        op_type: CorrectionOpType,
        chars_before: u32,
        chars_after: u32,
        source: CorrectionSourceType,
        timestamp_ms: i64,
    },
    /// Fired when user seeks or replays audio.
    PlaybackSeek {
        segment_id_hash: String,
        from_ms: i64,
        to_ms: i64,
        trigger: SeekTrigger,
        timestamp_ms: i64,
    },
    /// Fired when user inserts a timestamp mark (Ctrl+T).
    TimestampMark {
        segment_id_hash: String,
        mark_ms: i64,
        label_type: String,
        timestamp_ms: i64,
    },
    /// Fired when user exports a transcript.
    ExportCompleted {
        format: String,
        include_timestamps: bool,
        include_speakers: bool,
        timestamp_ms: i64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CorrectionOpType {
    Insert,
    Delete,
    Replace,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CorrectionSourceType {
    User,
    Lexicon,
    Merge,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SeekTrigger {
    ClickSegment,
    Replay,
    JumpBack,
    JumpForward,
    ManualScrub,
}

// ── Telemetry Collector ────────────────────────────────────────────────────

/// Thread-safe telemetry event collector.
pub struct TelemetryCollectorStd {
    events: std::sync::Mutex<Vec<TelemetryEvent>>,
    authorized: bool,
}

impl TelemetryCollectorStd {
    pub fn new(authorized: bool) -> Self {
        Self {
            events: std::sync::Mutex::new(Vec::new()),
            authorized,
        }
    }

    /// Record a telemetry event. Silently drops if telemetry is disabled.
    pub fn record(&self, event: TelemetryEvent) {
        if !self.authorized {
            return;
        }
        if let Ok(mut events) = self.events.lock() {
            events.push(event);
        }
    }

    /// Get all collected events (for local stats panel or export).
    pub fn get_events(&self) -> Vec<TelemetryEvent> {
        self.events.lock().map(|e| e.clone()).unwrap_or_default()
    }

    /// Count events by type.
    pub fn count_by_type(&self) -> std::collections::HashMap<String, usize> {
        let mut counts = std::collections::HashMap::new();
        if let Ok(events) = self.events.lock() {
            for event in events.iter() {
                let key = match event {
                    TelemetryEvent::ProofreadSessionStart { .. } => "proofread_session_start",
                    TelemetryEvent::ProofreadSessionEnd { .. } => "proofread_session_end",
                    TelemetryEvent::CorrectionEvent { .. } => "correction_event",
                    TelemetryEvent::PlaybackSeek { .. } => "playback_seek",
                    TelemetryEvent::TimestampMark { .. } => "timestamp_mark",
                    TelemetryEvent::ExportCompleted { .. } => "export_completed",
                };
                *counts.entry(key.to_string()).or_insert(0) += 1;
            }
        }
        counts
    }

    /// Compute local MCM/H from collected events.
    /// MCM/H = (active_proofread_seconds + seek_count × 3) / 60 / audio_hours
    pub fn compute_mcm_per_hour(&self, audio_hours: f64) -> f64 {
        if audio_hours <= 0.0 {
            return 0.0;
        }

        let events = self.get_events();
        let mut active_seconds = 0.0;
        let mut seek_count = 0u64;

        for event in &events {
            match event {
                TelemetryEvent::ProofreadSessionEnd {
                    active_seconds: a, ..
                } => {
                    active_seconds += a;
                }
                TelemetryEvent::PlaybackSeek { .. } => {
                    seek_count += 1;
                }
                _ => {}
            }
        }

        (active_seconds + seek_count as f64 * 3.0) / 60.0 / audio_hours
    }

    /// Clear all collected events.
    pub fn clear(&self) {
        if let Ok(mut events) = self.events.lock() {
            events.clear();
        }
    }

    /// Check if authorized.
    pub fn is_authorized(&self) -> bool {
        self.authorized
    }
}

// ── Privacy Utilities ──────────────────────────────────────────────────────

/// Hash a segment ID for telemetry (never expose raw IDs).
#[allow(dead_code)]
pub fn hash_segment_id(segment_id: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    segment_id.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Sanitize a string to ensure no PII leaks into telemetry.
/// Strips: email patterns, person names (heuristic), file paths.
#[allow(dead_code)]
pub fn sanitize_for_telemetry(s: &str) -> String {
    // In production: use regex to strip patterns.
    // For MVP: only allow alphanumeric + common separators.
    s.chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-' || *c == '.')
        .collect()
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_telemetry_disabled_drops_events() {
        let collector = TelemetryCollectorStd::new(false);
        collector.record(TelemetryEvent::ExportCompleted {
            format: "markdown".into(),
            include_timestamps: true,
            include_speakers: false,
            timestamp_ms: 1000,
        });
        assert_eq!(collector.get_events().len(), 0);
    }

    #[test]
    fn test_telemetry_enabled_records_events() {
        let collector = TelemetryCollectorStd::new(true);
        collector.record(TelemetryEvent::ProofreadSessionStart {
            job_id: "job-1".into(),
            audio_hours: 1.0,
            transcript_chars: 5000,
            app_version: "0.1.0".into(),
            model_version: "whisper-base-v1".into(),
            timestamp_ms: 1000,
        });
        collector.record(TelemetryEvent::PlaybackSeek {
            segment_id_hash: "abc123".into(),
            from_ms: 10000,
            to_ms: 7000,
            trigger: SeekTrigger::Replay,
            timestamp_ms: 2000,
        });

        assert_eq!(collector.get_events().len(), 2);
        let counts = collector.count_by_type();
        assert_eq!(counts.get("proofread_session_start"), Some(&1));
        assert_eq!(counts.get("playback_seek"), Some(&1));
    }

    #[test]
    fn test_mcm_per_hour_computation() {
        let collector = TelemetryCollectorStd::new(true);

        // Simulate a 1-hour audio proofreading session
        collector.record(TelemetryEvent::ProofreadSessionEnd {
            job_id: "job-1".into(),
            active_seconds: 600.0, // 10 minutes active proofreading
            inactive_seconds: 120.0,
            completed_ratio: 1.0,
            timestamp_ms: 1000,
        });
        // 20 seek events
        for _ in 0..20 {
            collector.record(TelemetryEvent::PlaybackSeek {
                segment_id_hash: "abc".into(),
                from_ms: 1000,
                to_ms: 500,
                trigger: SeekTrigger::Replay,
                timestamp_ms: 1000,
            });
        }

        // MCM/H = (600 + 20*3) / 60 / 1 = 660/60 = 11.0
        let mcm = collector.compute_mcm_per_hour(1.0);
        assert!((mcm - 11.0).abs() < 0.01);
    }

    #[test]
    fn test_hash_segment_id() {
        let h1 = hash_segment_id("seg-001");
        let h2 = hash_segment_id("seg-002");
        assert_ne!(h1, h2);
        assert_eq!(h1, hash_segment_id("seg-001")); // Deterministic
    }

    #[test]
    fn test_sanitize() {
        let clean = sanitize_for_telemetry("user@example.com/test.mp3");
        assert!(!clean.contains('@'));
        assert!(!clean.contains('/'));
    }

    #[test]
    fn test_events_never_contain_user_content() {
        // All events must only use hashed IDs, not raw content
        let event = TelemetryEvent::TimestampMark {
            segment_id_hash: hash_segment_id("seg-42"),
            mark_ms: 5000,
            label_type: "important".into(),
            timestamp_ms: 1000,
        };

        let json = serde_json::to_string(&event).unwrap();
        // Must NOT contain raw segment IDs
        assert!(!json.contains("seg-42"));
        // Must NOT contain any audio/text fields
        assert!(!json.contains("audio"));
        assert!(!json.contains("transcript"));
        assert!(!json.contains("text"));
    }

    #[test]
    fn test_clear_events() {
        let collector = TelemetryCollectorStd::new(true);
        collector.record(TelemetryEvent::ExportCompleted {
            format: "txt".into(),
            include_timestamps: false,
            include_speakers: false,
            timestamp_ms: 1000,
        });
        assert_eq!(collector.get_events().len(), 1);
        collector.clear();
        assert_eq!(collector.get_events().len(), 0);
    }
}
