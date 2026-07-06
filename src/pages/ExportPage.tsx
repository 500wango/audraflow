import { useState } from 'react';
import { save as saveDialog } from '@tauri-apps/plugin-dialog';
import type { JobInfo } from '../types';
import { errorToMessage, invokeTauri } from '../tauri';
import { useI18n } from '../useI18n';

function exportExtension(format: string): string {
  const normalized = format.trim().toLowerCase();
  if (normalized === 'markdown') return 'md';
  return normalized;
}

export function ExportPage({ jobs }: { jobs: JobInfo[] }) {
  const { t } = useI18n();
  const completedJobs = jobs.filter((job) => job.state === 'completed');
  const [selectedJobId, setSelectedJobId] = useState('');
  const [includeTimestamps, setIncludeTimestamps] = useState(true);
  const [speakerFilter, setSpeakerFilter] = useState<'all' | 'namedOnly' | 'hidden'>('all');
  const [includeMarks, setIncludeMarks] = useState(true);
  const [exportStatus, setExportStatus] = useState<string | null>(null);
  const [exportingFormat, setExportingFormat] = useState<string | null>(null);
  const [copyingFormat, setCopyingFormat] = useState<string | null>(null);
  const effectiveJobId = selectedJobId || completedJobs[completedJobs.length - 1]?.jobId || '';
  const includeSpeakers = speakerFilter !== 'hidden';

  const handleExport = async (format: string) => {
    if (!effectiveJobId) {
      setExportStatus(t('export.noCompletedExport'));
      return;
    }

    setExportingFormat(format);
    setExportStatus(null);
    try {
      const extension = exportExtension(format);
      const selectedJob = completedJobs.find((job) => job.jobId === effectiveJobId);
      const targetPath = await saveDialog({
        title: t('export.saveTitle'),
        defaultPath: `${selectedJob?.fileName?.replace(/\.[^.]+$/, '') || `transcript-${effectiveJobId}`}.${extension}`,
        filters: [
          {
            name: format,
            extensions: [extension],
          },
        ],
      });
      if (!targetPath) {
        setExportStatus(t('export.cancelled'));
        return;
      }
      const path = await invokeTauri<string>('cmd_export_transcript', {
        jobId: effectiveJobId,
        format,
        includeSpeakers,
        includeTimestamps,
        includeMarks,
        speakerFilter,
        outputPath: targetPath,
      });
      setExportStatus(t('export.exportedTo', { path }));
    } catch (error) {
      setExportStatus(errorToMessage(error, t));
    } finally {
      setExportingFormat(null);
    }
  };

  const handleCopy = async (format: string) => {
    if (!effectiveJobId) {
      setExportStatus(t('export.noCompletedCopy'));
      return;
    }
    if (!navigator.clipboard?.writeText) {
      setExportStatus(t('export.clipboardUnavailable'));
      return;
    }

    setCopyingFormat(format);
    setExportStatus(null);
    try {
      const content = await invokeTauri<string>('cmd_render_transcript_export', {
        jobId: effectiveJobId,
        format,
        includeSpeakers,
        includeTimestamps,
        includeMarks,
        speakerFilter,
      });
      await navigator.clipboard.writeText(content);
      setExportStatus(t('export.copied', { format }));
    } catch (error) {
      setExportStatus(errorToMessage(error, t));
    } finally {
      setCopyingFormat(null);
    }
  };

  const formats = [
    { format: 'TXT', desc: t('export.descTxt'), enabled: true, exportable: true, copyable: true },
    { format: 'Markdown', desc: t('export.descMarkdown'), enabled: true, exportable: true, copyable: true },
    { format: 'SRT', desc: t('export.descSrt'), enabled: true, exportable: true, copyable: true },
    { format: 'VTT', desc: t('export.descVtt'), enabled: true, exportable: true, copyable: true },
    { format: 'JSON', desc: t('export.descJson'), enabled: true, exportable: true, copyable: true },
    { format: 'DOCX', desc: t('export.descDocx'), enabled: true, exportable: true, copyable: false },
    { format: 'Obsidian', desc: t('export.descObsidian'), enabled: true, exportable: false, copyable: true },
    { format: 'Notion', desc: t('export.descNotion'), enabled: true, exportable: false, copyable: true },
  ];

  return (
    <div className="page export-page">
      <h2>{t('export.title')}</h2>
      <div className="export-toolbar">
        <label>
          {t('export.job')}
          <select
            value={effectiveJobId}
            onChange={(event) => setSelectedJobId(event.target.value)}
            disabled={completedJobs.length === 0}
          >
            {completedJobs.length === 0 ? (
              <option value="">{t('export.noCompletedJobs')}</option>
            ) : (
              completedJobs.map((job) => (
                <option key={job.jobId} value={job.jobId}>
                  {job.fileName}
                </option>
              ))
            )}
          </select>
        </label>
        <label><input type="checkbox" checked={includeTimestamps} onChange={(event) => setIncludeTimestamps(event.target.checked)} /> {t('export.timestamps')}</label>
        <label>
          {t('export.speakers')}
          <select
            value={speakerFilter}
            onChange={(event) => setSpeakerFilter(event.target.value as 'all' | 'namedOnly' | 'hidden')}
          >
            <option value="all">{t('export.speakersAll')}</option>
            <option value="namedOnly">{t('export.speakersNamed')}</option>
            <option value="hidden">{t('export.speakersHidden')}</option>
          </select>
        </label>
        <label><input type="checkbox" checked={includeMarks} onChange={(event) => setIncludeMarks(event.target.checked)} /> {t('export.marks')}</label>
      </div>
      {exportStatus && <p className="export-status">{exportStatus}</p>}
      <div className="export-grid">
        {formats.map(({ format, desc, enabled, exportable, copyable }) => (
          <div key={format} className="export-card">
            <h3>.{format.toLowerCase()}</h3>
            <p>{desc}</p>
            <div className="export-card-actions">
              {exportable && (
                <button
                  className="btn-primary btn-sm"
                  disabled={!enabled || completedJobs.length === 0 || exportingFormat === format}
                  onClick={() => handleExport(format)}
                >
                  {exportingFormat === format ? t('export.exporting') : t('export.export')}
                </button>
              )}
              {copyable && (
                <button
                  className="btn-secondary btn-sm"
                  disabled={!enabled || completedJobs.length === 0 || copyingFormat === format}
                  onClick={() => handleCopy(format)}
                >
                  {copyingFormat === format ? t('export.copying') : t('export.copy')}
                </button>
              )}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
