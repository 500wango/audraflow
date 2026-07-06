//! Diarization Worker — Speaker Separation
//!
//! PRD §6.3: MVP diarization uses VAD + voice embedding + clustering.
//! Outputs editable Speaker A/B/C labels; marks overlapping speech as risk.
//!
//! Lightweight approach for MVP:
//! - Uses existing VAD segments from audio_pipeline
//! - Extracts simple acoustic features per segment
//! - Agglomerative clustering with cosine distance
//! - Assigns temporary speaker labels (A, B, C, ...)
//!
//! Does NOT: identify real identities, track across projects.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use crate::audio_pipeline::VadSegment;

// ── Diarization Input ──────────────────────────────────────────────────────

/// Input to the diarization pipeline.
#[derive(Debug, Clone)]
pub struct DiarizationInput {
    /// VAD speech segments.
    pub speech_segments: Vec<VadSegment>,
    /// Path to the decoded 16kHz mono WAV file.
    pub wav_path: std::path::PathBuf,
    /// Expected maximum number of speakers (0 = auto-detect).
    pub max_speakers: u32,
}

// ── Diarization Output ─────────────────────────────────────────────────────

/// Output of the diarization pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiarizationOutput {
    /// Estimated speaker count.
    pub speaker_count_estimate: u32,
    /// Per-segment speaker assignments.
    pub speaker_segments: Vec<SpeakerSegment>,
    /// Overall clustering quality (0.0–1.0).
    pub clustering_quality: f64,
}

/// A single segment with speaker assignment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeakerSegment {
    pub segment_id: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub speaker_id: String,
    pub confidence: f64,
    pub is_overlap: bool,
}

// ── Acoustic Features ──────────────────────────────────────────────────────

/// Simplified acoustic features for a speech segment.
/// Production systems use x-vectors or d-vectors from neural networks.
#[derive(Debug, Clone)]
struct SegmentFeatures {
    /// Number of frames in this segment.
    frame_count: usize,
    /// Mean energy (RMS)
    mean_energy: f64,
    /// Energy variance
    energy_variance: f64,
    /// Mean zero-crossing rate
    mean_zcr: f64,
    /// Mean first-difference energy. Useful as a cheap spectral-shape proxy.
    mean_delta_energy: f64,
    /// Peak absolute amplitude.
    peak_energy: f64,
    /// Duration in ms
    duration_ms: f64,
    /// SNR estimate for this segment
    snr_db: f64,
}

impl SegmentFeatures {
    /// Compute a 6-dimensional feature vector for clustering.
    fn to_vector(&self) -> Vec<f64> {
        vec![
            self.mean_energy.clamp(0.0, 1.0) * 1.5,
            self.energy_variance.sqrt().clamp(0.0, 1.0) * 1.2,
            (self.mean_zcr * 8.0).clamp(0.0, 1.0) * 2.2,
            self.mean_delta_energy.clamp(0.0, 1.0) * 6.0,
            self.peak_energy.clamp(0.0, 1.0) * 0.8,
            ((self.snr_db + 10.0) / 60.0).clamp(0.0, 1.0) * 0.4,
        ]
    }

    /// Weighted Euclidean distance between feature vectors.
    fn feature_distance(a: &[f64], b: &[f64]) -> f64 {
        if a.is_empty() || b.is_empty() {
            return 1.0;
        }

        let dim = a.len().min(b.len());
        let sum: f64 = a
            .iter()
            .zip(b)
            .take(dim)
            .map(|(x, y)| {
                let delta = x - y;
                delta * delta
            })
            .sum();

        (sum / dim as f64).sqrt()
    }
}

#[derive(Debug, Clone)]
struct PcmAudio {
    sample_rate: u32,
    samples: Vec<f32>,
}

// ── Diarization Worker ─────────────────────────────────────────────────────

/// The diarization engine.
pub struct DiarizationWorker {
    /// Minimum segment duration to consider for clustering (ms).
    min_segment_ms: i64,
    /// Distance threshold for merging clusters (0.0–1.0, lower = more aggressive merge).
    merge_threshold: f64,
}

impl DiarizationWorker {
    pub fn new() -> Self {
        Self {
            min_segment_ms: 500,   // Ignore segments shorter than 0.5s
            merge_threshold: 0.22, // Feature distance below this → same speaker
        }
    }

    pub fn with_threshold(mut self, threshold: f64) -> Self {
        self.merge_threshold = threshold;
        self
    }

    /// Run the full diarization pipeline.
    ///
    /// Steps:
    /// 1. Filter segments too short for reliable features
    /// 2. Extract acoustic features for each segment
    /// 3. Cluster segments by feature similarity
    /// 4. Detect overlapping speech
    /// 5. Assign speaker labels
    pub fn run(&self, input: &DiarizationInput) -> DiarizationOutput {
        // ── Step 1: Filter ─────────────────────────────────────────────────
        let valid: Vec<&VadSegment> = input
            .speech_segments
            .iter()
            .filter(|s| s.has_speech && (s.end_ms - s.start_ms) >= self.min_segment_ms)
            .collect();

        if valid.is_empty() {
            return DiarizationOutput {
                speaker_count_estimate: 1,
                speaker_segments: input
                    .speech_segments
                    .iter()
                    .map(|s| SpeakerSegment {
                        segment_id: format!("spk_{}_{}", s.start_ms, s.end_ms),
                        start_ms: s.start_ms,
                        end_ms: s.end_ms,
                        speaker_id: "Speaker A".to_string(),
                        confidence: 0.5,
                        is_overlap: false,
                    })
                    .collect(),
                clustering_quality: 0.0,
            };
        }

        // ── Step 2: Extract features ───────────────────────────────────────
        let audio = match read_wav_pcm(&input.wav_path) {
            Ok(audio) => Some(audio),
            Err(error) => {
                log::warn!(
                    "Diarization could not read WAV features from {}: {error}; using metadata fallback",
                    input.wav_path.display()
                );
                None
            }
        };
        let features: Vec<(usize, SegmentFeatures)> = valid
            .iter()
            .enumerate()
            .map(|(i, seg)| {
                let features = extract_segment_features(seg, audio.as_ref());
                (i, features)
            })
            .collect();

        // ── Step 3: Agglomerative clustering ───────────────────────────────
        let feature_vectors: Vec<Vec<f64>> = features.iter().map(|(_, f)| f.to_vector()).collect();
        let mut cluster_labels = agglomerative_cluster(&feature_vectors, self.merge_threshold);
        if input.max_speakers > 0 {
            cluster_labels = cap_cluster_count(
                &feature_vectors,
                &cluster_labels,
                input.max_speakers as usize,
            );
        }

        // ── Step 4: Determine speaker count and assign labels ──────────────
        let unique_clusters: Vec<usize> = {
            let mut set: Vec<usize> = cluster_labels.to_vec();
            set.sort_unstable();
            set.dedup();
            set
        };

        let speaker_count = unique_clusters.len().max(1) as u32;

        // Map cluster IDs to speaker labels (A, B, C, ...)
        let label_map: HashMap<usize, String> = unique_clusters
            .iter()
            .take(speaker_count as usize)
            .enumerate()
            .map(|(i, &cluster)| {
                let label = format!("Speaker {}", (b'A' + i as u8) as char);
                (cluster, label)
            })
            .collect();

        // ── Step 5: Assign speaker labels to ALL segments ───────────────────
        let mut speaker_segments = Vec::new();
        let mut overlap_pairs = Vec::new();

        // Detect overlaps among all segments
        for i in 0..input.speech_segments.len() {
            let seg = &input.speech_segments[i];
            for j in (i + 1)..input.speech_segments.len() {
                let other = &input.speech_segments[j];
                if seg.start_ms < other.end_ms
                    && other.start_ms < seg.end_ms
                    && seg.has_speech
                    && other.has_speech
                {
                    overlap_pairs.push((i, j));
                }
            }
        }

        for i in 0..input.speech_segments.len() {
            let seg = &input.speech_segments[i];
            let dur_ms = seg.end_ms - seg.start_ms;

            // For short segments: use nearest neighbor speaker or default
            let (speaker, confidence) = if dur_ms < self.min_segment_ms {
                // Find the nearest valid segment's speaker
                let nearest = valid
                    .iter()
                    .min_by_key(|v| {
                        if v.start_ms > seg.end_ms {
                            v.start_ms - seg.end_ms
                        } else if seg.start_ms > v.end_ms {
                            seg.start_ms - v.end_ms
                        } else {
                            0
                        }
                    })
                    .and_then(|v| {
                        let _v_idx = input
                            .speech_segments
                            .iter()
                            .position(|s| s.start_ms == v.start_ms && s.end_ms == v.end_ms)?;
                        let cluster =
                            cluster_labels.get(valid.iter().position(|x| {
                                x.start_ms == v.start_ms && x.end_ms == v.end_ms
                            })?)?;
                        Some((
                            label_map
                                .get(cluster)
                                .cloned()
                                .unwrap_or_else(|| "Speaker A".to_string()),
                            0.55,
                        ))
                    })
                    .unwrap_or_else(|| ("Speaker A".to_string(), 0.5));

                (nearest.0, nearest.1)
            } else {
                // Find this segment in the valid list
                let valid_idx = valid
                    .iter()
                    .position(|v| v.start_ms == seg.start_ms && v.end_ms == seg.end_ms);
                if let Some(v_idx) = valid_idx {
                    let cluster = cluster_labels.get(v_idx).copied().unwrap_or(0);
                    let spk = label_map
                        .get(&cluster)
                        .cloned()
                        .unwrap_or_else(|| "Speaker A".to_string());
                    let conf = (0.5 + (dur_ms as f64 / self.min_segment_ms as f64) * 0.3).min(0.95);
                    (spk, conf)
                } else {
                    ("Speaker A".to_string(), 0.5)
                }
            };

            let is_overlap = overlap_pairs.iter().any(|(a, b)| *a == i || *b == i);

            speaker_segments.push(SpeakerSegment {
                segment_id: format!("spk_{}_{}", seg.start_ms, seg.end_ms),
                start_ms: seg.start_ms,
                end_ms: seg.end_ms,
                speaker_id: speaker,
                confidence,
                is_overlap,
            });
        }

        // ── Compute clustering quality ─────────────────────────────────────
        let quality = if unique_clusters.len() <= 1 {
            0.5 // Single cluster — low confidence in separation
        } else if overlap_pairs.is_empty() {
            0.8 // Multiple clusters, no overlaps — good separation
        } else {
            0.6 // Some overlaps — moderate quality
        };

        DiarizationOutput {
            speaker_count_estimate: speaker_count,
            speaker_segments,
            clustering_quality: quality,
        }
    }
}

// ── Feature Estimation ─────────────────────────────────────────────────────

fn extract_segment_features(seg: &VadSegment, audio: Option<&PcmAudio>) -> SegmentFeatures {
    if let Some(audio) = audio {
        if let Some(features) = extract_wav_features(seg, audio) {
            return features;
        }
    }

    fallback_features(seg)
}

fn extract_wav_features(seg: &VadSegment, audio: &PcmAudio) -> Option<SegmentFeatures> {
    let sample_rate = audio.sample_rate.max(1) as usize;
    let start_sample = ms_to_sample(seg.start_ms.max(0), sample_rate);
    let end_sample = ms_to_sample(seg.end_ms.max(seg.start_ms).max(0), sample_rate);
    let start_sample = start_sample.min(audio.samples.len());
    let end_sample = end_sample.min(audio.samples.len());
    if end_sample <= start_sample {
        return None;
    }

    let samples = &audio.samples[start_sample..end_sample];
    let frame_len = (sample_rate / 40).max(1); // 25 ms
    let hop_len = (sample_rate / 100).max(1); // 10 ms
    let min_frame_len = (sample_rate / 200).max(1); // 5 ms
    let mut rms_values = Vec::new();
    let mut zcr_values = Vec::new();
    let mut delta_values = Vec::new();
    let mut peak_energy = 0.0_f64;

    let mut offset = 0;
    while offset < samples.len() {
        let end = (offset + frame_len).min(samples.len());
        let frame = &samples[offset..end];
        if frame.len() < min_frame_len {
            break;
        }

        let rms = rms(frame);
        let zcr = zero_crossing_rate(frame);
        let delta = delta_rms(frame);
        let peak = frame
            .iter()
            .map(|sample| sample.abs() as f64)
            .fold(0.0, f64::max);

        rms_values.push(rms);
        zcr_values.push(zcr);
        delta_values.push(delta);
        peak_energy = peak_energy.max(peak);

        offset += hop_len;
    }

    if rms_values.is_empty() {
        return None;
    }

    let mean_energy = mean(&rms_values);
    let mean_zcr = mean(&zcr_values);
    let mean_delta_energy = mean(&delta_values);
    let energy_variance = variance(&rms_values, mean_energy);
    let duration_ms = (seg.end_ms - seg.start_ms).max(0) as f64;

    Some(SegmentFeatures {
        frame_count: rms_values.len(),
        mean_energy,
        energy_variance,
        mean_zcr,
        mean_delta_energy,
        peak_energy,
        duration_ms,
        snr_db: seg.snr_db,
    })
}

fn fallback_features(seg: &VadSegment) -> SegmentFeatures {
    let dur_ms = (seg.end_ms - seg.start_ms).max(0) as f64;
    SegmentFeatures {
        frame_count: (dur_ms / 10.0).ceil() as usize,
        mean_energy: estimate_energy(seg),
        energy_variance: estimate_energy_variance(seg, dur_ms),
        mean_zcr: estimate_zcr(seg),
        mean_delta_energy: estimate_zcr(seg) * estimate_energy(seg),
        peak_energy: estimate_energy(seg).min(1.0),
        duration_ms: dur_ms,
        snr_db: seg.snr_db,
    }
}

fn read_wav_pcm(path: &Path) -> Result<PcmAudio, String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }

    let mut cursor = 12usize;
    let mut sample_rate = None;
    let mut channels = None;
    let mut bits_per_sample = None;
    let mut audio_format = None;
    let mut data_range = None;

    while cursor + 8 <= bytes.len() {
        let chunk_id = &bytes[cursor..cursor + 4];
        let chunk_size = read_u32_le(&bytes, cursor + 4)
            .ok_or_else(|| "invalid WAV chunk header".to_string())?
            as usize;
        cursor += 8;
        let chunk_end = cursor
            .checked_add(chunk_size)
            .ok_or_else(|| "WAV chunk size overflow".to_string())?;
        if chunk_end > bytes.len() {
            return Err("WAV chunk extends beyond file".into());
        }

        match chunk_id {
            b"fmt " if chunk_size >= 16 => {
                audio_format = read_u16_le(&bytes, cursor);
                channels = read_u16_le(&bytes, cursor + 2);
                sample_rate = read_u32_le(&bytes, cursor + 4);
                bits_per_sample = read_u16_le(&bytes, cursor + 14);
            }
            b"data" => {
                data_range = Some(cursor..chunk_end);
            }
            _ => {}
        }

        cursor = chunk_end + (chunk_size % 2);
    }

    let audio_format = audio_format.ok_or_else(|| "missing WAV fmt chunk".to_string())?;
    if audio_format != 1 {
        return Err(format!(
            "unsupported WAV format {audio_format}; expected PCM"
        ));
    }
    let channels = channels.ok_or_else(|| "missing WAV channel count".to_string())?;
    if channels == 0 {
        return Err("WAV channel count is zero".into());
    }
    let sample_rate = sample_rate.ok_or_else(|| "missing WAV sample rate".to_string())?;
    if sample_rate == 0 {
        return Err("WAV sample rate is zero".into());
    }
    let bits_per_sample =
        bits_per_sample.ok_or_else(|| "missing WAV bits per sample".to_string())?;
    if bits_per_sample != 16 {
        return Err(format!(
            "unsupported WAV bit depth {bits_per_sample}; expected 16-bit PCM"
        ));
    }

    let data_range = data_range.ok_or_else(|| "missing WAV data chunk".to_string())?;
    let data = &bytes[data_range];
    let frame_bytes = channels as usize * 2;
    if frame_bytes == 0 {
        return Err("invalid WAV frame size".into());
    }

    let mut samples = Vec::with_capacity(data.len() / frame_bytes);
    for frame in data.chunks_exact(frame_bytes) {
        let mut mixed = 0.0_f32;
        for channel in 0..channels as usize {
            let offset = channel * 2;
            let sample = i16::from_le_bytes([frame[offset], frame[offset + 1]]) as f32 / 32768.0;
            mixed += sample;
        }
        samples.push(mixed / channels as f32);
    }

    if samples.is_empty() {
        return Err("WAV data chunk has no samples".into());
    }

    Ok(PcmAudio {
        sample_rate,
        samples,
    })
}

fn read_u16_le(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes([
        *bytes.get(offset)?,
        *bytes.get(offset + 1)?,
    ]))
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *bytes.get(offset)?,
        *bytes.get(offset + 1)?,
        *bytes.get(offset + 2)?,
        *bytes.get(offset + 3)?,
    ]))
}

fn ms_to_sample(ms: i64, sample_rate: usize) -> usize {
    ((ms as u128 * sample_rate as u128) / 1000) as usize
}

fn rms(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let power = samples
        .iter()
        .map(|sample| {
            let sample = *sample as f64;
            sample * sample
        })
        .sum::<f64>()
        / samples.len() as f64;
    power.sqrt()
}

fn zero_crossing_rate(samples: &[f32]) -> f64 {
    if samples.len() < 2 {
        return 0.0;
    }

    let crossings = samples
        .windows(2)
        .filter(|pair| (pair[0] >= 0.0 && pair[1] < 0.0) || (pair[0] < 0.0 && pair[1] >= 0.0))
        .count();
    crossings as f64 / (samples.len() - 1) as f64
}

fn delta_rms(samples: &[f32]) -> f64 {
    if samples.len() < 2 {
        return 0.0;
    }
    let power = samples
        .windows(2)
        .map(|pair| {
            let delta = (pair[1] - pair[0]) as f64;
            delta * delta
        })
        .sum::<f64>()
        / (samples.len() - 1) as f64;
    power.sqrt()
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

fn variance(values: &[f64], mean: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }

    values
        .iter()
        .map(|value| {
            let delta = value - mean;
            delta * delta
        })
        .sum::<f64>()
        / values.len() as f64
}

/// Estimate mean energy for a segment from metadata when WAV samples are not available.
fn estimate_energy(seg: &VadSegment) -> f64 {
    (seg.snr_db + 40.0) / 80.0
}

/// Estimate energy variance for a segment from metadata.
fn estimate_energy_variance(_seg: &VadSegment, duration_ms: f64) -> f64 {
    0.1 + duration_ms / 10000.0
}

/// Estimate zero-crossing rate for a segment from metadata.
fn estimate_zcr(seg: &VadSegment) -> f64 {
    if seg.snr_db < 20.0 {
        0.6
    } else {
        0.3
    }
}

// ── Agglomerative Clustering ───────────────────────────────────────────────

/// Simple agglomerative clustering with weighted feature distance.
///
/// Algorithm:
/// 1. Start with each point as its own cluster
/// 2. Repeatedly merge the closest pair of clusters
/// 3. Stop when the minimum distance exceeds the threshold
fn agglomerative_cluster(vectors: &[Vec<f64>], threshold: f64) -> Vec<usize> {
    let n = vectors.len();
    if n == 0 {
        return vec![];
    }
    if n == 1 {
        return vec![0];
    }

    // Initialize: each point is its own cluster
    let mut labels: Vec<usize> = (0..n).collect();
    let mut cluster_count = n;

    // Compute pairwise distances
    let mut distances: Vec<(usize, usize, f64)> = Vec::new();
    for i in 0..n {
        for j in (i + 1)..n {
            let d = SegmentFeatures::feature_distance(&vectors[i], &vectors[j]);
            distances.push((i, j, d));
        }
    }
    distances.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap());

    // Merge until threshold exceeded
    for (i, j, dist) in &distances {
        if *dist > threshold {
            break; // All remaining distances exceed threshold
        }

        let label_i = labels[*i];
        let label_j = labels[*j];

        if label_i != label_j {
            // Merge cluster j into cluster i
            let min_label = label_i.min(label_j);
            let max_label = label_i.max(label_j);
            for label in labels.iter_mut() {
                if *label == max_label {
                    *label = min_label;
                }
            }
            cluster_count -= 1;
        }

        if cluster_count <= 1 {
            break;
        }
    }

    // Re-label clusters to be 0, 1, 2, ...
    renumber_labels(&labels)
}

fn cap_cluster_count(vectors: &[Vec<f64>], labels: &[usize], max_clusters: usize) -> Vec<usize> {
    if labels.is_empty() || max_clusters == 0 {
        return labels.to_vec();
    }

    let mut labels = renumber_labels(labels);
    while unique_label_count(&labels) > max_clusters {
        let centroids = cluster_centroids(vectors, &labels);
        let mut closest_pair: Option<(usize, usize, f64)> = None;

        for i in 0..centroids.len() {
            for j in (i + 1)..centroids.len() {
                let dist = SegmentFeatures::feature_distance(&centroids[i], &centroids[j]);
                if closest_pair
                    .as_ref()
                    .is_none_or(|(_, _, best_dist)| dist < *best_dist)
                {
                    closest_pair = Some((i, j, dist));
                }
            }
        }

        let Some((keep, merge, _)) = closest_pair else {
            break;
        };
        for label in &mut labels {
            if *label == merge {
                *label = keep;
            }
        }
        labels = renumber_labels(&labels);
    }

    labels
}

fn unique_label_count(labels: &[usize]) -> usize {
    let mut unique = labels.to_vec();
    unique.sort_unstable();
    unique.dedup();
    unique.len()
}

fn cluster_centroids(vectors: &[Vec<f64>], labels: &[usize]) -> Vec<Vec<f64>> {
    let cluster_count = unique_label_count(labels);
    if cluster_count == 0 {
        return Vec::new();
    }

    let dim = vectors.first().map(|vector| vector.len()).unwrap_or(0);
    let mut sums = vec![vec![0.0; dim]; cluster_count];
    let mut counts = vec![0usize; cluster_count];

    for (vector, label) in vectors.iter().zip(labels) {
        if *label >= cluster_count {
            continue;
        }
        counts[*label] += 1;
        for (index, value) in vector.iter().take(dim).enumerate() {
            sums[*label][index] += value;
        }
    }

    for (centroid, count) in sums.iter_mut().zip(counts) {
        if count == 0 {
            continue;
        }
        for value in centroid {
            *value /= count as f64;
        }
    }

    sums
}

/// Renumber cluster labels to be consecutive starting from 0.
fn renumber_labels(labels: &[usize]) -> Vec<usize> {
    let mut unique: Vec<usize> = labels.to_vec();
    unique.sort_unstable();
    unique.dedup();

    let mut map = HashMap::new();
    for (i, &old) in unique.iter().enumerate() {
        map.insert(old, i);
    }

    labels.iter().map(|l| map[l]).collect()
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn make_vad(start_ms: i64, end_ms: i64, snr: f64) -> VadSegment {
        VadSegment {
            start_ms,
            end_ms,
            has_speech: true,
            snr_db: snr,
        }
    }

    fn write_sine_wav(path: &Path, tones: &[(f64, f64, i64)]) {
        const SAMPLE_RATE: u32 = 16_000;

        let mut samples = Vec::new();
        for (frequency_hz, amplitude, duration_ms) in tones {
            let sample_count = (SAMPLE_RATE as i64 * *duration_ms / 1_000).max(0) as usize;
            for index in 0..sample_count {
                let t = index as f64 / SAMPLE_RATE as f64;
                let sample = (std::f64::consts::TAU * frequency_hz * t).sin() * amplitude;
                samples.push((sample * i16::MAX as f64) as i16);
            }
        }

        let data_len = samples.len() * 2;
        let mut bytes = Vec::with_capacity(44 + data_len);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
        bytes.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data_len as u32).to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }

        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn test_single_speaker() {
        let worker = DiarizationWorker::new();
        let input = DiarizationInput {
            speech_segments: vec![
                make_vad(0, 2000, 30.0),
                make_vad(2500, 5000, 28.0),
                make_vad(5500, 8000, 32.0),
            ],
            wav_path: std::path::PathBuf::from("test.wav"),
            max_speakers: 0,
        };

        let output = worker.run(&input);
        assert!(output.speaker_count_estimate >= 1);
        assert_eq!(output.speaker_segments.len(), 3);
        // Similar SNR → likely same speaker
    }

    #[test]
    fn test_two_speakers_different_snr() {
        let worker = DiarizationWorker::new();
        let input = DiarizationInput {
            speech_segments: vec![
                make_vad(0, 2000, 35.0),     // Speaker A: loud
                make_vad(2500, 5000, 10.0),  // Speaker B: quiet
                make_vad(5500, 8000, 33.0),  // Speaker A: loud
                make_vad(8500, 11000, 12.0), // Speaker B: quiet
            ],
            wav_path: std::path::PathBuf::from("test.wav"),
            max_speakers: 0,
        };

        let output = worker.run(&input);
        // Different SNR patterns → likely 2 speakers
        assert!(output.speaker_count_estimate >= 2);
        assert_eq!(output.speaker_segments.len(), 4);
    }

    #[test]
    fn test_wav_features_separate_alternating_speakers() {
        let temp_dir = tempfile::tempdir().unwrap();
        let wav_path = temp_dir.path().join("alternating-speakers.wav");
        write_sine_wav(
            &wav_path,
            &[
                (220.0, 0.55, 1_000),
                (660.0, 0.55, 1_000),
                (220.0, 0.55, 1_000),
                (660.0, 0.55, 1_000),
            ],
        );

        let worker = DiarizationWorker::new();
        let input = DiarizationInput {
            speech_segments: vec![
                make_vad(0, 1_000, 20.0),
                make_vad(1_000, 2_000, 20.0),
                make_vad(2_000, 3_000, 20.0),
                make_vad(3_000, 4_000, 20.0),
            ],
            wav_path,
            max_speakers: 0,
        };

        let output = worker.run(&input);
        let speakers = output
            .speaker_segments
            .iter()
            .map(|segment| segment.speaker_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(output.speaker_count_estimate, 2);
        assert_eq!(speakers[0], speakers[2]);
        assert_eq!(speakers[1], speakers[3]);
        assert_ne!(speakers[0], speakers[1]);
    }

    #[test]
    fn test_overlap_detection() {
        let worker = DiarizationWorker::new();
        let input = DiarizationInput {
            speech_segments: vec![
                make_vad(0, 3000, 30.0),
                make_vad(2000, 5000, 15.0), // Overlaps with first segment
                make_vad(5500, 8000, 28.0),
            ],
            wav_path: std::path::PathBuf::from("test.wav"),
            max_speakers: 0,
        };

        let output = worker.run(&input);
        let has_overlap = output.speaker_segments.iter().any(|s| s.is_overlap);
        assert!(has_overlap);
    }

    #[test]
    fn test_max_speakers_cap() {
        let worker = DiarizationWorker::new();
        let input = DiarizationInput {
            speech_segments: vec![
                make_vad(0, 1000, 35.0),
                make_vad(2000, 3000, 10.0),
                make_vad(4000, 5000, 33.0),
                make_vad(6000, 7000, 12.0),
                make_vad(8000, 9000, 31.0),
            ],
            wav_path: std::path::PathBuf::from("test.wav"),
            max_speakers: 2,
        };

        let output = worker.run(&input);
        assert!(output.speaker_count_estimate <= 2);
    }

    #[test]
    fn test_empty_input() {
        let worker = DiarizationWorker::new();
        let input = DiarizationInput {
            speech_segments: vec![],
            wav_path: std::path::PathBuf::from("test.wav"),
            max_speakers: 0,
        };

        let output = worker.run(&input);
        assert_eq!(output.speaker_count_estimate, 1);
        assert!(output.speaker_segments.is_empty());
    }

    #[test]
    fn test_short_segments_filtered() {
        let worker = DiarizationWorker::new();
        let input = DiarizationInput {
            speech_segments: vec![
                make_vad(0, 200, 30.0),     // Too short → filtered from clustering
                make_vad(1000, 3000, 28.0), // Long enough
            ],
            wav_path: std::path::PathBuf::from("test.wav"),
            max_speakers: 0,
        };

        let output = worker.run(&input);
        // Both segments appear in output, but the short one may have lower confidence
        // Short segments still get speaker labels even if not used for clustering
        assert_eq!(output.speaker_segments.len(), 2);
        // The short segment should have lower confidence
        let short_seg = output
            .speaker_segments
            .iter()
            .find(|s| s.start_ms == 0)
            .unwrap();
        assert!(short_seg.confidence <= 0.55); // Short segments get penalized
    }

    #[test]
    fn test_clustering_same_features_same_cluster() {
        // Vectors with clear separation
        let vectors = vec![
            vec![0.9, 0.1, 0.1, 0.9, 0.9, 0.1],       // cluster 0
            vec![0.89, 0.11, 0.12, 0.88, 0.91, 0.09], // cluster 0 (similar)
            vec![0.1, 0.9, 0.9, 0.1, 0.1, 0.9],       // cluster 1 (very different)
        ];
        let labels = agglomerative_cluster(&vectors, 0.35);
        assert_eq!(
            labels[0], labels[1],
            "Similar vectors should be in same cluster"
        );
        assert_ne!(
            labels[0], labels[2],
            "Different vectors should be in separate clusters"
        );
    }
}
