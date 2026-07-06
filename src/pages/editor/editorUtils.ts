import type { TranscriptSegment } from '../../types';
import type { Translate } from '../../tauri';

const LOW_CONFIDENCE_REASON_LABELS: Record<string, string> = {
  low_snr: 'Low SNR',
  high_noise: 'Noise',
  term_conflict: 'Term',
  overlapping_speech: 'Overlap',
  speaker_uncertain: 'Speaker',
  low_confidence: 'ASR',
};

const LOW_CONFIDENCE_REASON_WEIGHTS: Record<string, number> = {
  overlapping_speech: 40,
  term_conflict: 34,
  low_snr: 30,
  high_noise: 26,
  speaker_uncertain: 24,
  low_confidence: 18,
};

export function lowConfidenceRiskScore(segment: TranscriptSegment): number {
  const confidencePenalty = Math.round((1 - Math.max(0, Math.min(1, segment.confidence))) * 70);
  const reasonPenalty = segment.lowConfidenceReasons.reduce(
    (max, reason) => Math.max(max, LOW_CONFIDENCE_REASON_WEIGHTS[reason] ?? 16),
    0
  );
  return Math.min(100, confidencePenalty + reasonPenalty);
}

export function riskLevel(score: number): 'high' | 'medium' | 'low' {
  if (score >= 70) return 'high';
  if (score >= 45) return 'medium';
  return 'low';
}

export function riskLabel(level: ReturnType<typeof riskLevel>, t: Translate): string {
  return t(`editor.risk.${level}`);
}

export function reasonLabel(reason: string, t: Translate): string {
  const key = `editor.reason.${reason}`;
  const translated = t(key);
  return translated === key ? LOW_CONFIDENCE_REASON_LABELS[reason] ?? reason.replace(/_/g, ' ') : translated;
}

export function segmentTextDiff(segment: TranscriptSegment): { original: string; replacement: string } | null {
  if (segment.rawText === segment.text) return null;

  const before = Array.from(segment.rawText);
  const after = Array.from(segment.text);
  let prefix = 0;
  while (prefix < before.length && prefix < after.length && before[prefix] === after[prefix]) {
    prefix += 1;
  }

  let beforeSuffix = before.length - 1;
  let afterSuffix = after.length - 1;
  while (
    beforeSuffix >= prefix &&
    afterSuffix >= prefix &&
    before[beforeSuffix] === after[afterSuffix]
  ) {
    beforeSuffix -= 1;
    afterSuffix -= 1;
  }

  const original = before.slice(prefix, beforeSuffix + 1).join('').trim();
  const replacement = after.slice(prefix, afterSuffix + 1).join('').trim();
  if (!original || !replacement || original === replacement) return null;
  return { original, replacement };
}

const WAVEFORM_BAR_COUNT = 120;
export const MAX_WAVEFORM_DECODE_BYTES = 120 * 1024 * 1024;

export function extractWaveformPeaks(audioBuffer: AudioBuffer, barCount = WAVEFORM_BAR_COUNT): number[] {
  const peaks: number[] = [];
  const channelCount = Math.max(1, Math.min(audioBuffer.numberOfChannels, 2));
  const samplesPerBar = Math.max(1, Math.floor(audioBuffer.length / barCount));

  for (let i = 0; i < barCount; i += 1) {
    const start = i * samplesPerBar;
    const end = Math.min(audioBuffer.length, start + samplesPerBar);
    const step = Math.max(1, Math.floor((end - start) / 240));
    let peak = 0;

    for (let sample = start; sample < end; sample += step) {
      let value = 0;
      for (let channel = 0; channel < channelCount; channel += 1) {
        value += Math.abs(audioBuffer.getChannelData(channel)[sample] ?? 0);
      }
      peak = Math.max(peak, value / channelCount);
    }

    peaks.push(peak);
  }

  const maxPeak = Math.max(...peaks, 0.01);
  return peaks.map((peak) => Math.max(0.08, Math.min(1, peak / maxPeak)));
}

export function buildSegmentTimelinePeaks(
  segments: TranscriptSegment[],
  totalDuration: number,
  barCount = WAVEFORM_BAR_COUNT
): number[] {
  if (segments.length === 0 || totalDuration <= 0) return [];

  const peaks = Array.from({ length: barCount }, () => 0.08);
  for (const segment of segments) {
    const start = Math.max(0, Math.floor((segment.startMs / 1000 / totalDuration) * barCount));
    const end = Math.min(barCount - 1, Math.ceil((segment.endMs / 1000 / totalDuration) * barCount));
    const base = Math.max(0.22, Math.min(0.92, 0.26 + segment.confidence * 0.58));
    for (let index = start; index <= end; index += 1) {
      const variation = ((index % 7) / 7) * 0.16;
      peaks[index] = Math.max(peaks[index], Math.min(1, base + variation));
    }
  }

  return peaks;
}

// Demo data simulating a transcribed interview
export const DEMO_SEGMENTS: TranscriptSegment[] = [
  { id: 's1', startMs: 0, endMs: 4200, speaker: 'A', text: '今天我们来聊一聊人工智能在医疗领域的应用。', rawText: '今天我们来聊一聊人工智能在医疗领域的应用。', confidence: 0.94, lowConfidenceReasons: [], hasCorrection: false, hasMark: false, marks: [] },
  { id: 's2', startMs: 4500, endMs: 9800, speaker: 'B', text: '是的，最近腾训在医疗AI方面投入很大。', rawText: '是的，最近腾训在医疗AI方面投入很大。', confidence: 0.72, lowConfidenceReasons: ['term_conflict'], hasCorrection: true, hasMark: false, marks: [] },
  { id: 's3', startMs: 10200, endMs: 15800, speaker: 'A', text: '你说的是腾讯吧？他们确实在医学影像分析上做得不错。', rawText: '你说的是腾训吧？他们确实在医学影像分析上做得不错。', confidence: 0.88, lowConfidenceReasons: [], hasCorrection: true, hasMark: false, marks: [] },
  { id: 's4', startMs: 16200, endMs: 22100, speaker: 'B', text: '对，就是腾讯。另外，字节跳动也在探索AI辅助诊断。', rawText: '对，就是腾训。另外，字节跳动也在探索AI辅助诊断。', confidence: 0.85, lowConfidenceReasons: [], hasCorrection: true, hasMark: false, marks: [] },
  { id: 's5', startMs: 22500, endMs: 26300, speaker: 'A', text: '这是一个值得关注的方向。', rawText: '这是一个值得关注的方向。', confidence: 0.96, lowConfidenceReasons: [], hasCorrection: false, hasMark: true, marks: [{ id: 1, segmentId: 's5', markMs: 24000, label: 'Mark' }] },
];

