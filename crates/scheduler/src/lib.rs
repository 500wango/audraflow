//! AudraFlow Adaptive Scheduler
//!
//! PRD §7: The system bears the technical decisions. The user only expresses
//! delivery intent. Default target: fastest deliverable draft.
//! "追求极致准确" is the only explicit quality toggle.
//!
//! Core principle: don't let users choose speed/balanced/accuracy modes.
//! The scheduler auto-selects model, beam, chunk, parallelism, and
//! post-processing intensity based on audio analysis and device capability.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Scheduler Input ────────────────────────────────────────────────────────

/// All inputs the scheduler uses to make decisions.
/// Based on PRD §7.2 scheduler decision rules table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerInput {
    // ── Audio characteristics ──
    pub duration_seconds: f64,
    pub snr_db: Option<f64>,
    pub speech_density: Option<f64>, // 0.0–1.0
    pub estimated_speaker_count: u32,
    pub is_high_noise: bool,

    // ── Device capability ──
    pub device_tier: DeviceTier,
    pub cuda_available: bool,
    pub vram_gb: Option<f64>,
    pub cpu_cores: u32,

    // ── User preference ──
    pub extreme_accuracy: bool,

    // ── System state ──
    pub model_cached: bool,
    pub cold_start_seconds: Option<f64>,
}

// ── Scheduler Output ───────────────────────────────────────────────────────

/// The scheduler's plan for how to process a specific audio file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerPlan {
    /// Unique plan ID for traceability and experiment reproducibility.
    pub plan_id: String,

    /// Which model variant to use.
    pub model_size: ModelSize,

    /// Beam search width (higher = more accurate, slower).
    pub beam_size: u32,

    /// Chunk duration in milliseconds for parallel processing.
    pub chunk_duration_ms: u32,

    /// Maximum number of chunks to process in parallel.
    pub max_parallelism: u32,

    /// Whether to enable noise reduction preprocessing.
    pub noise_reduction_enabled: bool,

    /// Post-processing intensity.
    pub post_processing: PostProcessingLevel,

    /// Diarization (speaker separation) mode.
    pub diarization_mode: DiarizationMode,

    /// Whether low-confidence segments should be re-run.
    pub rerun_low_confidence: bool,

    /// Estimated processing time (seconds).
    pub estimated_duration_seconds: f64,

    /// Human-readable explanation of the decision.
    pub explanation: String,

    /// Why this plan was downgraded from a previous plan, if applicable.
    pub fallback_reason: Option<String>,

    /// All input signals used (for log reproducibility).
    pub input_signals: SchedulerInput,
}

// ── Enum Types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum DeviceTier {
    /// Windows GPU standard: RTX 4060/4070, 8GB+ VRAM
    GpuStandard,
    /// Windows GPU entry: RTX 3060/4050, 6GB+ VRAM
    GpuEntry,
    /// CPU-only: 8+ cores, 16GB+ RAM
    CpuOnly,
    /// Below minimum spec
    LowSpec,
}

impl DeviceTier {
    pub fn classify(cuda_available: bool, vram_gb: Option<f64>, cpu_cores: u32) -> Self {
        if cuda_available {
            match vram_gb {
                Some(vram) if vram >= 8.0 => DeviceTier::GpuStandard,
                Some(vram) if vram >= 6.0 => DeviceTier::GpuEntry,
                _ => DeviceTier::GpuEntry, // CUDA but unknown VRAM → assume entry
            }
        } else if cpu_cores >= 8 {
            DeviceTier::CpuOnly
        } else {
            DeviceTier::LowSpec
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ModelSize {
    /// ~75MB, fastest, lower accuracy
    Tiny,
    /// ~150MB
    Base,
    /// ~500MB, balanced
    Small,
    /// ~1.5GB, good accuracy
    Medium,
    /// ~3GB, best accuracy, slowest
    Large,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PostProcessingLevel {
    /// Only basic punctuation and sentence segmentation.
    Minimal,
    /// Standard lexicon correction + confidence marking.
    Standard,
    /// Aggressive: stronger lexicon, re-evaluation of ambiguous segments.
    Aggressive,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum DiarizationMode {
    /// No speaker separation (single-speaker audio).
    None,
    /// Basic clustering: Speaker A/B output.
    Basic,
    /// Enhanced clustering for 3-5 speakers with overlap marking.
    Enhanced,
}

// ── Audio Duration Buckets ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
enum DurationBucket {
    Short,  // < 15 min
    Medium, // 15–60 min
    Long,   // > 60 min
}

impl DurationBucket {
    fn from_seconds(s: f64) -> Self {
        if s < 900.0 {
            DurationBucket::Short
        } else if s <= 3600.0 {
            DurationBucket::Medium
        } else {
            DurationBucket::Long
        }
    }
}

// ── The Scheduler ──────────────────────────────────────────────────────────

/// The adaptive scheduling engine.
pub struct Scheduler;

impl Scheduler {
    /// Create a scheduling plan based on audio analysis, device capability,
    /// and user preferences.
    ///
    /// This is the core decision function — every job passes through here.
    /// The output plan is logged with a unique `plan_id` for reproducibility.
    pub fn plan(input: &SchedulerInput) -> SchedulerPlan {
        let plan_id = Uuid::new_v4().to_string();
        let bucket = DurationBucket::from_seconds(input.duration_seconds);

        let (
            model_size,
            beam_size,
            chunk_ms,
            parallelism,
            noise_reduction,
            post_processing,
            diarization,
            rerun_low_conf,
            explanation,
        ) = Self::decide(input, bucket);

        let estimated = Self::estimate_duration(
            input,
            model_size,
            beam_size,
            chunk_ms,
            parallelism,
            input.duration_seconds,
        );

        let plan = SchedulerPlan {
            plan_id,
            model_size,
            beam_size,
            chunk_duration_ms: chunk_ms,
            max_parallelism: parallelism,
            noise_reduction_enabled: noise_reduction,
            post_processing,
            diarization_mode: diarization,
            rerun_low_confidence: rerun_low_conf,
            estimated_duration_seconds: estimated,
            explanation,
            fallback_reason: None,
            input_signals: input.clone(),
        };

        log::info!(
            "Scheduler plan {}: model={:?} beam={} chunk={}ms parallel={} noise={} post={:?} diar={:?} rerun={} est={:.0}s — {}",
            plan.plan_id,
            plan.model_size,
            plan.beam_size,
            plan.chunk_duration_ms,
            plan.max_parallelism,
            plan.noise_reduction_enabled,
            plan.post_processing,
            plan.diarization_mode,
            plan.rerun_low_confidence,
            plan.estimated_duration_seconds,
            plan.explanation,
        );

        plan
    }

    /// Create a CPU fallback plan after GPU/CUDA execution fails.
    pub fn plan_cpu_fallback(input: &SchedulerInput, reason: impl Into<String>) -> SchedulerPlan {
        let reason = reason.into();
        let mut fallback_input = input.clone();
        fallback_input.cuda_available = false;
        fallback_input.vram_gb = None;
        fallback_input.device_tier = DeviceTier::classify(false, None, input.cpu_cores);

        let mut plan = Self::plan(&fallback_input);
        plan.fallback_reason = Some(reason.clone());
        plan.explanation = format!(
            "{} Fallback reason: {}; continuing with CPU execution.",
            plan.explanation, reason
        );
        plan
    }

    /// Core decision logic based on PRD §7 strategy table.
    fn decide(
        input: &SchedulerInput,
        bucket: DurationBucket,
    ) -> (
        ModelSize, // model
        u32,       // beam
        u32,       // chunk_ms
        u32,       // parallelism
        bool,      // noise_reduction
        PostProcessingLevel,
        DiarizationMode,
        bool,   // rerun_low_confidence
        String, // explanation
    ) {
        let extreme = input.extreme_accuracy;
        let is_gpu_standard = input.device_tier == DeviceTier::GpuStandard;
        let is_noisy = input.is_high_noise || input.snr_db.map(|s| s < 20.0).unwrap_or(false);
        let multi_speaker = input.estimated_speaker_count > 1;

        match bucket {
            // ── Short audio (< 15 min) ────────────────────────────────────
            DurationBucket::Short => {
                if extreme {
                    (
                        if is_gpu_standard { ModelSize::Large } else { ModelSize::Medium },
                        10,     // high beam
                        15_000, // 15s chunks
                        2,
                        is_noisy,
                        PostProcessingLevel::Aggressive,
                        if multi_speaker { DiarizationMode::Enhanced } else { DiarizationMode::None },
                        true,   // rerun low-confidence
                        "Short audio + extreme accuracy: large model, high beam, aggressive post-processing, fine timestamps".into(),
                    )
                } else {
                    (
                        if is_gpu_standard {
                            ModelSize::Medium
                        } else {
                            ModelSize::Small
                        },
                        5,      // standard beam
                        30_000, // 30s chunks
                        2,
                        is_noisy,
                        PostProcessingLevel::Standard,
                        if multi_speaker {
                            DiarizationMode::Basic
                        } else {
                            DiarizationMode::None
                        },
                        false,
                        "Short audio: medium model, standard beam, direct high-quality output"
                            .into(),
                    )
                }
            }

            // ── Medium audio (15–60 min) ──────────────────────────────────
            DurationBucket::Medium => {
                if extreme {
                    (
                        ModelSize::Medium,
                        8,
                        20_000, // 20s chunks
                        if is_gpu_standard { 4 } else { 2 },
                        is_noisy,
                        PostProcessingLevel::Aggressive,
                        if multi_speaker { DiarizationMode::Enhanced } else { DiarizationMode::Basic },
                        true,
                        "Medium audio + extreme: medium model, higher beam, aggressive post-processing, rerun low-confidence".into(),
                    )
                } else {
                    (
                        ModelSize::Small,
                        5,
                        30_000, // 30s chunks
                        if is_gpu_standard { 4 } else { 2 },
                        is_noisy,
                        PostProcessingLevel::Standard,
                        if multi_speaker { DiarizationMode::Basic } else { DiarizationMode::None },
                        false,
                        "Medium audio: small model, standard beam, parallel inference, fast proofreadable output".into(),
                    )
                }
            }

            // ── Long audio (> 60 min) ─────────────────────────────────────
            DurationBucket::Long => {
                if extreme {
                    (
                        ModelSize::Medium,
                        8,
                        15_000, // 15s chunks (finer for long audio)
                        if is_gpu_standard { 4 } else { 2 },
                        is_noisy,
                        PostProcessingLevel::Aggressive,
                        if multi_speaker { DiarizationMode::Enhanced } else { DiarizationMode::Basic },
                        true,
                        "Long audio + extreme: medium model, fine chunks, aggressive post-processing, stream first pass then rerun low-confidence".into(),
                    )
                } else {
                    // Default for long audio: speed priority
                    (
                        ModelSize::Base,     // smaller model for speed
                        4,                   // lower beam
                        45_000,             // 45s chunks (fewer, larger)
                        if is_gpu_standard { 6 } else { 3 },
                        is_noisy,
                        PostProcessingLevel::Standard,
                        if multi_speaker { DiarizationMode::Basic } else { DiarizationMode::None },
                        false,
                        "Long audio: base model, low beam, large chunks, maximum parallelism, fast first pass for quick review".into(),
                    )
                }
            }
        }
    }

    /// Estimate total processing duration.
    /// Rough heuristic: processing_time ≈ (audio_duration / parallelism) * model_factor / chunk_factor
    fn estimate_duration(
        input: &SchedulerInput,
        model: ModelSize,
        _beam: u32,
        chunk_ms: u32,
        parallelism: u32,
        audio_duration: f64,
    ) -> f64 {
        let model_factor = match model {
            ModelSize::Tiny => 0.03,
            ModelSize::Base => 0.06,
            ModelSize::Small => 0.15,
            ModelSize::Medium => 0.4,
            ModelSize::Large => 1.0,
        };

        let device_multiplier = match input.device_tier {
            DeviceTier::GpuStandard => 1.0,
            DeviceTier::GpuEntry => 1.8,
            DeviceTier::CpuOnly => 6.0,
            DeviceTier::LowSpec => 15.0,
        };

        // Chunk overhead: smaller chunks = more overhead from merging
        let chunk_overhead = 60_000.0 / chunk_ms as f64;

        let base_seconds = audio_duration * model_factor * device_multiplier;
        let with_overhead = base_seconds * chunk_overhead;
        let parallelized = with_overhead / parallelism as f64;

        // Add cold start if model not cached
        let cold_start = input.cold_start_seconds.unwrap_or(0.0);

        // Floor at 3 seconds
        (parallelized + cold_start).max(3.0)
    }

    /// Generate a formatted decision log (without audio/text content).
    /// PRD §7.2: Scheduler logs must NOT contain audio content, transcript text,
    /// glossary entries, person names, or company names.
    pub fn decision_log(plan: &SchedulerPlan) -> String {
        serde_json::to_string_pretty(&DecisionLogEntry {
            plan_id: plan.plan_id.clone(),
            duration_bucket: format_duration_bucket(plan.input_signals.duration_seconds),
            device_tier: format!("{:?}", plan.input_signals.device_tier),
            cuda_available: plan.input_signals.cuda_available,
            vram_gb: plan.input_signals.vram_gb,
            cpu_cores: plan.input_signals.cpu_cores,
            snr_db: plan.input_signals.snr_db,
            is_high_noise: plan.input_signals.is_high_noise,
            speaker_count: plan.input_signals.estimated_speaker_count,
            extreme_accuracy: plan.input_signals.extreme_accuracy,
            model_cached: plan.input_signals.model_cached,
            model_size: format!("{:?}", plan.model_size),
            beam_size: plan.beam_size,
            chunk_duration_ms: plan.chunk_duration_ms,
            max_parallelism: plan.max_parallelism,
            noise_reduction: plan.noise_reduction_enabled,
            post_processing: format!("{:?}", plan.post_processing),
            diarization: format!("{:?}", plan.diarization_mode),
            rerun_low_confidence: plan.rerun_low_confidence,
            estimated_seconds: plan.estimated_duration_seconds,
            explanation: plan.explanation.clone(),
            fallback_reason: plan.fallback_reason.clone(),
        })
        .unwrap_or_default()
    }
}

#[derive(Serialize)]
struct DecisionLogEntry {
    plan_id: String,
    duration_bucket: String,
    device_tier: String,
    cuda_available: bool,
    vram_gb: Option<f64>,
    cpu_cores: u32,
    snr_db: Option<f64>,
    is_high_noise: bool,
    speaker_count: u32,
    extreme_accuracy: bool,
    model_cached: bool,
    model_size: String,
    beam_size: u32,
    chunk_duration_ms: u32,
    max_parallelism: u32,
    noise_reduction: bool,
    post_processing: String,
    diarization: String,
    rerun_low_confidence: bool,
    estimated_seconds: f64,
    explanation: String,
    fallback_reason: Option<String>,
}

fn format_duration_bucket(seconds: f64) -> String {
    if seconds < 900.0 {
        "short (<15min)".into()
    } else if seconds <= 3600.0 {
        "medium (15-60min)".into()
    } else {
        "long (>60min)".into()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_input(
        duration: f64,
        extreme: bool,
        device: DeviceTier,
        speakers: u32,
        noisy: bool,
    ) -> SchedulerInput {
        SchedulerInput {
            duration_seconds: duration,
            snr_db: if noisy { Some(12.0) } else { Some(30.0) },
            speech_density: Some(0.85),
            estimated_speaker_count: speakers,
            is_high_noise: noisy,
            device_tier: device,
            cuda_available: device != DeviceTier::CpuOnly,
            vram_gb: if device == DeviceTier::GpuStandard {
                Some(8.0)
            } else {
                Some(6.0)
            },
            cpu_cores: 8,
            extreme_accuracy: extreme,
            model_cached: true,
            cold_start_seconds: None,
        }
    }

    #[test]
    fn test_short_clean_default() {
        let input = make_input(300.0, false, DeviceTier::GpuStandard, 1, false);
        let plan = Scheduler::plan(&input);
        assert_eq!(plan.model_size, ModelSize::Medium);
        assert_eq!(plan.beam_size, 5);
        assert!(!plan.rerun_low_confidence);
    }

    #[test]
    fn test_short_clean_extreme() {
        let input = make_input(300.0, true, DeviceTier::GpuStandard, 1, false);
        let plan = Scheduler::plan(&input);
        assert_eq!(plan.model_size, ModelSize::Large);
        assert!(plan.beam_size >= 8);
        assert!(plan.rerun_low_confidence);
    }

    #[test]
    fn test_long_audio_default_speed_priority() {
        let input = make_input(5400.0, false, DeviceTier::GpuStandard, 1, false);
        let plan = Scheduler::plan(&input);
        assert_eq!(plan.model_size, ModelSize::Base); // speed priority
        assert!(plan.chunk_duration_ms >= 30_000);
        assert!(plan.max_parallelism >= 4);
    }

    #[test]
    fn test_long_audio_extreme() {
        let input = make_input(5400.0, true, DeviceTier::GpuStandard, 1, false);
        let plan = Scheduler::plan(&input);
        assert_eq!(plan.model_size, ModelSize::Medium);
        assert!(plan.rerun_low_confidence);
    }

    #[test]
    fn test_cpu_only_slower() {
        let input = make_input(600.0, false, DeviceTier::CpuOnly, 1, false);
        let plan = Scheduler::plan(&input);
        // CPU-only should still produce a valid plan
        assert!(plan.estimated_duration_seconds > 0.0);
        assert_eq!(plan.model_size, ModelSize::Small);
    }

    #[test]
    fn test_cpu_fallback_plan_records_reason() {
        let input = make_input(600.0, false, DeviceTier::GpuStandard, 1, false);
        let plan = Scheduler::plan_cpu_fallback(&input, "GPU OOM");

        assert_eq!(plan.input_signals.device_tier, DeviceTier::CpuOnly);
        assert!(!plan.input_signals.cuda_available);
        assert_eq!(plan.input_signals.vram_gb, None);
        assert_eq!(plan.fallback_reason.as_deref(), Some("GPU OOM"));

        let log = Scheduler::decision_log(&plan);
        assert!(log.contains("fallback_reason"));
        assert!(log.contains("GPU OOM"));
    }

    #[test]
    fn test_noisy_audio_enables_noise_reduction() {
        let input = make_input(600.0, false, DeviceTier::GpuStandard, 1, true);
        let plan = Scheduler::plan(&input);
        assert!(plan.noise_reduction_enabled);
        assert_eq!(plan.post_processing, PostProcessingLevel::Standard);
    }

    #[test]
    fn test_multi_speaker_enables_diarization() {
        let input = make_input(600.0, false, DeviceTier::GpuStandard, 2, false);
        let plan = Scheduler::plan(&input);
        assert_ne!(plan.diarization_mode, DiarizationMode::None);
    }

    #[test]
    fn test_decision_log_no_panic() {
        let input = make_input(1200.0, false, DeviceTier::GpuStandard, 1, false);
        let plan = Scheduler::plan(&input);
        let log = Scheduler::decision_log(&plan);
        assert!(log.contains("plan_id"));
        assert!(!log.contains("transcript")); // Must not contain content fields
    }

    #[test]
    fn test_device_tier_classification() {
        assert_eq!(
            DeviceTier::classify(true, Some(8.0), 8),
            DeviceTier::GpuStandard
        );
        assert_eq!(
            DeviceTier::classify(true, Some(6.5), 8),
            DeviceTier::GpuEntry
        );
        assert_eq!(DeviceTier::classify(false, None, 8), DeviceTier::CpuOnly);
    }
}
