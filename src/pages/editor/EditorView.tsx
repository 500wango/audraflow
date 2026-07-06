import type { Dispatch, RefObject, SetStateAction } from 'react';
import type { TranscriptSegment } from '../../types';
import { formatTime } from '../../appUtils';
import type { Translate } from '../../tauri';
import {
  lowConfidenceRiskScore,
  reasonLabel,
  riskLabel,
  riskLevel,
  segmentTextDiff,
} from './editorUtils';

interface EditorViewProps {
  t: Translate;
  audioRef: RefObject<HTMLAudioElement | null>;
  activeSegmentId: string | null;
  correctionItems: TranscriptSegment[];
  currentSegmentId: string | null;
  currentTime: number;
  displayedWaveformPeaks: number[];
  editingSpeaker: string | null;
  editorStatus: string | null;
  handleAcceptCandidate: (target?: TranscriptSegment) => void;
  handleAddToGlossary: (target?: TranscriptSegment) => void;
  handleRenameSpeaker: (segmentId: string, newName: string) => void;
  handleSearch: () => Promise<void>;
  handleSearchStep: (direction: 1 | -1) => void;
  handleSpeakerLabelChange: (fromSpeaker: string, toSpeaker: string) => Promise<void>;
  isPlaying: boolean;
  loadState: 'idle' | 'loading' | 'ready' | 'error';
  lowConfidenceItems: TranscriptSegment[];
  mediaSrc: string | null;
  saveSegmentChange: (segmentId: string, patch: { text?: string; speaker?: string }) => Promise<void>;
  searchBusy: boolean;
  searchIndex: number;
  searchQuery: string;
  searchResultIds: Set<string>;
  searchResults: TranscriptSegment[];
  seekAudio: (seconds: number, play?: boolean, trigger?: string, segmentId?: string) => void;
  segments: TranscriptSegment[];
  setCurrentSegmentId: Dispatch<SetStateAction<string | null>>;
  setCurrentTime: Dispatch<SetStateAction<number>>;
  setEditingSpeaker: Dispatch<SetStateAction<string | null>>;
  setIsPlaying: Dispatch<SetStateAction<boolean>>;
  setSearchIndex: Dispatch<SetStateAction<number>>;
  setSearchQuery: Dispatch<SetStateAction<string>>;
  setSearchResults: Dispatch<SetStateAction<TranscriptSegment[]>>;
  setSegments: Dispatch<SetStateAction<TranscriptSegment[]>>;
  setSpeakerEdits: Dispatch<SetStateAction<Record<string, string>>>;
  setSpeed: Dispatch<SetStateAction<number>>;
  speakerCounts: Record<string, number>;
  speakerEdits: Record<string, string>;
  speakers: string[];
  speed: number;
  termActionSegmentId: string | null;
  totalDuration: number;
}

export function EditorView({
  t,
  audioRef,
  activeSegmentId,
  correctionItems,
  currentSegmentId,
  currentTime,
  displayedWaveformPeaks,
  editingSpeaker,
  editorStatus,
  handleAcceptCandidate,
  handleAddToGlossary,
  handleRenameSpeaker,
  handleSearch,
  handleSearchStep,
  handleSpeakerLabelChange,
  isPlaying,
  loadState,
  lowConfidenceItems,
  mediaSrc,
  saveSegmentChange,
  searchBusy,
  searchIndex,
  searchQuery,
  searchResultIds,
  searchResults,
  seekAudio,
  segments,
  setCurrentSegmentId,
  setCurrentTime,
  setEditingSpeaker,
  setIsPlaying,
  setSearchIndex,
  setSearchQuery,
  setSearchResults,
  setSegments,
  setSpeakerEdits,
  setSpeed,
  speakerCounts,
  speakerEdits,
  speakers,
  speed,
  termActionSegmentId,
  totalDuration,
}: EditorViewProps) {
  return (
    <div className="page editor-page">
      <div className="editor-toolbar">
        <div>
          <h2>{t('editor.title')}</h2>
          <p className={`editor-status editor-status-${loadState}`}>
            {editorStatus || t('editor.ready')}
          </p>
        </div>
        <form
          className="editor-search"
          onSubmit={(event) => {
            event.preventDefault();
            void handleSearch();
          }}
        >
          <input
            type="search"
            value={searchQuery}
            onChange={(event) => {
              const value = event.target.value;
              setSearchQuery(value);
              if (!value.trim()) {
                setSearchResults([]);
                setSearchIndex(0);
              }
            }}
            placeholder={t('editor.searchPlaceholder')}
            disabled={segments.length === 0 || searchBusy}
          />
          <button
            type="submit"
            className="btn-secondary btn-sm-inline"
            disabled={segments.length === 0 || searchBusy}
          >
            {searchBusy ? t('editor.searching') : t('editor.search')}
          </button>
          <button
            type="button"
            className="btn-secondary btn-sm-inline"
            disabled={searchResults.length === 0}
            onClick={() => handleSearchStep(-1)}
          >
            {t('editor.prev')}
          </button>
          <button
            type="button"
            className="btn-secondary btn-sm-inline"
            disabled={searchResults.length === 0}
            onClick={() => handleSearchStep(1)}
          >
            {t('editor.next')}
          </button>
          {searchResults.length > 0 ? (
            <span>{searchIndex + 1}/{searchResults.length}</span>
          ) : null}
        </form>
      </div>
      <div className="editor-layout">
        <div className="editor-main" id="transcript-scroll">
          <div className="transcript-area">
            {segments.length === 0 ? (
              <div className="empty-state editor-empty">
                <h2>{loadState === 'loading' ? t('editor.loadingTitle') : t('editor.noTranscript')}</h2>
                <p>{editorStatus || t('editor.openCompleted')}</p>
              </div>
            ) : segments.map((seg) => {
              const isActive = seg.id === activeSegmentId;
              const isEdited = seg.text !== seg.rawText || seg.hasCorrection;
              const isSearchHit = searchResultIds.has(seg.id);

              return (
                <div
                  key={seg.id}
                  id={`seg-${seg.id}`}
                  className={`transcript-segment ${isActive ? 'segment-active' : ''} ${isEdited ? 'segment-edited' : ''} ${isSearchHit ? 'segment-search-hit' : ''}`}
                  onClick={() => {
                    setCurrentSegmentId(seg.id);
                    seekAudio(seg.startMs / 1000, false, 'click_segment', seg.id);
                  }}
                >
                  <div className="segment-meta">
                    <span className="segment-speaker">
                      {editingSpeaker === seg.id ? (
                        <input
                          className="speaker-edit-input"
                          defaultValue={seg.speaker}
                          autoFocus
                          onBlur={(e) => handleRenameSpeaker(seg.id, e.target.value)}
                          onKeyDown={(e) => {
                            if (e.key === 'Enter') handleRenameSpeaker(seg.id, (e.target as HTMLInputElement).value);
                            if (e.key === 'Escape') setEditingSpeaker(null);
                          }}
                        />
                      ) : (
                        <span
                          className="speaker-badge"
                          onClick={(e) => { e.stopPropagation(); setEditingSpeaker(seg.id); }}
                          title={t('editor.clickRename')}
                        >
                          {seg.speaker}
                        </span>
                      )}
                    </span>
                    <span
                      className="segment-time"
                      onClick={(e) => {
                        e.stopPropagation();
                        setCurrentSegmentId(seg.id);
                        seekAudio(seg.startMs / 1000, true, 'click_segment', seg.id);
                      }}
                      title={t('editor.clickJump')}
                    >
                      {formatTime(seg.startMs)}
                    </span>
                    {seg.hasMark && (
                      <span className="segment-mark" title={t('editor.markTitle')}>
                        {t('editor.mark')} {seg.marks.length > 1 ? seg.marks.length : ''}
                      </span>
                    )}
                    {seg.confidence < 0.8 && (
                      <span className="segment-low-conf" title={t('editor.confidence', { percent: (seg.confidence * 100).toFixed(0) })}>{t('editor.low')}</span>
                    )}
                  </div>
                  <div className="segment-text" onClick={(event) => event.stopPropagation()}>
                    <textarea
                      value={seg.text}
                      onChange={(event) => {
                        const nextText = event.target.value;
                        setSegments((prev) => prev.map((item) =>
                          item.id === seg.id ? { ...item, text: nextText } : item
                        ));
                      }}
                      onFocus={() => setCurrentSegmentId(seg.id)}
                      onBlur={(event) => {
                        void saveSegmentChange(seg.id, { text: event.target.value });
                      }}
                      rows={Math.max(2, Math.ceil(seg.text.length / 56))}
                    />
                    {seg.hasCorrection && (
                      <span className="correction-indicator" title={t('editor.hasCorrections')}>{t('editor.edited')}</span>
                    )}
                    {seg.marks.length > 0 && (
                      <div className="segment-marks">
                        {seg.marks.map((mark) => (
                          <button
                            key={mark.id}
                            type="button"
                            onClick={() => seekAudio(mark.markMs / 1000, true, 'replay', seg.id)}
                          >
                            {formatTime(mark.markMs)}
                            {mark.label ? ` ${mark.label}` : ''}
                          </button>
                        ))}
                      </div>
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        </div>

        <aside className="editor-sidebar">
          <div className="sidebar-section">
            <h3>{t('editor.lowConfidence', { count: lowConfidenceItems.length })}</h3>
            {lowConfidenceItems.length === 0 ? (
              <p className="placeholder-text">{t('editor.noItems')}</p>
            ) : (
              <ul className="sidebar-list">
                {lowConfidenceItems.map(item => (
                  <li
                    key={item.id}
                    className="sidebar-item"
                    onClick={() => {
                      setCurrentSegmentId(item.id);
                      seekAudio(item.startMs / 1000, false, 'click_segment', item.id);
                    }}
                  >
                    <span className="item-time">{formatTime(item.startMs)}</span>
                    <span className={`risk-badge risk-${riskLevel(lowConfidenceRiskScore(item))}`}>
                      {riskLabel(riskLevel(lowConfidenceRiskScore(item)), t)}
                    </span>
                    <span className="item-reason">
                      {item.lowConfidenceReasons.map((reason) => reasonLabel(reason, t)).join(', ')}
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </div>

          <div className="sidebar-section">
            <h3>{t('editor.termCandidates', { count: correctionItems.length })}</h3>
            {correctionItems.length === 0 ? (
              <p className="placeholder-text">{t('editor.noCandidates')}</p>
            ) : (
              <ul className="sidebar-list">
                {correctionItems.map(item => {
                  const diff = segmentTextDiff(item);
                  const busy = termActionSegmentId === item.id;
                  return (
                    <li
                      key={item.id}
                      className="sidebar-item candidate-item"
                      onClick={() => {
                        setCurrentSegmentId(item.id);
                        seekAudio(item.startMs / 1000, false, 'click_segment', item.id);
                      }}
                    >
                      <div className="candidate-summary">
                        <span className="item-time">{formatTime(item.startMs)}</span>
                        <span className="item-diff">
                          <span className="diff-old">{(diff?.original ?? item.rawText).slice(0, 15)}</span>
                          {' -> '}
                          <span className="diff-new">{(diff?.replacement ?? item.text).slice(0, 15)}</span>
                        </span>
                      </div>
                      <div className="candidate-actions">
                        <button
                          type="button"
                          disabled={busy}
                          onClick={(event) => {
                            event.stopPropagation();
                            handleAcceptCandidate(item);
                          }}
                        >
                          {busy ? t('editor.accepting') : t('editor.acceptCandidate')}
                        </button>
                        <button
                          type="button"
                          disabled={busy || !diff}
                          onClick={(event) => {
                            event.stopPropagation();
                            handleAddToGlossary(item);
                          }}
                        >
                          {busy ? t('editor.addingGlossary') : t('editor.addToGlossary')}
                        </button>
                      </div>
                    </li>
                  );
                })}
              </ul>
            )}
          </div>

          <div className="sidebar-section">
            <h3>{t('editor.speakers', { count: speakers.length })}</h3>
            <ul className="sidebar-list">
              {speakers.map(sp => (
                <li key={sp} className="sidebar-item speaker-item">
                  <div className="speaker-row">
                    <span className="speaker-badge">{sp}</span>
                    <span className="speaker-count">{t('editor.segmentCount', { count: speakerCounts[sp] })}</span>
                  </div>
                  <div className="speaker-actions">
                    <input
                      className="speaker-label-input"
                      value={speakerEdits[sp] ?? sp}
                      onChange={(event) => setSpeakerEdits((prev) => ({
                        ...prev,
                        [sp]: event.target.value,
                      }))}
                      onKeyDown={(event) => {
                        if (event.key === 'Enter') {
                          void handleSpeakerLabelChange(sp, speakerEdits[sp] ?? sp);
                        }
                      }}
                    />
                    <button
                      type="button"
                      onClick={() => void handleSpeakerLabelChange(sp, speakerEdits[sp] ?? sp)}
                      disabled={(speakerEdits[sp] ?? sp).trim() === sp}
                    >
                      {t('editor.rename')}
                    </button>
                  </div>
                  {speakers.length > 1 && (
                    <select
                      className="speaker-merge-select"
                      value=""
                      onChange={(event) => {
                        if (event.target.value) {
                          void handleSpeakerLabelChange(sp, event.target.value);
                        }
                      }}
                    >
                      <option value="" disabled>{t('editor.merge')}</option>
                      {speakers.filter((target) => target !== sp).map((target) => (
                        <option key={target} value={target}>{target}</option>
                      ))}
                    </select>
                  )}
                </li>
              ))}
            </ul>
          </div>
        </aside>
      </div>

      <div className="playback-bar">
        <button
          title={isPlaying ? t('editor.pauseShortcut') : t('editor.playShortcut')}
          disabled={segments.length === 0}
          onClick={() => setIsPlaying(p => !p)}
        >
          {isPlaying ? t('editor.pause') : t('editor.play')}
        </button>
        {mediaSrc ? (
          <audio
            ref={audioRef}
            src={mediaSrc}
            preload="metadata"
            onTimeUpdate={(event) => setCurrentTime(event.currentTarget.currentTime)}
            onPlay={() => setIsPlaying(true)}
            onPause={() => setIsPlaying(false)}
            onEnded={() => setIsPlaying(false)}
            onLoadedMetadata={(event) => {
              event.currentTarget.playbackRate = speed;
            }}
          />
        ) : null}
        <div
          className="waveform-placeholder"
          onClick={(event) => {
            const rect = event.currentTarget.getBoundingClientRect();
            const ratio = rect.width > 0 ? (event.clientX - rect.left) / rect.width : 0;
            seekAudio(totalDuration * ratio, isPlaying, 'manual_scrub', currentSegmentId ?? undefined);
          }}
        >
          <div className="waveform-bars" aria-hidden="true">
            {displayedWaveformPeaks.map((peak, index) => (
              <span
                key={index}
                style={{ height: `${Math.round(Math.max(0.08, peak) * 100)}%` }}
              />
            ))}
          </div>
          <div
            className="waveform-progress"
            style={{ width: `${totalDuration > 0 ? (currentTime / totalDuration) * 100 : 0}%` }}
          />
        </div>
        <span className="time-display">
          {formatTime(Math.floor(currentTime * 1000))} / {formatTime(Math.floor(totalDuration * 1000))}
        </span>
        <select
          value={speed}
          onChange={(e) => setSpeed(parseFloat(e.target.value))}
        >
          <option value={0.5}>0.5x</option>
          <option value={1}>1x</option>
          <option value={1.5}>1.5x</option>
        </select>
        <span className="shortcuts-hint">
          {t('editor.shortcuts')}
        </span>
      </div>
    </div>
  );
}
