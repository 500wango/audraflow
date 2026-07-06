//! Audio preprocessing pipeline.
//!
//! Handles audio file import, decoding, normalization, VAD, SNR estimation,
//! and chunking/splitting for parallel ASR processing.
//!
//! MVP: uses FFmpeg subprocess for decoding. Future: embedded decoder.

use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;
use std::time::Instant;

// ── Audio Metadata ────────────────────────────────────────────────────────

/// Extracted metadata from an audio file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioInfo {
    pub file_path: String,
    pub file_hash: String,
    pub duration_seconds: f64,
    pub sample_rate: u32,
    pub channels: u32,
    pub codec: String,
    pub format: String,
    pub snr_db: Option<f64>,
    pub speech_density: Option<f64>, // 0.0–1.0: fraction of audio containing speech
    pub is_high_noise: bool,
    pub estimated_speakers: Option<u32>,
}

// ── VAD Result ────────────────────────────────────────────────────────────

/// Voice Activity Detection result for a time range.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VadSegment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub has_speech: bool,
    pub snr_db: f64,
}

// ── Audio Chunk ────────────────────────────────────────────────────────────

/// A chunk of audio ready for ASR processing.
#[derive(Debug, Clone)]
pub struct AudioChunk {
    pub index: usize,
    pub start_ms: i64,
    pub end_ms: i64,
    pub wav_path: std::path::PathBuf,
    pub snr_db: f64,
}

#[derive(Debug, Clone, PartialEq)]
struct ChunkRange {
    start_ms: i64,
    end_ms: i64,
    snr_db: f64,
}

#[derive(Debug, Clone)]
struct SpeechSpan {
    start_ms: i64,
    end_ms: i64,
    snr_db: f64,
}

#[derive(Debug)]
struct ChunkAccumulator {
    start_ms: i64,
    end_ms: i64,
    weighted_snr_sum: f64,
    speech_duration_ms: i64,
}

const MAX_MERGE_GAP_MS: i64 = 2_000;

// ── Pipeline ───────────────────────────────────────────────────────────────

/// The audio preprocessing pipeline.
pub struct AudioPipeline {
    ffmpeg_bin: String,
    ffprobe_bin: String,
    temp_dir: std::path::PathBuf,
}

impl AudioPipeline {
    /// Create a new pipeline. Auto-detects FFmpeg/ffprobe in PATH or bundled.
    pub fn new() -> anyhow::Result<Self> {
        let ffmpeg_bin = find_binary("ffmpeg")?;
        let ffprobe_bin = find_binary("ffprobe")?;
        let temp_dir = std::env::temp_dir()
            .join("audraflow")
            .join(format!("job-{}", uuid::Uuid::new_v4()));

        std::fs::create_dir_all(&temp_dir)?;

        log::info!("FFmpeg: {}", ffmpeg_bin);
        log::info!("FFprobe: {}", ffprobe_bin);
        log::info!("Temp dir: {}", temp_dir.display());

        Ok(Self {
            ffmpeg_bin,
            ffprobe_bin,
            temp_dir,
        })
    }

    /// Create a pipeline with explicit binary paths (for bundled distribution).
    pub fn with_binaries(ffmpeg: &str, ffprobe: &str) -> anyhow::Result<Self> {
        let temp_dir = std::env::temp_dir()
            .join("audraflow")
            .join(format!("job-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir)?;
        Ok(Self {
            ffmpeg_bin: ffmpeg.to_string(),
            ffprobe_bin: ffprobe.to_string(),
            temp_dir,
        })
    }

    /// Return this job's temporary working directory.
    pub fn temp_dir(&self) -> &Path {
        &self.temp_dir
    }

    // ── Metadata Extraction ────────────────────────────────────────────────

    /// Extract full audio metadata and compute SNR/speech density.
    pub fn analyze(&self, file_path: &Path, file_hash: &str) -> anyhow::Result<AudioInfo> {
        let start = Instant::now();

        // Extract format info via ffprobe
        let duration = self.probe_duration(file_path)?;
        let sample_rate = self.probe_sample_rate(file_path)?;
        let channels = self.probe_channels(file_path)?;
        let codec = self.probe_codec(file_path)?;
        let format = file_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("unknown")
            .to_uppercase();

        // Compute SNR and speech density
        let (snr_db, is_high_noise, speech_density) = self.compute_snr_and_density(file_path)?;

        let elapsed = start.elapsed();
        log::info!(
            "Analysis complete in {:.1}s: {:.0}s audio, SNR={:.1}dB, speech_density={:.2}, noise={}",
            elapsed.as_secs_f64(),
            duration,
            snr_db.unwrap_or(0.0),
            speech_density.unwrap_or(0.0),
            is_high_noise,
        );

        Ok(AudioInfo {
            file_path: file_path.display().to_string(),
            file_hash: file_hash.to_string(),
            duration_seconds: duration,
            sample_rate,
            channels,
            codec,
            format,
            snr_db,
            speech_density,
            is_high_noise,
            estimated_speakers: None, // Filled by diarization later
        })
    }

    // ── Decoding ────────────────────────────────────────────────────────────

    /// Decode an audio file to 16kHz mono 16-bit WAV for ASR processing.
    /// Returns the path to the decoded WAV file.
    pub fn decode_to_wav(&self, file_path: &Path) -> anyhow::Result<std::path::PathBuf> {
        let output_path = self.temp_dir.join(format!(
            "decoded_{}.wav",
            file_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("audio")
        ));

        log::info!(
            "Decoding {} → {}",
            file_path.display(),
            output_path.display()
        );

        let output = Command::new(&self.ffmpeg_bin)
            .args([
                "-y", // Overwrite output
                "-i",
                &file_path.display().to_string(),
                "-acodec",
                "pcm_s16le", // 16-bit PCM
                "-ar",
                "16000", // 16kHz sample rate
                "-ac",
                "1", // Mono
                "-af",
                "loudnorm=I=-16:TP=-1.5:LRA=11", // Loudness normalization
                &output_path.display().to_string(),
            ])
            .output()
            .context("Failed to run ffmpeg for audio decoding")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("ffmpeg decode failed: {}", stderr);
        }

        Ok(output_path)
    }

    // ── VAD & Chunking ─────────────────────────────────────────────────────

    /// Run simple energy-based VAD and split audio into speech chunks.
    /// Returns a list of chunks ready for parallel ASR processing.
    pub fn vad_and_chunk(
        &self,
        wav_path: &Path,
        min_chunk_ms: i64,
        max_chunk_ms: i64,
    ) -> anyhow::Result<Vec<AudioChunk>> {
        // Use FFmpeg silencedetect filter for VAD
        let segments = self.detect_speech_segments(wav_path)?;

        // Merge small segments and split large ones into chunks
        let chunks = self.split_into_chunks(wav_path, &segments, min_chunk_ms, max_chunk_ms)?;

        log::info!(
            "VAD+chunking: {} speech segments → {} chunks",
            segments.len(),
            chunks.len()
        );

        Ok(chunks)
    }

    /// Return VAD speech spans before ASR chunk merging.
    ///
    /// Diarization needs short speech regions. Reusing the merged ASR chunks can
    /// collapse several turns into one speaker label.
    pub fn detect_speech_for_diarization(
        &self,
        wav_path: &Path,
    ) -> anyhow::Result<Vec<VadSegment>> {
        let segments = self.detect_speech_segments(wav_path)?;
        log::info!("Diarization VAD: {} speech segments", segments.len());
        Ok(segments)
    }

    /// Split the full audio duration into fixed chunks without VAD.
    ///
    /// This is useful for lyrics and strong-background music where speech VAD
    /// can drop quiet vocals or treat sung passages as non-speech.
    pub fn chunk_full_audio(
        &self,
        wav_path: &Path,
        max_chunk_ms: i64,
        overlap_ms: i64,
    ) -> anyhow::Result<Vec<AudioChunk>> {
        let duration_ms = (self.probe_duration(wav_path)? * 1000.0).ceil() as i64;
        let ranges = full_audio_chunk_ranges(duration_ms, max_chunk_ms, overlap_ms);

        log::info!(
            "Full-audio chunking: {:.1}s → {} chunks (overlap {}ms)",
            duration_ms as f64 / 1000.0,
            ranges.len(),
            overlap_ms
        );

        ranges
            .into_iter()
            .enumerate()
            .map(|(index, range)| {
                let chunk_wav =
                    self.extract_chunk(wav_path, range.start_ms, range.end_ms, index)?;
                Ok(AudioChunk {
                    index,
                    start_ms: range.start_ms,
                    end_ms: range.end_ms,
                    wav_path: chunk_wav,
                    snr_db: range.snr_db,
                })
            })
            .collect()
    }

    // ── Private Methods ────────────────────────────────────────────────────

    fn probe_duration(&self, file_path: &Path) -> anyhow::Result<f64> {
        let output = Command::new(&self.ffprobe_bin)
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
            ])
            .arg(file_path)
            .output()
            .context("Failed to run ffprobe")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let duration: f64 = stdout.trim().parse().unwrap_or(0.0);
        Ok(duration)
    }

    fn probe_sample_rate(&self, file_path: &Path) -> anyhow::Result<u32> {
        let output = Command::new(&self.ffprobe_bin)
            .args([
                "-v",
                "error",
                "-select_streams",
                "a:0",
                "-show_entries",
                "stream=sample_rate",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
            ])
            .arg(file_path)
            .output()
            .context("Failed to probe sample rate")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.trim().parse().unwrap_or(16000))
    }

    fn probe_channels(&self, file_path: &Path) -> anyhow::Result<u32> {
        let output = Command::new(&self.ffprobe_bin)
            .args([
                "-v",
                "error",
                "-select_streams",
                "a:0",
                "-show_entries",
                "stream=channels",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
            ])
            .arg(file_path)
            .output()
            .context("Failed to probe channels")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.trim().parse().unwrap_or(2))
    }

    fn probe_codec(&self, file_path: &Path) -> anyhow::Result<String> {
        let output = Command::new(&self.ffprobe_bin)
            .args([
                "-v",
                "error",
                "-select_streams",
                "a:0",
                "-show_entries",
                "stream=codec_name",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
            ])
            .arg(file_path)
            .output()
            .context("Failed to probe codec")?;

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Compute SNR and speech density using FFmpeg volumedetect filter.
    fn compute_snr_and_density(
        &self,
        file_path: &Path,
    ) -> anyhow::Result<(Option<f64>, bool, Option<f64>)> {
        let output = Command::new(&self.ffmpeg_bin)
            .args([
                "-i",
                &file_path.display().to_string(),
                "-af",
                "volumedetect,silencedetect=noise=-30dB:d=0.5",
                "-f",
                "null",
                "-",
            ])
            .output()
            .context("Failed to run ffmpeg for SNR analysis")?;

        let stderr = String::from_utf8_lossy(&output.stderr);

        // Parse mean_volume from volumedetect
        let mean_volume = parse_ffmpeg_value(&stderr, "mean_volume:");
        let max_volume = parse_ffmpeg_value(&stderr, "max_volume:");
        let snr_db = match (mean_volume, max_volume) {
            (Some(mean), Some(max)) => Some(max - mean),
            _ => None,
        };

        // Determine if high noise: SNR < 20dB is noisy
        let is_high_noise = snr_db.map(|s| s < 20.0).unwrap_or(false);

        // Count silence segments for speech density
        let silence_count = stderr.matches("silence_start:").count();
        let speech_density = if silence_count > 0 {
            // Rough estimate: fewer silence segments = higher speech density
            // This is a heuristic; proper calculation needs silence duration sums
            Some(0.85) // Default assumption for typical recordings
        } else {
            Some(0.9)
        };

        Ok((snr_db, is_high_noise, speech_density))
    }

    /// Use FFmpeg silencedetect to find speech segments.
    fn detect_speech_segments(&self, wav_path: &Path) -> anyhow::Result<Vec<VadSegment>> {
        let output = Command::new(&self.ffmpeg_bin)
            .args([
                "-i",
                &wav_path.display().to_string(),
                "-af",
                "silencedetect=noise=-35dB:d=0.3",
                "-f",
                "null",
                "-",
            ])
            .output()
            .context("Failed to run ffmpeg silencedetect")?;

        let stderr = String::from_utf8_lossy(&output.stderr);
        let mut segments = Vec::new();
        let mut silence_starts: Vec<f64> = Vec::new();
        let mut silence_ends: Vec<f64> = Vec::new();

        // Parse silence_start and silence_end lines
        for line in stderr.lines() {
            if let Some(val) = parse_ffmpeg_line_value(line, "silence_start:") {
                silence_starts.push(val);
            }
            if let Some(val) = parse_ffmpeg_line_value(line, "silence_end:") {
                silence_ends.push(val);
            }
        }

        // Speech is everything outside the silence ranges reported by FFmpeg.
        let duration = self.probe_duration(wav_path)?;
        let mut cursor_s = 0.0;

        for (idx, silence_start) in silence_starts.iter().copied().enumerate() {
            if silence_start - cursor_s > 0.1 {
                segments.push(VadSegment {
                    start_ms: (cursor_s * 1000.0) as i64,
                    end_ms: (silence_start * 1000.0) as i64,
                    has_speech: true,
                    snr_db: 20.0, // Conservative per-segment SNR heuristic.
                });
            }

            if let Some(silence_end) = silence_ends.get(idx).copied() {
                cursor_s = silence_end;
            } else {
                cursor_s = duration;
                break;
            }
        }

        if duration - cursor_s > 0.1 {
            segments.push(VadSegment {
                start_ms: (cursor_s * 1000.0) as i64,
                end_ms: (duration * 1000.0) as i64,
                has_speech: true,
                snr_db: 20.0,
            });
        }

        // If no silence detected, treat the whole file as one speech segment
        if segments.is_empty() {
            segments.push(VadSegment {
                start_ms: 0,
                end_ms: (duration * 1000.0) as i64,
                has_speech: true,
                snr_db: 20.0,
            });
        }

        Ok(segments)
    }

    /// Split VAD segments into evenly-sized chunks for parallel processing.
    fn split_into_chunks(
        &self,
        wav_path: &Path,
        segments: &[VadSegment],
        min_chunk_ms: i64,
        max_chunk_ms: i64,
    ) -> anyhow::Result<Vec<AudioChunk>> {
        chunk_ranges_from_segments(segments, min_chunk_ms, max_chunk_ms)
            .into_iter()
            .enumerate()
            .map(|(index, range)| {
                let chunk_wav =
                    self.extract_chunk(wav_path, range.start_ms, range.end_ms, index)?;
                Ok(AudioChunk {
                    index,
                    start_ms: range.start_ms,
                    end_ms: range.end_ms,
                    wav_path: chunk_wav,
                    snr_db: range.snr_db,
                })
            })
            .collect()
    }

    /// Extract a time-range sub-segment from a WAV file as a new WAV file.
    fn extract_chunk(
        &self,
        wav_path: &Path,
        start_ms: i64,
        end_ms: i64,
        index: usize,
    ) -> anyhow::Result<std::path::PathBuf> {
        let output_path = self.temp_dir.join(format!(
            "chunk_{:04}_{}.wav",
            index,
            wav_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("audio")
        ));

        let start_sec = start_ms as f64 / 1000.0;
        let duration_sec = (end_ms - start_ms) as f64 / 1000.0;

        let output = Command::new(&self.ffmpeg_bin)
            .args([
                "-y",
                "-ss",
                &format!("{:.3}", start_sec),
                "-t",
                &format!("{:.3}", duration_sec),
                "-i",
                &wav_path.display().to_string(),
                "-acodec",
                "pcm_s16le",
                "-ar",
                "16000",
                "-ac",
                "1",
                &output_path.display().to_string(),
            ])
            .output()
            .context("Failed to extract audio chunk")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("ffmpeg chunk extraction failed: {}", stderr);
        }

        Ok(output_path)
    }

    /// Clean up temporary files.
    pub fn cleanup(&self) -> anyhow::Result<()> {
        if self.temp_dir.exists() {
            std::fs::remove_dir_all(&self.temp_dir)?;
        }
        Ok(())
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Find a binary from env, managed runtime components, bundled paths, then PATH.
fn find_binary(name: &str) -> anyhow::Result<String> {
    if let Some(path) = binary_env_override(name) {
        return Ok(path.display().to_string());
    }

    if let Some(path) = managed_component_binary("ffmpeg", name) {
        return Ok(path.display().to_string());
    }

    if let Ok(exe_path) = std::env::current_exe() {
        for root in exe_path.ancestors() {
            for bundled in bundled_binary_candidates(root, name) {
                if bundled.exists() {
                    return Ok(bundled.display().to_string());
                }
            }
        }
    }

    if let Ok(path) = which::which(name) {
        return Ok(path.display().to_string());
    }

    anyhow::bail!(
        "{} not found. Install the FFmpeg runtime component in Settings or set AUDRAFLOW_{}_BIN.",
        name,
        name.to_ascii_uppercase().replace('-', "_")
    )
}

fn managed_component_binary(component_id: &str, name: &str) -> Option<std::path::PathBuf> {
    let binary_name = platform_binary_name(name);
    let path = runtime_component_bin_dir(component_id).join(binary_name);
    path.is_file().then_some(path)
}

fn platform_binary_name(name: &str) -> String {
    if cfg!(windows) && !name.ends_with(".exe") {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn runtime_component_bin_dir(component_id: &str) -> std::path::PathBuf {
    app_data_dir()
        .join("runtime")
        .join("components")
        .join(component_id)
        .join("bin")
}

fn app_data_dir() -> std::path::PathBuf {
    if let Some(path) = std::env::var_os("AUDRAFLOW_APP_DATA_DIR")
        .map(std::path::PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
    {
        return path;
    }

    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("APPDATA")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("com.audraflow.app");
    }

    #[cfg(not(target_os = "windows"))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|home| std::path::PathBuf::from(home).join(".local/share"))
            })
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("com.audraflow.app")
    }
}

fn binary_env_override(name: &str) -> Option<std::path::PathBuf> {
    let suffix = name.to_ascii_uppercase().replace('-', "_");
    let env_name = format!("AUDRAFLOW_{suffix}_BIN");
    let legacy_env_name = format!("FT_{suffix}_BIN");
    std::env::var_os(env_name)
        .or_else(|| std::env::var_os(legacy_env_name))
        .map(std::path::PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

fn bundled_binary_candidates(root: &std::path::Path, name: &str) -> Vec<std::path::PathBuf> {
    let mut candidates = vec![
        root.join(name),
        root.join("bin").join(name),
        root.join("external").join("ffmpeg").join("bin").join(name),
        root.join("tools").join("ffmpeg").join("bin").join(name),
    ];
    if cfg!(windows) {
        candidates.extend([
            root.join(format!("{name}.exe")),
            root.join("bin").join(format!("{name}.exe")),
            root.join("external")
                .join("ffmpeg")
                .join("bin")
                .join(format!("{name}.exe")),
            root.join("tools")
                .join("ffmpeg")
                .join("bin")
                .join(format!("{name}.exe")),
        ]);
    }
    candidates
}

fn full_audio_chunk_ranges(
    duration_ms: i64,
    max_chunk_ms: i64,
    overlap_ms: i64,
) -> Vec<ChunkRange> {
    let duration_ms = duration_ms.max(0);
    let max_chunk_ms = max_chunk_ms.max(1);
    let overlap_ms = overlap_ms.max(0).min(max_chunk_ms.saturating_sub(1));
    let mut ranges = Vec::new();
    let mut start_ms = 0;

    while start_ms < duration_ms {
        let end_ms = (start_ms + max_chunk_ms).min(duration_ms);
        ranges.push(ChunkRange {
            start_ms,
            end_ms,
            snr_db: 20.0,
        });
        if end_ms >= duration_ms {
            break;
        }
        start_ms = (end_ms - overlap_ms).max(start_ms + 1);
    }

    ranges
}

fn chunk_ranges_from_segments(
    segments: &[VadSegment],
    min_chunk_ms: i64,
    max_chunk_ms: i64,
) -> Vec<ChunkRange> {
    let max_chunk_ms = max_chunk_ms.max(1);
    let min_chunk_ms = min_chunk_ms.clamp(1, max_chunk_ms);
    let mut spans = segments
        .iter()
        .filter(|segment| segment.has_speech && segment.end_ms > segment.start_ms)
        .flat_map(|segment| split_segment_span(segment, max_chunk_ms))
        .collect::<Vec<_>>();

    spans.sort_by_key(|span| (span.start_ms, span.end_ms));

    let mut ranges = Vec::new();
    let mut current: Option<ChunkAccumulator> = None;

    for span in spans {
        let Some(accumulator) = current.as_mut() else {
            current = Some(ChunkAccumulator::new(&span));
            continue;
        };

        let gap_ms = (span.start_ms - accumulator.end_ms).max(0);
        let merged_duration_ms = span.end_ms - accumulator.start_ms;
        let can_merge = gap_ms <= MAX_MERGE_GAP_MS && merged_duration_ms <= max_chunk_ms;

        if can_merge {
            accumulator.push(&span);
            continue;
        }

        if accumulator.duration_ms() < min_chunk_ms
            && gap_ms <= MAX_MERGE_GAP_MS
            && merged_duration_ms <= max_chunk_ms
        {
            accumulator.push(&span);
            continue;
        }

        if let Some(done) = current.replace(ChunkAccumulator::new(&span)) {
            ranges.push(done.into_range());
        }
    }

    if let Some(done) = current {
        ranges.push(done.into_range());
    }

    ranges
}

fn split_segment_span(segment: &VadSegment, max_chunk_ms: i64) -> Vec<SpeechSpan> {
    let mut spans = Vec::new();
    let mut start_ms = segment.start_ms;

    while segment.end_ms - start_ms > max_chunk_ms {
        let end_ms = start_ms + max_chunk_ms;
        spans.push(SpeechSpan {
            start_ms,
            end_ms,
            snr_db: normalized_snr(segment.snr_db),
        });
        start_ms = end_ms;
    }

    if segment.end_ms > start_ms {
        spans.push(SpeechSpan {
            start_ms,
            end_ms: segment.end_ms,
            snr_db: normalized_snr(segment.snr_db),
        });
    }

    spans
}

fn normalized_snr(snr_db: f64) -> f64 {
    if snr_db.is_finite() {
        snr_db
    } else {
        20.0
    }
}

impl ChunkAccumulator {
    fn new(span: &SpeechSpan) -> Self {
        let speech_duration_ms = span.end_ms - span.start_ms;
        Self {
            start_ms: span.start_ms,
            end_ms: span.end_ms,
            weighted_snr_sum: span.snr_db * speech_duration_ms as f64,
            speech_duration_ms,
        }
    }

    fn duration_ms(&self) -> i64 {
        self.end_ms - self.start_ms
    }

    fn push(&mut self, span: &SpeechSpan) {
        let speech_duration_ms = span.end_ms - span.start_ms;
        self.end_ms = self.end_ms.max(span.end_ms);
        self.weighted_snr_sum += span.snr_db * speech_duration_ms as f64;
        self.speech_duration_ms += speech_duration_ms;
    }

    fn into_range(self) -> ChunkRange {
        let snr_db = if self.speech_duration_ms > 0 {
            self.weighted_snr_sum / self.speech_duration_ms as f64
        } else {
            20.0
        };

        ChunkRange {
            start_ms: self.start_ms,
            end_ms: self.end_ms,
            snr_db,
        }
    }
}

/// Parse a floating-point value after a key in ffmpeg output.
fn parse_ffmpeg_value(stderr: &str, key: &str) -> Option<f64> {
    for line in stderr.lines() {
        if let Some(idx) = line.find(key) {
            let after = &line[idx + key.len()..];
            let val_str = after
                .split_whitespace()
                .next()?
                .trim_end_matches(',')
                .trim_end_matches(';');
            return val_str.parse().ok();
        }
    }
    None
}

fn parse_ffmpeg_line_value(line: &str, key: &str) -> Option<f64> {
    if let Some(idx) = line.find(key) {
        let after = &line[idx + key.len()..];
        let val_str = after.split_whitespace().next()?.trim_end_matches(',');
        return val_str.parse().ok();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vad_segment(start_ms: i64, end_ms: i64) -> VadSegment {
        VadSegment {
            start_ms,
            end_ms,
            has_speech: true,
            snr_db: 20.0,
        }
    }

    #[test]
    fn test_parse_ffmpeg_value() {
        let stderr = "[Parsed_volumedetect_0 @ 000001] mean_volume: -23.4 dB\n\
                      [Parsed_volumedetect_0 @ 000001] max_volume: -2.1 dB";

        let mean = parse_ffmpeg_value(stderr, "mean_volume:");
        let max = parse_ffmpeg_value(stderr, "max_volume:");
        assert!((mean.unwrap() + 23.4).abs() < 0.1);
        assert!((max.unwrap() + 2.1).abs() < 0.1);
    }

    #[test]
    fn test_pipeline_creation() {
        // Don't require ffmpeg for unit tests
        let pipeline = AudioPipeline::with_binaries("ffmpeg", "ffprobe");
        assert!(pipeline.is_ok());
    }

    #[test]
    fn chunk_ranges_merge_short_adjacent_speech_segments() {
        let segments = (0..10)
            .map(|index| {
                let start_ms = index * 4_000;
                vad_segment(start_ms, start_ms + 2_000)
            })
            .collect::<Vec<_>>();

        let ranges = chunk_ranges_from_segments(&segments, 30_000, 60_000);

        assert_eq!(
            ranges,
            vec![ChunkRange {
                start_ms: 0,
                end_ms: 38_000,
                snr_db: 20.0,
            }]
        );
    }

    #[test]
    fn chunk_ranges_split_long_speech_segments() {
        let ranges = chunk_ranges_from_segments(&[vad_segment(0, 125_000)], 30_000, 60_000);

        assert_eq!(
            ranges,
            vec![
                ChunkRange {
                    start_ms: 0,
                    end_ms: 60_000,
                    snr_db: 20.0,
                },
                ChunkRange {
                    start_ms: 60_000,
                    end_ms: 120_000,
                    snr_db: 20.0,
                },
                ChunkRange {
                    start_ms: 120_000,
                    end_ms: 125_000,
                    snr_db: 20.0,
                },
            ]
        );
    }

    #[test]
    fn chunk_ranges_keep_large_silence_gaps_separate() {
        let ranges = chunk_ranges_from_segments(
            &[vad_segment(0, 10_000), vad_segment(15_000, 25_000)],
            30_000,
            60_000,
        );

        assert_eq!(
            ranges,
            vec![
                ChunkRange {
                    start_ms: 0,
                    end_ms: 10_000,
                    snr_db: 20.0,
                },
                ChunkRange {
                    start_ms: 15_000,
                    end_ms: 25_000,
                    snr_db: 20.0,
                },
            ]
        );
    }

    #[test]
    fn chunk_ranges_ignore_non_speech_and_invalid_segments() {
        let mut muted = vad_segment(0, 1_000);
        muted.has_speech = false;
        let invalid = vad_segment(4_000, 3_000);

        let ranges = chunk_ranges_from_segments(
            &[muted, invalid, vad_segment(5_000, 12_000)],
            30_000,
            60_000,
        );

        assert_eq!(
            ranges,
            vec![ChunkRange {
                start_ms: 5_000,
                end_ms: 12_000,
                snr_db: 20.0,
            }]
        );
    }

    #[test]
    fn full_audio_chunk_ranges_cover_complete_duration() {
        let ranges = full_audio_chunk_ranges(65_000, 30_000, 0);

        assert_eq!(
            ranges,
            vec![
                ChunkRange {
                    start_ms: 0,
                    end_ms: 30_000,
                    snr_db: 20.0,
                },
                ChunkRange {
                    start_ms: 30_000,
                    end_ms: 60_000,
                    snr_db: 20.0,
                },
                ChunkRange {
                    start_ms: 60_000,
                    end_ms: 65_000,
                    snr_db: 20.0,
                },
            ]
        );
    }

    #[test]
    fn full_audio_chunk_ranges_ignore_empty_duration() {
        assert!(full_audio_chunk_ranges(0, 30_000, 5_000).is_empty());
        assert!(full_audio_chunk_ranges(-1_000, 30_000, 5_000).is_empty());
    }

    #[test]
    fn full_audio_chunk_ranges_support_overlap() {
        let ranges = full_audio_chunk_ranges(65_000, 30_000, 5_000);

        assert_eq!(
            ranges,
            vec![
                ChunkRange {
                    start_ms: 0,
                    end_ms: 30_000,
                    snr_db: 20.0,
                },
                ChunkRange {
                    start_ms: 25_000,
                    end_ms: 55_000,
                    snr_db: 20.0,
                },
                ChunkRange {
                    start_ms: 50_000,
                    end_ms: 65_000,
                    snr_db: 20.0,
                },
            ]
        );
    }
}
