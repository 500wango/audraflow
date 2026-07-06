import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { convertFileSrc } from '@tauri-apps/api/core';
import type { GlossaryApplyResult, TimestampMark, TranscriptResponse, TranscriptSegment } from '../types';
import { formatTime, recordTelemetry } from '../appUtils';
import { errorToMessage, hasTauriRuntime, invokeTauri } from '../tauri';
import { useI18n } from '../useI18n';
import { EditorView } from './editor/EditorView';
import {
  buildSegmentTimelinePeaks,
  DEMO_SEGMENTS,
  extractWaveformPeaks,
  lowConfidenceRiskScore,
  MAX_WAVEFORM_DECODE_BYTES,
  segmentTextDiff,
} from './editor/editorUtils';

export function EditorPage({ jobId }: { jobId: string | null }) {
  const { t } = useI18n();
  const editorIsTauriRuntime = hasTauriRuntime();
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const proofreadSessionRef = useRef<{
    jobId: string;
    startedAtMs: number;
    completedRatio: number;
  } | null>(null);
  const [currentSegmentId, setCurrentSegmentId] = useState<string | null>(null);
  const [isPlaying, setIsPlaying] = useState(false);
  const [speed, setSpeed] = useState(1.0);
  const [currentTime, setCurrentTime] = useState(0);
  const [segments, setSegments] = useState<TranscriptSegment[]>(() => (
    editorIsTauriRuntime ? [] : DEMO_SEGMENTS
  ));
  const [editingSpeaker, setEditingSpeaker] = useState<string | null>(null);
  const [speakerEdits, setSpeakerEdits] = useState<Record<string, string>>({});
  const [termActionSegmentId, setTermActionSegmentId] = useState<string | null>(null);
  const [mediaSrc, setMediaSrc] = useState<string | null>(null);
  const [waveform, setWaveform] = useState<{ source: string; peaks: number[] } | null>(null);
  const [loadState, setLoadState] = useState<'idle' | 'loading' | 'ready' | 'error'>(() => (
    editorIsTauriRuntime ? 'idle' : 'ready'
  ));
  const [editorStatus, setEditorStatus] = useState<string | null>(() => (
    editorIsTauriRuntime ? null : t('editor.browserPreview')
  ));
  const [searchQuery, setSearchQuery] = useState('');
  const [searchResults, setSearchResults] = useState<TranscriptSegment[]>([]);
  const [searchIndex, setSearchIndex] = useState(0);
  const [searchBusy, setSearchBusy] = useState(false);

  const totalDuration = segments.length > 0
    ? segments[segments.length - 1].endMs / 1000
    : 0;
  const displayedWaveformPeaks = useMemo(
    () => (
      waveform?.source === mediaSrc && waveform.peaks.length > 0
        ? waveform.peaks
        : buildSegmentTimelinePeaks(segments, totalDuration)
    ),
    [mediaSrc, segments, totalDuration, waveform]
  );
  const searchResultIds = useMemo(
    () => new Set(searchResults.map((segment) => segment.id)),
    [searchResults]
  );

  useEffect(() => {
    let cancelled = false;
    if (!mediaSrc) return;

    const AudioContextCtor = window.AudioContext
      ?? (window as Window & { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
    if (!AudioContextCtor) return;

    const controller = new AbortController();
    let audioContext: AudioContext | null = null;

    void fetch(mediaSrc, { signal: controller.signal })
      .then((response) => {
        if (!response.ok) {
          throw new Error(`Could not load waveform source: ${response.status}`);
        }
        return response.arrayBuffer();
      })
      .then(async (buffer) => {
        if (buffer.byteLength > MAX_WAVEFORM_DECODE_BYTES) {
          throw new Error('Audio is too large for inline waveform decoding.');
        }
        audioContext = new AudioContextCtor();
        const decoded = await audioContext.decodeAudioData(buffer);
        if (!cancelled) {
          setWaveform({ source: mediaSrc, peaks: extractWaveformPeaks(decoded) });
        }
      })
      .catch((error) => {
        if (!cancelled && error instanceof Error && error.name !== 'AbortError') {
          setWaveform((current) => current?.source === mediaSrc ? null : current);
        }
      })
      .finally(() => {
        void audioContext?.close().catch(() => {});
      });

    return () => {
      cancelled = true;
      controller.abort();
    };
  }, [mediaSrc]);

  useEffect(() => {
    let cancelled = false;

    if (!jobId) {
      queueMicrotask(() => {
        if (cancelled) return;
        setIsPlaying(false);
        setCurrentTime(0);
        setCurrentSegmentId(null);
        setMediaSrc(null);
        setSpeakerEdits({});
        setSearchQuery('');
        setSearchResults([]);
        setSearchIndex(0);
        if (editorIsTauriRuntime) {
          setSegments([]);
          setLoadState('idle');
          setEditorStatus(t('editor.openCompleted'));
        } else {
          setSegments(DEMO_SEGMENTS);
          setLoadState('ready');
          setEditorStatus(t('editor.browserPreview'));
        }
      });
      return () => {
        cancelled = true;
      };
    }

    queueMicrotask(() => {
      if (cancelled) return;
      setIsPlaying(false);
      setCurrentTime(0);
      setSegments([]);
      setSpeakerEdits({});
      setSearchResults([]);
      setSearchIndex(0);
      setMediaSrc(null);
      setLoadState('loading');
      setEditorStatus(t('editor.loading'));
    });

    void invokeTauri<TranscriptResponse>('cmd_get_transcript', { jobId })
      .then((response) => {
        if (cancelled) return;
        setSegments(response.segments);
        setSpeakerEdits({});
        setCurrentSegmentId(response.segments[0]?.id ?? null);
        setMediaSrc(convertFileSrc(response.mediaSrcPath || response.filePath));
        setLoadState('ready');
        setEditorStatus(t('editor.loadedSegments', { count: response.segments.length }));
        proofreadSessionRef.current = {
          jobId,
          startedAtMs: Date.now(),
          completedRatio: response.segments.length > 0 ? 1 : 0,
        };
        recordTelemetry({
          eventType: 'proofread_session_start',
          jobId,
          audioHours: (response.segments.at(-1)?.endMs ?? 0) / 3_600_000,
          transcriptChars: response.segments.reduce((sum, segment) => sum + segment.text.length, 0),
        });
      })
      .catch((error) => {
        if (cancelled) return;
        setLoadState('error');
        setEditorStatus(errorToMessage(error, t));
      });

    return () => {
      cancelled = true;
      const session = proofreadSessionRef.current;
      if (session?.jobId === jobId) {
        const activeSeconds = Math.max(0, (Date.now() - session.startedAtMs) / 1000);
        recordTelemetry({
          eventType: 'proofread_session_end',
          jobId,
          activeSeconds,
          inactiveSeconds: 0,
          completedRatio: session.completedRatio,
        });
        proofreadSessionRef.current = null;
      }
    };
  }, [editorIsTauriRuntime, jobId, t]);

  const handleInsertMark = useCallback(() => {
    const currentMs = Math.round(currentTime * 1000);
    const activeSegment = segments.find((segment) =>
      currentMs >= segment.startMs && currentMs < segment.endMs
    ) ?? segments.find((segment) => segment.id === currentSegmentId);
    if (!activeSegment) return;

    const optimisticMark: TimestampMark = {
      id: Date.now(),
      segmentId: activeSegment.id,
      markMs: currentMs,
      label: 'Mark',
    };
    setSegments(prev => prev.map(s =>
      s.id === activeSegment.id
        ? { ...s, hasMark: true, marks: [...s.marks, optimisticMark] }
        : s
    ));
    setEditorStatus(t('editor.markAdded', { time: formatTime(currentMs) }));

    if (!jobId) return;

    void invokeTauri<TranscriptSegment>('cmd_add_timestamp_mark', {
      request: {
        segmentId: activeSegment.id,
        markMs: currentMs,
        label: 'Mark',
      },
    })
      .then((updated) => {
        setSegments((prev) => prev.map((segment) =>
          segment.id === activeSegment.id ? updated : segment
        ));
        setEditorStatus(t('editor.markSaved', { time: formatTime(currentMs) }));
      })
      .catch((error) => {
        setSegments((prev) => prev.map((segment) =>
          segment.id === activeSegment.id
            ? {
                ...segment,
                marks: segment.marks.filter((mark) => mark.id !== optimisticMark.id),
                hasMark: segment.marks.some((mark) => mark.id !== optimisticMark.id),
              }
            : segment
        ));
        setEditorStatus(errorToMessage(error, t));
      });
  }, [currentSegmentId, currentTime, jobId, segments, t]);

  const handleAcceptCandidate = useCallback((target?: TranscriptSegment) => {
    const segment = target ?? segments.find((item) => item.id === currentSegmentId) ?? segments.find((item) => item.hasCorrection);
    if (!segment) return;

    setTermActionSegmentId(segment.id);
    if (!jobId) {
      setSegments((prev) => prev.map((item) =>
        item.id === segment.id
          ? {
              ...item,
              lowConfidenceReasons: item.lowConfidenceReasons.filter((reason) => reason !== 'term_conflict'),
              hasCorrection: true,
            }
          : item
      ));
      setEditorStatus(t('editor.acceptedCandidate'));
      setTermActionSegmentId(null);
      return;
    }

    void invokeTauri<TranscriptSegment>('cmd_accept_term_candidate', {
      request: { segmentId: segment.id },
    })
      .then((updated) => {
        setSegments((prev) => prev.map((item) => item.id === updated.id ? updated : item));
        setEditorStatus(t('editor.acceptedCandidate'));
      })
      .catch((error) => setEditorStatus(errorToMessage(error, t)))
      .finally(() => setTermActionSegmentId(null));
  }, [currentSegmentId, jobId, segments, t]);

  const handleAddToGlossary = useCallback((target?: TranscriptSegment) => {
    const segment = target ?? segments.find((item) => item.id === currentSegmentId) ?? segments.find((item) => item.hasCorrection);
    if (!segment) return;

    const diff = segmentTextDiff(segment);
    if (!diff) {
      setEditorStatus(t('editor.glossaryNoDiff'));
      return;
    }

    setTermActionSegmentId(segment.id);
    if (!jobId) {
      setEditorStatus(t('editor.addedGlossary'));
      setTermActionSegmentId(null);
      return;
    }

    void invokeTauri<GlossaryApplyResult>('cmd_add_glossary_entry', {
      request: {
        jobId,
        canonical: diff.replacement,
        aliases: [diff.original],
        category: 'term',
      },
    })
      .then((result) => {
        if (result.updatedSegments.length > 0) {
          setSegments(result.updatedSegments);
        }
        setEditorStatus(t('editor.glossaryApplied', {
          canonical: result.entry.canonical,
          count: result.updatedCount,
        }));
      })
      .catch((error) => setEditorStatus(errorToMessage(error, t)))
      .finally(() => setTermActionSegmentId(null));
  }, [currentSegmentId, jobId, segments, t]);

  const seekAudio = useCallback((
    seconds: number,
    play = false,
    trigger = 'manual_scrub',
    segmentId?: string
  ) => {
    const bounded = Math.max(0, Math.min(totalDuration || seconds, seconds));
    const fromMs = Math.round(currentTime * 1000);
    const toMs = Math.round(bounded * 1000);
    const audio = audioRef.current;
    if (audio) {
      audio.currentTime = bounded;
      if (play) {
        void audio.play().catch((error) => {
          setIsPlaying(false);
          setEditorStatus(errorToMessage(error, t));
        });
      }
    }
    setCurrentTime(bounded);
    if (jobId && fromMs !== toMs) {
      recordTelemetry({
        eventType: 'playback_seek',
        jobId,
        segmentId: segmentId ?? currentSegmentId,
        fromMs,
        toMs,
        trigger,
      });
    }
  }, [currentSegmentId, currentTime, jobId, t, totalDuration]);

  const focusSearchResult = useCallback((results: TranscriptSegment[], nextIndex: number) => {
    const result = results[nextIndex];
    if (!result) return;
    setSearchIndex(nextIndex);
    setCurrentSegmentId(result.id);
    seekAudio(result.startMs / 1000, false, 'search_result', result.id);
    window.requestAnimationFrame(() => {
      document.getElementById(`seg-${result.id}`)?.scrollIntoView({
        block: 'center',
        behavior: 'smooth',
      });
    });
  }, [seekAudio]);

  const handleSearch = useCallback(async () => {
    const query = searchQuery.trim();
    if (!query) {
      setSearchResults([]);
      setSearchIndex(0);
      setEditorStatus(t('editor.searchEmpty'));
      return;
    }

    setSearchBusy(true);
    try {
      const results = jobId
        ? await invokeTauri<TranscriptSegment[]>('cmd_search_transcript', { jobId, query })
        : segments.filter((segment) => {
            const needle = query.toLowerCase();
            return (
              segment.text.toLowerCase().includes(needle) ||
              segment.rawText.toLowerCase().includes(needle) ||
              segment.speaker.toLowerCase().includes(needle)
            );
          });
      setSearchResults(results);
      setEditorStatus(t('editor.searchCount', { count: results.length }));
      if (results.length > 0) {
        focusSearchResult(results, 0);
      }
    } catch (error) {
      setEditorStatus(errorToMessage(error, t));
    } finally {
      setSearchBusy(false);
    }
  }, [focusSearchResult, jobId, searchQuery, segments, t]);

  const handleSearchStep = useCallback((direction: 1 | -1) => {
    if (searchResults.length === 0) return;
    const nextIndex = (searchIndex + direction + searchResults.length) % searchResults.length;
    focusSearchResult(searchResults, nextIndex);
  }, [focusSearchResult, searchIndex, searchResults]);

  const saveSegmentChange = useCallback(async (
    segmentId: string,
    patch: { text?: string; speaker?: string }
  ) => {
    if (!jobId) return;

    const current = segments.find((segment) => segment.id === segmentId);
    if (!current) return;
    if (patch.speaker !== undefined && patch.speaker === current.speaker) return;

    setSegments((prev) => prev.map((segment) =>
      segment.id === segmentId
        ? {
            ...segment,
            ...patch,
            hasCorrection: true,
          }
        : segment
    ));
    setEditorStatus(t('editor.saving'));

    try {
      const updated = await invokeTauri<TranscriptSegment>('cmd_update_segment', {
        request: {
          segmentId,
          ...patch,
        },
      });
      setSegments((prev) => prev.map((segment) =>
        segment.id === segmentId ? updated : segment
      ));
      setEditorStatus(t('editor.saved'));
    } catch (error) {
      setSegments((prev) => prev.map((segment) =>
        segment.id === segmentId ? current : segment
      ));
      setEditorStatus(errorToMessage(error, t));
    }
  }, [jobId, segments, t]);

  // ── Keyboard shortcuts (PRD §11.1) ──────────────────────────────────────
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      // Don't capture when typing in an input
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;

      const idx = segments.findIndex(s => s.id === currentSegmentId);

      switch (e.key) {
        case ' ':
          e.preventDefault();
          setIsPlaying(p => !p);
          break;
        case 'ArrowUp':
          e.preventDefault();
          if (idx > 0) setCurrentSegmentId(segments[idx - 1].id);
          break;
        case 'ArrowDown':
          e.preventDefault();
          if (idx < segments.length - 1) setCurrentSegmentId(segments[idx + 1].id);
          break;
        case 'j':
          e.preventDefault();
          seekAudio(currentTime - 3, isPlaying, 'jump_back', currentSegmentId ?? undefined);
          break;
        case 'l':
          e.preventDefault();
          seekAudio(currentTime + 3, isPlaying, 'jump_forward', currentSegmentId ?? undefined);
          break;
        case 't':
          if (e.ctrlKey) {
            e.preventDefault();
            handleInsertMark();
          }
          break;
        case '1':
          if (e.ctrlKey) { e.preventDefault(); setSpeed(0.5); }
          break;
        case '2':
          if (e.ctrlKey) { e.preventDefault(); setSpeed(1.0); }
          break;
        case '3':
          if (e.ctrlKey) { e.preventDefault(); setSpeed(1.5); }
          break;
        case 'Enter':
          if (e.ctrlKey) {
            e.preventDefault();
            handleAcceptCandidate();
          }
          break;
        case 'k':
          if (e.ctrlKey) {
            e.preventDefault();
            handleAddToGlossary();
          }
          break;
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [
    currentSegmentId,
    currentTime,
    handleAcceptCandidate,
    handleAddToGlossary,
    handleInsertMark,
    isPlaying,
    seekAudio,
    segments,
    totalDuration,
  ]);

  useEffect(() => {
    const audio = audioRef.current;
    if (!audio) return;
    audio.playbackRate = speed;
  }, [speed, mediaSrc]);

  useEffect(() => {
    const audio = audioRef.current;
    if (!audio || !mediaSrc) return;

    if (isPlaying) {
      void audio.play().catch((error) => {
        setIsPlaying(false);
        setEditorStatus(errorToMessage(error, t));
      });
    } else {
      audio.pause();
    }
  }, [isPlaying, mediaSrc, t]);

  useEffect(() => {
    if (mediaSrc) return;
    if (!isPlaying) return;
    const interval = window.setInterval(() => {
      setCurrentTime(t => {
        const next = t + 0.1 * speed;
        if (next >= totalDuration) {
          setIsPlaying(false);
          return totalDuration;
        }
        return next;
      });
    }, 100);
    return () => window.clearInterval(interval);
  }, [isPlaying, mediaSrc, speed, totalDuration]);

  const playbackSegmentId = useMemo(() => {
    const currentMs = currentTime * 1000;
    const active = segments.find(s => currentMs >= s.startMs && currentMs < s.endMs);
    return active?.id ?? null;
  }, [currentTime, segments]);

  const activeSegmentId = playbackSegmentId ?? currentSegmentId;

  // ── Actions ─────────────────────────────────────────────────────────────
  const handleRenameSpeaker = (segmentId: string, newName: string) => {
    setEditingSpeaker(null);
    const trimmed = newName.trim();
    if (!trimmed) return;
    void saveSegmentChange(segmentId, { speaker: trimmed });
  };

  // ── Sidebar data ────────────────────────────────────────────────────────
  const lowConfidenceItems = useMemo(() => [...segments]
    .filter(segment => segment.lowConfidenceReasons.length > 0)
    .sort((a, b) => {
      const riskDelta = lowConfidenceRiskScore(b) - lowConfidenceRiskScore(a);
      return riskDelta !== 0 ? riskDelta : a.startMs - b.startMs;
    }), [segments]);
  const correctionItems = segments.filter(s => s.hasCorrection);
  const handleSpeakerLabelChange = useCallback(async (fromSpeaker: string, toSpeaker: string) => {
    const trimmed = toSpeaker.trim();
    if (!trimmed || trimmed === fromSpeaker) return;

    const previousSegments = segments;
    const isMerge = segments.some((segment) => segment.speaker === trimmed);
    setSegments((prev) => prev.map((segment) =>
      segment.speaker === fromSpeaker
        ? { ...segment, speaker: trimmed, hasCorrection: true }
        : segment
    ));
    setSpeakerEdits((prev) => {
      const next = { ...prev };
      delete next[fromSpeaker];
      return next;
    });
    setEditorStatus(isMerge ? t('editor.merging') : t('editor.renaming'));

    if (!jobId) {
      setEditorStatus(isMerge ? t('editor.mergedPreview') : t('editor.renamedPreview'));
      return;
    }

    try {
      const updated = await invokeTauri<TranscriptSegment[]>('cmd_update_speaker_label', {
        request: {
          jobId,
          fromSpeaker,
          toSpeaker: trimmed,
        },
      });
      setSegments(updated);
      setEditorStatus(isMerge
        ? t('editor.merged', { from: fromSpeaker, to: trimmed })
        : t('editor.renamed', { from: fromSpeaker, to: trimmed }));
    } catch (error) {
      setSegments(previousSegments);
      setEditorStatus(errorToMessage(error, t));
    }
  }, [jobId, segments, t]);
  const speakerCounts = useMemo(() => segments.reduce<Record<string, number>>((counts, segment) => {
    counts[segment.speaker] = (counts[segment.speaker] ?? 0) + 1;
    return counts;
  }, {}), [segments]);
  const speakers = useMemo(() => Object.keys(speakerCounts), [speakerCounts]);

  // ── Format time ─────────────────────────────────────────────────────────
  // ── Render ──────────────────────────────────────────────────────────────
  return (
    <EditorView
      t={t}
      audioRef={audioRef}
      activeSegmentId={activeSegmentId}
      correctionItems={correctionItems}
      currentSegmentId={currentSegmentId}
      currentTime={currentTime}
      displayedWaveformPeaks={displayedWaveformPeaks}
      editingSpeaker={editingSpeaker}
      editorStatus={editorStatus}
      handleAcceptCandidate={handleAcceptCandidate}
      handleAddToGlossary={handleAddToGlossary}
      handleRenameSpeaker={handleRenameSpeaker}
      handleSearch={handleSearch}
      handleSearchStep={handleSearchStep}
      handleSpeakerLabelChange={handleSpeakerLabelChange}
      isPlaying={isPlaying}
      loadState={loadState}
      lowConfidenceItems={lowConfidenceItems}
      mediaSrc={mediaSrc}
      saveSegmentChange={saveSegmentChange}
      searchBusy={searchBusy}
      searchIndex={searchIndex}
      searchQuery={searchQuery}
      searchResultIds={searchResultIds}
      searchResults={searchResults}
      seekAudio={seekAudio}
      segments={segments}
      setCurrentSegmentId={setCurrentSegmentId}
      setCurrentTime={setCurrentTime}
      setEditingSpeaker={setEditingSpeaker}
      setIsPlaying={setIsPlaying}
      setSearchIndex={setSearchIndex}
      setSearchQuery={setSearchQuery}
      setSearchResults={setSearchResults}
      setSegments={setSegments}
      setSpeakerEdits={setSpeakerEdits}
      setSpeed={setSpeed}
      speakerCounts={speakerCounts}
      speakerEdits={speakerEdits}
      speakers={speakers}
      speed={speed}
      termActionSegmentId={termActionSegmentId}
      totalDuration={totalDuration}
    />
  );
}
