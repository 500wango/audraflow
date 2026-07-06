import { useCallback, useMemo, useState } from 'react';
import type { JobInfo, QueueAction } from '../types';
import { buildBatchQueueReport, formatPercent, formatRtf, stateLabel } from '../appUtils';
import { errorToMessage } from '../tauri';
import { useI18n } from '../useI18n';

export function QueuePage({
  jobs,
  onImport,
  onJobAction,
  onRefreshJobs,
}: {
  jobs: JobInfo[];
  onImport: () => void;
  onJobAction: (jobId: string, action: QueueAction) => void;
  onRefreshJobs: () => Promise<number>;
}) {
  const { t } = useI18n();
  const [refreshing, setRefreshing] = useState(false);
  const [refreshMessage, setRefreshMessage] = useState<string | null>(null);
  const report = useMemo(() => buildBatchQueueReport(jobs), [jobs]);
  const handleRefresh = useCallback(() => {
    setRefreshing(true);
    setRefreshMessage(null);
    void onRefreshJobs()
      .then((count) => {
        setRefreshMessage(t('queue.refreshed', {
          count,
          plural: count === 1 ? '' : 's',
        }));
      })
      .catch((error) => {
        setRefreshMessage(t('queue.refreshFailed', { error: errorToMessage(error, t) }));
      })
      .finally(() => setRefreshing(false));
  }, [onRefreshJobs, t]);

  if (jobs.length === 0) {
    return (
      <div className="page queue-page">
        <div className="empty-state">
          <h2>{t('queue.emptyTitle')}</h2>
          <p>{t('queue.emptyDescription')}</p>
          <div className="empty-state-actions">
            <button className="btn-primary" type="button" onClick={onImport}>
              {t('queue.importAction')}
            </button>
            <button className="btn-secondary" type="button" disabled={refreshing} onClick={handleRefresh}>
              {refreshing ? t('queue.refreshing') : t('queue.refresh')}
            </button>
          </div>
          {refreshMessage ? <p className="queue-sync-status">{refreshMessage}</p> : null}
        </div>
      </div>
    );
  }

  return (
    <div className="page queue-page">
      <div className="queue-header">
        <h2>{t('queue.title', { count: jobs.length })}</h2>
        <button className="btn-secondary" type="button" disabled={refreshing} onClick={handleRefresh}>
          {refreshing ? t('queue.refreshing') : t('queue.refresh')}
        </button>
      </div>
      <div className="queue-note">
        {t('queue.note')}
      </div>
      {refreshMessage ? <p className="queue-sync-status">{refreshMessage}</p> : null}
      <div className={`batch-report ${report.allDone ? 'batch-report-done' : ''}`}>
        <div className="batch-report-title">
          <span>{report.allDone ? t('queue.batchComplete') : t('queue.batchInProgress')}</span>
          <strong>{t('queue.completedCount', { completed: report.completed, total: report.total })}</strong>
        </div>
        <div className="batch-report-grid">
          <div>
            <span>{t('queue.active')}</span>
            <strong>{report.active}</strong>
          </div>
          <div>
            <span>{t('queue.failed')}</span>
            <strong>{report.failed}</strong>
          </div>
          <div>
            <span>{t('queue.failureRate')}</span>
            <strong>{formatPercent(report.failureRatePct)}</strong>
          </div>
          <div>
            <span>{t('queue.rtf')}</span>
            <strong>{formatRtf(report.endToEndRtf)}</strong>
          </div>
          <div>
            <span>{t('queue.mcmSamples')}</span>
            <strong>{report.mcmSamples}</strong>
          </div>
          <div>
            <span>{t('queue.exportReady')}</span>
            <strong>{report.exportReady}/{report.total}</strong>
          </div>
        </div>
        {report.exportBlocked > 0 && (
          <p className="batch-report-note">
            {t('queue.notExportable', { count: report.exportBlocked })}
          </p>
        )}
      </div>
      <div className="job-list">
        {jobs.map((job) => (
          <div key={job.jobId} className={`job-card job-${job.state}`}>
            <div className="job-info">
              <span className="job-name">{job.fileName}</span>
              <span className="job-duration">{job.activity || job.duration}</span>
              <span className="job-media-meta">
                {[job.duration, job.format, job.size].filter(Boolean).join(' · ')}
              </span>
              <div className="job-progress-row">
                <div className="progress-bar">
                  <div
                    className="progress-fill"
                    style={{ width: `${job.progress}%` }}
                  />
                </div>
                <span className="progress-label">
                  {job.phase || 'queued'} · {Math.round(job.progress)}%
                </span>
              </div>
              {job.logs.length > 0 && (
                <div className="job-log">
                  {job.logs.slice(-8).map((entry) => (
                    <div key={entry.id} className={`job-log-line log-${entry.level}`}>
                      <span className="job-log-time">{entry.time}</span>
                      <span className="job-log-message">{entry.message}</span>
                    </div>
                  ))}
                </div>
              )}
            </div>
            <div className="job-status">
              <span className={`status-badge status-${job.state}`}>
                {stateLabel(job.state, t)}
              </span>
              {job.error && <span className="job-error" title={job.error}>{job.error}</span>}
            </div>
            <div className="job-actions">
              {job.state === 'paused' ? (
                <button onClick={() => onJobAction(job.jobId, 'resume')}>
                  {t('queue.resume')}
                </button>
              ) : (
                <button
                  disabled={['completed', 'cancelled', 'failed', 'notFound'].includes(job.state)}
                  onClick={() => onJobAction(job.jobId, 'pause')}
                >
                  {t('queue.pause')}
                </button>
              )}
              <button
                disabled={!['failed', 'cancelled'].includes(job.state)}
                onClick={() => onJobAction(job.jobId, 'retry')}
              >
                {t('queue.retry')}
              </button>
              <button
                disabled={['completed', 'cancelled', 'notFound'].includes(job.state)}
                onClick={() => onJobAction(job.jobId, 'skip')}
              >
                {t('queue.skip')}
              </button>
              <button
                disabled={['completed', 'cancelled', 'failed', 'notFound'].includes(job.state)}
                onClick={() => onJobAction(job.jobId, 'cancel')}
              >
                {t('queue.cancel')}
              </button>
              <button
                disabled={job.state !== 'completed'}
                onClick={() => onJobAction(job.jobId, 'open')}
              >
                {job.state === 'completed' ? t('queue.openTranscript') : t('queue.open')}
              </button>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
