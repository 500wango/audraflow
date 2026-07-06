//! Benchmark module for AudraFlow ASR Runtime.
//!
//! Measures: RTF, TTFV, CER/WER, memory peak.
//! PRD §14: Benchmark results must include device, model, audio type, and whether post-processing was enabled.
//! PRD §14.1: No absolute "fastest"/"most accurate" claims without benchmark backing.

use serde::{Deserialize, Serialize};
use std::path::Path;

// ── Benchmark Configuration ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkConfig {
    /// Path to the test audio file or directory.
    pub audio_path: String,
    /// Path to the whisper.cpp GGML model.
    pub model_path: String,
    /// Reference transcript for CER/WER calculation (optional).
    pub reference_transcript: Option<String>,
    /// Whether to run with extreme accuracy mode.
    pub extreme_accuracy: bool,
    /// Number of runs for statistical averaging.
    pub runs: u32,
    /// Language code (zh, en, mix).
    pub language: String,
}

// ── Benchmark Result ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkResult {
    // ── Device info (required by PRD §14) ──
    pub device_config: DeviceConfig,

    // ── Model info ──
    pub model_name: String,
    pub model_version: String,

    // ── Audio info ──
    pub audio_duration_seconds: f64,
    pub audio_type: String, // CN-Clean, CN-Meeting, CN-EN-Mix, Noise, LongAudio
    pub language: String,

    // ── Performance metrics ──
    pub rtf_mean: f64,
    pub rtf_min: f64,
    pub rtf_max: f64,
    pub ttfv_mean_seconds: f64,
    pub elapsed_mean_seconds: f64,

    // ── Accuracy metrics (if reference available) ──
    pub cer: Option<f64>,
    pub wer: Option<f64>,

    // ── Resource metrics ──
    pub peak_memory_mb: Option<f64>,

    // ── Configuration ──
    pub extreme_accuracy: bool,
    pub post_processing_enabled: bool,
    pub chunk_count: u32,
    pub runs_completed: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct Alpha1BenchmarkReport {
    pub adaptive_scheduler: BenchmarkResult,
    pub fixed_parameters: BenchmarkResult,
    pub strategy_comparison: StrategyComparison,
    pub term_correction_metrics: TermCorrectionMetrics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct StrategyComparison {
    pub adaptive_rtf_mean: f64,
    pub fixed_rtf_mean: f64,
    /// Positive means adaptive scheduling is faster than fixed parameters.
    pub rtf_improvement_ratio: f64,
    pub adaptive_ttfv_seconds: f64,
    pub fixed_ttfv_seconds: f64,
    pub ttfv_delta_seconds: f64,
    /// Positive means adaptive CER is higher than fixed CER.
    pub cer_delta: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct TermCorrectionMetrics {
    pub expected_terms: u32,
    pub matched_terms: u32,
    pub total_replacements: u32,
    pub false_replacements: u32,
    pub term_hit_rate: f64,
    pub false_replacement_rate: f64,
}

#[allow(dead_code)]
impl TermCorrectionMetrics {
    pub fn from_counts(
        expected_terms: u32,
        matched_terms: u32,
        total_replacements: u32,
        false_replacements: u32,
    ) -> Self {
        Self {
            expected_terms,
            matched_terms,
            total_replacements,
            false_replacements,
            term_hit_rate: ratio(matched_terms, expected_terms),
            false_replacement_rate: ratio(false_replacements, total_replacements),
        }
    }
}

#[allow(dead_code)]
pub fn build_alpha1_benchmark_report(
    adaptive_scheduler: BenchmarkResult,
    fixed_parameters: BenchmarkResult,
    term_correction_metrics: TermCorrectionMetrics,
) -> Alpha1BenchmarkReport {
    let strategy_comparison = compare_scheduler_strategies(&adaptive_scheduler, &fixed_parameters);

    Alpha1BenchmarkReport {
        adaptive_scheduler,
        fixed_parameters,
        strategy_comparison,
        term_correction_metrics,
    }
}

#[allow(dead_code)]
pub fn compare_scheduler_strategies(
    adaptive_scheduler: &BenchmarkResult,
    fixed_parameters: &BenchmarkResult,
) -> StrategyComparison {
    StrategyComparison {
        adaptive_rtf_mean: adaptive_scheduler.rtf_mean,
        fixed_rtf_mean: fixed_parameters.rtf_mean,
        rtf_improvement_ratio: if fixed_parameters.rtf_mean > 0.0 {
            (fixed_parameters.rtf_mean - adaptive_scheduler.rtf_mean) / fixed_parameters.rtf_mean
        } else {
            0.0
        },
        adaptive_ttfv_seconds: adaptive_scheduler.ttfv_mean_seconds,
        fixed_ttfv_seconds: fixed_parameters.ttfv_mean_seconds,
        ttfv_delta_seconds: adaptive_scheduler.ttfv_mean_seconds
            - fixed_parameters.ttfv_mean_seconds,
        cer_delta: match (adaptive_scheduler.cer, fixed_parameters.cer) {
            (Some(adaptive), Some(fixed)) => Some(adaptive - fixed),
            _ => None,
        },
    }
}

#[allow(dead_code)]
fn ratio(numerator: u32, denominator: u32) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

// ── Device Configuration ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfig {
    pub os: String,
    pub cpu_model: String,
    pub cpu_cores: u32,
    pub ram_gb: f64,
    pub gpu_model: Option<String>,
    pub vram_gb: Option<f64>,
    pub cuda_version: Option<String>,
    pub driver_version: Option<String>,
}

impl DeviceConfig {
    /// Collect device configuration from the current machine.
    pub fn detect() -> Self {
        Self {
            os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
            cpu_model: detect_cpu_model(),
            cpu_cores: num_cpus::get() as u32,
            ram_gb: detect_ram_gb(),
            gpu_model: detect_gpu_model(),
            vram_gb: detect_vram_gb(),
            cuda_version: detect_cuda_version(),
            driver_version: detect_driver_version(),
        }
    }
}

// ── Benchmark Runner ────────────────────────────────────────────────────────

/// Run a benchmark and collect results.
#[allow(dead_code)]
pub fn run_benchmark(config: &BenchmarkConfig) -> anyhow::Result<BenchmarkResult> {
    let device = DeviceConfig::detect();
    log::info!("Benchmark device: {:?}", device);

    // ── Input validation ───────────────────────────────────────────────────
    let audio_path = Path::new(&config.audio_path);
    if !audio_path.exists() {
        anyhow::bail!("Audio file not found: {}", config.audio_path);
    }

    let model_path = Path::new(&config.model_path);
    if !model_path.exists() {
        anyhow::bail!("Model file not found: {}", config.model_path);
    }
    if config.runs == 0 {
        anyhow::bail!("Benchmark runs must be greater than zero");
    }

    // ── Run N iterations ───────────────────────────────────────────────────
    let mut rtfs = Vec::with_capacity(config.runs as usize);
    let mut ttfvs = Vec::with_capacity(config.runs as usize);
    let mut elapsed_times = Vec::with_capacity(config.runs as usize);

    let mut audio_duration = 0.0;
    let mut chunk_count_sum = 0_u32;
    let mut transcript_chars_sum = 0_usize;

    for run in 0..config.runs {
        log::info!("Benchmark run {}/{}...", run + 1, config.runs);

        let pipeline = crate::audio_pipeline::AudioPipeline::new()?;
        let engine =
            crate::whisper_engine::WhisperEngine::new(&crate::whisper_engine::detect_device())?
                .with_model(model_path.to_path_buf())
                .with_language(config.language.clone());
        let result = crate::transcribe_file_pipeline_sync(
            &engine,
            &pipeline,
            audio_path,
            &format!("benchmark-run-{}", run + 1),
            config.extreme_accuracy,
            crate::AudioMode::Speech,
            crate::VocalSeparationMode::Off,
        )?;

        if audio_duration == 0.0 {
            audio_duration = result.audio_info.duration_seconds;
        }
        chunk_count_sum += result.chunk_count;
        transcript_chars_sum += result
            .segments
            .iter()
            .map(|segment| segment.text.chars().count())
            .sum::<usize>();

        rtfs.push(result.rtf);
        ttfvs.push(result.ttfv_s);
        elapsed_times.push(result.rtf * result.audio_info.duration_seconds);

        log::info!(
            "  Run {}: RTF={:.4}, TTFV={:.1}s, elapsed={:.1}s, chunks={}, transcript_chars={}",
            run + 1,
            result.rtf,
            result.ttfv_s,
            result.rtf * result.audio_info.duration_seconds,
            result.chunk_count,
            result
                .segments
                .iter()
                .map(|segment| segment.text.chars().count())
                .sum::<usize>(),
        );
    }

    // ── Compute statistics ─────────────────────────────────────────────────
    let rtf_mean = rtfs.iter().sum::<f64>() / rtfs.len() as f64;
    let rtf_min = rtfs.iter().cloned().fold(f64::MAX, f64::min);
    let rtf_max = rtfs.iter().cloned().fold(f64::MIN, f64::max);
    let ttfv_mean = ttfvs.iter().sum::<f64>() / ttfvs.len() as f64;
    let elapsed_mean = elapsed_times.iter().sum::<f64>() / elapsed_times.len() as f64;
    let chunk_count = average_count_rounded(chunk_count_sum, config.runs);
    log::info!(
        "Benchmark transcript chars across runs: {}",
        transcript_chars_sum
    );

    Ok(BenchmarkResult {
        device_config: device,
        model_name: model_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string(),
        model_version: model_version_from_path(model_path),
        audio_duration_seconds: audio_duration,
        audio_type: audio_type_from_config(config, audio_duration),
        language: config.language.clone(),
        rtf_mean,
        rtf_min,
        rtf_max,
        ttfv_mean_seconds: ttfv_mean,
        elapsed_mean_seconds: elapsed_mean,
        cer: None, // Requires reference transcript
        wer: None,
        peak_memory_mb: None, // Requires OS-level memory tracking
        extreme_accuracy: config.extreme_accuracy,
        post_processing_enabled: false,
        chunk_count,
        runs_completed: config.runs,
    })
}

fn average_count_rounded(total: u32, runs: u32) -> u32 {
    if runs == 0 {
        0
    } else {
        ((total as f64) / (runs as f64)).round() as u32
    }
}

fn model_version_from_path(model_path: &Path) -> String {
    model_path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .and_then(|name| {
            name.rsplit_once("-v")
                .map(|(_, version)| version.to_string())
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn audio_type_from_config(config: &BenchmarkConfig, duration_seconds: f64) -> String {
    if duration_seconds >= 3600.0 {
        return "LongAudio".into();
    }

    let path_hint = config.audio_path.to_ascii_lowercase();
    if path_hint.contains("noise") || path_hint.contains("noisy") {
        return "Noise".into();
    }
    if path_hint.contains("meeting") {
        return "CN-Meeting".into();
    }
    if path_hint.contains("mix") || config.language.eq_ignore_ascii_case("mix") {
        return "CN-EN-Mix".into();
    }
    if config.language.eq_ignore_ascii_case("zh") || config.language.eq_ignore_ascii_case("cn") {
        "CN-Clean".into()
    } else {
        config.language.clone()
    }
}

/// Print a benchmark report in the format required by PRD §14.
#[allow(dead_code)]
pub fn print_report(result: &BenchmarkResult) {
    println!("═══════════════════════════════════════════════════");
    println!("  AudraFlow Benchmark Report");
    println!("═══════════════════════════════════════════════════");
    println!();
    println!("Device:");
    println!("  OS:       {}", result.device_config.os);
    println!(
        "  CPU:      {} ({} cores)",
        result.device_config.cpu_model, result.device_config.cpu_cores
    );
    println!("  RAM:      {:.1} GB", result.device_config.ram_gb);
    if let Some(ref gpu) = result.device_config.gpu_model {
        println!("  GPU:      {}", gpu);
    }
    if let Some(vram) = result.device_config.vram_gb {
        println!("  VRAM:     {:.1} GB", vram);
    }
    if let Some(ref cuda) = result.device_config.cuda_version {
        println!("  CUDA:     {}", cuda);
    }
    println!();
    println!("Model:");
    println!("  Name:     {}", result.model_name);
    println!("  Version:  {}", result.model_version);
    println!();
    println!("Audio:");
    println!("  Duration: {:.0} s", result.audio_duration_seconds);
    println!("  Type:     {}", result.audio_type);
    println!("  Language: {}", result.language);
    println!();
    println!("Performance:");
    println!(
        "  RTF mean: {:.4}  ({:.1}s per audio-hour)",
        result.rtf_mean,
        result.rtf_mean * 3600.0
    );
    println!("  RTF min:  {:.4}", result.rtf_min);
    println!("  RTF max:  {:.4}", result.rtf_max);
    println!("  TTFV:     {:.1} s", result.ttfv_mean_seconds);
    println!("  Elapsed:  {:.1} s", result.elapsed_mean_seconds);
    println!();
    if let Some(cer) = result.cer {
        println!("Accuracy:");
        println!("  CER:      {:.4} ({:.1}%)", cer, cer * 100.0);
    }
    if let Some(wer) = result.wer {
        println!("  WER:      {:.4} ({:.1}%)", wer, wer * 100.0);
    }
    println!();
    println!("Configuration:");
    println!("  Extreme accuracy: {}", result.extreme_accuracy);
    println!("  Post-processing:  {}", result.post_processing_enabled);
    println!("  Chunks:           {}", result.chunk_count);
    println!("  Runs:             {}", result.runs_completed);
    println!();
    println!("═══════════════════════════════════════════════════");
}

/// Save benchmark result as JSON for CI/historical tracking.
#[allow(dead_code)]
pub fn save_json(result: &BenchmarkResult, output_path: &Path) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(result)?;
    std::fs::write(output_path, json)?;
    log::info!("Benchmark saved to: {}", output_path.display());
    Ok(())
}

// ── Hardware Detection Helpers ─────────────────────────────────────────────

fn detect_cpu_model() -> String {
    #[cfg(target_os = "windows")]
    {
        std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "unknown".to_string())
    }
    #[cfg(not(target_os = "windows"))]
    {
        "unknown".to_string()
    }
}

fn detect_ram_gb() -> f64 {
    #[cfg(target_os = "linux")]
    {
        if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
            if let Some(kb) = meminfo
                .lines()
                .find_map(|line| line.strip_prefix("MemTotal:"))
                .and_then(|line| line.split_whitespace().next())
                .and_then(|value| value.parse::<f64>().ok())
            {
                return kb / 1024.0 / 1024.0;
            }
        }
    }

    16.0
}

fn detect_gpu_model() -> Option<String> {
    let output = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=name", "--format=csv,noheader"])
        .output()
        .ok()?;

    if output.status.success() {
        let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }
    None
}

fn detect_vram_gb() -> Option<f64> {
    let output = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=memory.total", "--format=csv,noheader,nounits"])
        .output()
        .ok()?;

    if output.status.success() {
        let mb: f64 = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .ok()?;
        return Some(mb / 1024.0);
    }
    None
}

fn detect_cuda_version() -> Option<String> {
    let output = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=cuda_version", "--format=csv,noheader"])
        .output()
        .ok()?;

    if output.status.success() {
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !version.is_empty() {
            return Some(version);
        }
    }
    None
}

fn detect_driver_version() -> Option<String> {
    let output = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=driver_version", "--format=csv,noheader"])
        .output()
        .ok()?;

    if output.status.success() {
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !version.is_empty() {
            return Some(version);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_result(rtf_mean: f64, ttfv: f64, cer: Option<f64>) -> BenchmarkResult {
        BenchmarkResult {
            device_config: DeviceConfig::detect(),
            model_name: "test-model".into(),
            model_version: "1.0".into(),
            audio_duration_seconds: 60.0,
            audio_type: "CN-Clean".into(),
            language: "zh".into(),
            rtf_mean,
            rtf_min: rtf_mean * 0.9,
            rtf_max: rtf_mean * 1.1,
            ttfv_mean_seconds: ttfv,
            elapsed_mean_seconds: rtf_mean * 60.0,
            cer,
            wer: None,
            peak_memory_mb: Some(2048.0),
            extreme_accuracy: false,
            post_processing_enabled: false,
            chunk_count: 4,
            runs_completed: 3,
        }
    }

    #[test]
    fn test_device_detection() {
        let device = DeviceConfig::detect();
        assert!(!device.os.is_empty());
        assert!(device.cpu_cores > 0);
    }

    #[test]
    fn test_alpha1_strategy_comparison() {
        let adaptive = sample_result(0.08, 3.0, Some(0.045));
        let fixed = sample_result(0.10, 5.0, Some(0.050));
        let comparison = compare_scheduler_strategies(&adaptive, &fixed);

        assert!((comparison.rtf_improvement_ratio - 0.2).abs() < 0.001);
        assert_eq!(comparison.ttfv_delta_seconds, -2.0);
        assert!((comparison.cer_delta.unwrap() + 0.005).abs() < 0.001);
    }

    #[test]
    fn test_alpha1_term_metrics() {
        let metrics = TermCorrectionMetrics::from_counts(20, 18, 19, 1);

        assert_eq!(metrics.expected_terms, 20);
        assert!((metrics.term_hit_rate - 0.9).abs() < 0.001);
        assert!((metrics.false_replacement_rate - (1.0 / 19.0)).abs() < 0.001);
    }

    #[test]
    fn test_alpha1_report_contains_strategy_and_terms() {
        let adaptive = sample_result(0.08, 3.0, Some(0.045));
        let fixed = sample_result(0.10, 5.0, Some(0.050));
        let metrics = TermCorrectionMetrics::from_counts(20, 18, 19, 1);

        let report = build_alpha1_benchmark_report(adaptive, fixed, metrics);

        assert_eq!(report.adaptive_scheduler.rtf_mean, 0.08);
        assert_eq!(report.fixed_parameters.rtf_mean, 0.10);
        assert_eq!(report.term_correction_metrics.matched_terms, 18);
        assert!(report.strategy_comparison.rtf_improvement_ratio > 0.0);
    }

    #[test]
    fn test_report_does_not_panic() {
        let result = sample_result(0.1, 2.5, Some(0.05));
        print_report(&result);
    }
}
