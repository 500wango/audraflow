//! AudraFlow Storage Layer
//!
//! SQLite + FTS5 persistence implementing the unified Transcript Schema (PRD §13.2).
//! All proofreading, low-confidence queues, export, and telemetry read from this schema.
//!
//! Design rules:
//! - Large objects (audio paths) do NOT go in the main table.
//! - All corrections are tracked as diffs.
//! - FTS5 index enables fast full-text search of transcript text.
//! - Supports local history clearing.

use audraflow_ipc::{CorrectionSource, Segment};
use rusqlite::{params, Connection, Result as SqliteResult, Row};
use std::path::Path;

// ── Schema Version ─────────────────────────────────────────────────────────

#[allow(dead_code)]
const SCHEMA_VERSION: u32 = 2;

// ── Database ───────────────────────────────────────────────────────────────

pub struct Storage {
    conn: Connection,
}

impl Storage {
    /// Open (or create) the database at the given path and run migrations.
    pub fn open(db_path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(db_path)?;

        // Enable WAL mode for concurrent reads
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;

        let storage = Self { conn };
        storage.run_migrations()?;
        Ok(storage)
    }

    /// Open an in-memory database for testing.
    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        let storage = Self { conn };
        storage.run_migrations()?;
        Ok(storage)
    }

    fn run_migrations(&self) -> anyhow::Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER PRIMARY KEY
            );",
        )?;

        let current: u32 = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if current < 1 {
            self.migrate_v1()?;
        }
        if current < 2 {
            self.migrate_v2()?;
        }

        Ok(())
    }

    fn migrate_v1(&self) -> anyhow::Result<()> {
        self.conn.execute_batch(
            "
            -- ── Jobs ──────────────────────────────────────────────────────────
            CREATE TABLE IF NOT EXISTS jobs (
                job_id          TEXT PRIMARY KEY NOT NULL,
                file_path       TEXT NOT NULL,
                file_hash       TEXT NOT NULL,
                audio_duration_s REAL,
                sample_rate     INTEGER,
                channels        INTEGER,
                state           TEXT NOT NULL DEFAULT 'pending',
                extreme_accuracy INTEGER NOT NULL DEFAULT 0,
                created_at      TEXT NOT NULL DEFAULT (datetime('now')),
                completed_at    TEXT
            );

            -- ── Transcript segments (unified schema) ────────────────────────
            CREATE TABLE IF NOT EXISTS segments (
                segment_id      TEXT PRIMARY KEY NOT NULL,
                job_id          TEXT NOT NULL REFERENCES jobs(job_id) ON DELETE CASCADE,
                start_ms        INTEGER NOT NULL,
                end_ms          INTEGER NOT NULL,
                speaker_id      TEXT,
                text            TEXT NOT NULL DEFAULT '',
                raw_text        TEXT NOT NULL DEFAULT '',
                confidence      REAL NOT NULL DEFAULT 0.0,
                sort_order      INTEGER NOT NULL DEFAULT 0
            );

            -- ── Low-confidence reasons (1:N with segments) ──────────────────
            CREATE TABLE IF NOT EXISTS low_confidence_reasons (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                segment_id      TEXT NOT NULL REFERENCES segments(segment_id) ON DELETE CASCADE,
                reason          TEXT NOT NULL
            );

            -- ── Corrections (diff log) ──────────────────────────────────────
            CREATE TABLE IF NOT EXISTS corrections (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                segment_id      TEXT NOT NULL REFERENCES segments(segment_id) ON DELETE CASCADE,
                field           TEXT NOT NULL,
                old_value       TEXT NOT NULL,
                new_value       TEXT NOT NULL,
                source          TEXT NOT NULL DEFAULT 'user',
                auto_applied    INTEGER NOT NULL DEFAULT 0,
                created_at      TEXT NOT NULL DEFAULT (datetime('now'))
            );

            -- ── Timestamp marks (Ctrl+T) ────────────────────────────────────
            CREATE TABLE IF NOT EXISTS marks (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                segment_id      TEXT NOT NULL REFERENCES segments(segment_id) ON DELETE CASCADE,
                mark_ms         INTEGER NOT NULL,
                label           TEXT,
                note            TEXT,
                created_at      TEXT NOT NULL DEFAULT (datetime('now'))
            );

            -- ── Glossary / Lexicon ──────────────────────────────────────────
            CREATE TABLE IF NOT EXISTS glossary (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                canonical       TEXT NOT NULL,
                category        TEXT,
                enabled         INTEGER NOT NULL DEFAULT 1,
                created_at      TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS glossary_aliases (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                glossary_id     INTEGER NOT NULL REFERENCES glossary(id) ON DELETE CASCADE,
                alias           TEXT NOT NULL,
                pinyin          TEXT
            );

            -- ── Metrics / Telemetry (local only, no user content) ──────────
            CREATE TABLE IF NOT EXISTS metrics_sessions (
                session_id      TEXT PRIMARY KEY NOT NULL,
                job_id          TEXT NOT NULL REFERENCES jobs(job_id),
                start_time      TEXT NOT NULL,
                end_time        TEXT,
                active_seconds  REAL DEFAULT 0,
                inactive_seconds REAL DEFAULT 0,
                completed_ratio REAL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS metrics_events (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id      TEXT NOT NULL REFERENCES metrics_sessions(session_id),
                event_type      TEXT NOT NULL,
                event_data      TEXT NOT NULL,     -- JSON, no audio/text content
                recorded_at     TEXT NOT NULL DEFAULT (datetime('now'))
            );

            -- ── Checkpoints for task recovery ───────────────────────────────
            CREATE TABLE IF NOT EXISTS checkpoints (
                checkpoint_id   TEXT PRIMARY KEY NOT NULL,
                job_id          TEXT NOT NULL REFERENCES jobs(job_id),
                last_segment_id TEXT NOT NULL,
                state_blob      BLOB NOT NULL,
                created_at      TEXT NOT NULL DEFAULT (datetime('now'))
            );

            -- ── FTS5 full-text index on transcript text ────────────────────
            CREATE VIRTUAL TABLE IF NOT EXISTS segments_fts USING fts5(
                segment_id,
                text,
                content='segments',
                content_rowid='rowid'
            );

            -- Triggers to keep FTS index in sync
            CREATE TRIGGER IF NOT EXISTS segments_ai AFTER INSERT ON segments BEGIN
                INSERT INTO segments_fts(rowid, segment_id, text)
                VALUES (new.rowid, new.segment_id, new.text);
            END;

            CREATE TRIGGER IF NOT EXISTS segments_ad AFTER DELETE ON segments BEGIN
                INSERT INTO segments_fts(segments_fts, rowid, segment_id, text)
                VALUES ('delete', old.rowid, old.segment_id, old.text);
            END;

            CREATE TRIGGER IF NOT EXISTS segments_au AFTER UPDATE ON segments BEGIN
                INSERT INTO segments_fts(segments_fts, rowid, segment_id, text)
                VALUES ('delete', old.rowid, old.segment_id, old.text);
                INSERT INTO segments_fts(rowid, segment_id, text)
                VALUES (new.rowid, new.segment_id, new.text);
            END;

            -- Schema version marker
            INSERT OR IGNORE INTO schema_version (version) VALUES (1);
            ",
        )?;
        Ok(())
    }

    fn migrate_v2(&self) -> anyhow::Result<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS marks (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                segment_id      TEXT NOT NULL REFERENCES segments(segment_id) ON DELETE CASCADE,
                mark_ms         INTEGER NOT NULL,
                label           TEXT,
                note            TEXT,
                created_at      TEXT NOT NULL DEFAULT (datetime('now'))
            );

            INSERT OR IGNORE INTO schema_version (version) VALUES (2);
            ",
        )?;
        Ok(())
    }

    // ── Job Operations ──────────────────────────────────────────────────────

    /// Create a new job record.
    pub fn create_job(
        &self,
        job_id: &str,
        file_path: &str,
        file_hash: &str,
        extreme_accuracy: bool,
    ) -> SqliteResult<()> {
        self.conn.execute(
            "INSERT INTO jobs (job_id, file_path, file_hash, state, extreme_accuracy)
             VALUES (?1, ?2, ?3, 'pending', ?4)",
            params![job_id, file_path, file_hash, extreme_accuracy as i32],
        )?;
        Ok(())
    }

    /// Update job state.
    pub fn update_job_state(&self, job_id: &str, state: &str) -> SqliteResult<()> {
        self.conn.execute(
            "UPDATE jobs SET state = ?1 WHERE job_id = ?2",
            params![state, job_id],
        )?;
        Ok(())
    }

    /// Mark job as completed.
    pub fn complete_job(&self, job_id: &str) -> SqliteResult<()> {
        self.conn.execute(
            "UPDATE jobs SET state = 'completed', completed_at = datetime('now')
             WHERE job_id = ?1",
            params![job_id],
        )?;
        Ok(())
    }

    /// Get a single job record.
    pub fn get_job(&self, job_id: &str) -> SqliteResult<Option<JobRow>> {
        self.conn
            .query_row(
                "SELECT job_id, file_path, file_hash, audio_duration_s, sample_rate,
                        channels, state, extreme_accuracy, created_at, completed_at
                 FROM jobs
                 WHERE job_id = ?1",
                params![job_id],
                job_row_from_sql,
            )
            .optional()
    }

    /// List recent job records, newest first.
    pub fn list_jobs(&self, limit: usize) -> SqliteResult<Vec<JobRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT job_id, file_path, file_hash, audio_duration_s, sample_rate,
                    channels, state, extreme_accuracy, created_at, completed_at
             FROM jobs
             ORDER BY datetime(created_at) DESC, job_id DESC
             LIMIT ?1",
        )?;

        let rows = stmt.query_map(params![limit.max(1) as i64], job_row_from_sql)?;
        rows.collect()
    }

    // ── Segment Operations ──────────────────────────────────────────────────

    /// Count segments for a job (avoids loading all segments).
    pub fn segment_count(&self, job_id: &str) -> SqliteResult<u32> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM segments WHERE job_id = ?1",
                params![job_id],
                |row| row.get(0),
            )
    }

    /// Get the maximum end_ms for segments in a job (for computing duration).
    pub fn max_segment_end_ms(&self, job_id: &str) -> SqliteResult<Option<i64>> {
        self.conn
            .query_row(
                "SELECT MAX(end_ms) FROM segments WHERE job_id = ?1",
                params![job_id],
                |row| row.get(0),
            )
            .optional()
    }

    /// Insert a batch of segments.
    pub fn insert_segments(&self, job_id: &str, segments: &[Segment]) -> SqliteResult<()> {
        let mut stmt = self.conn.prepare(
            "INSERT OR REPLACE INTO segments
             (segment_id, job_id, start_ms, end_ms, speaker_id, text, raw_text, confidence, sort_order)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"
        )?;

        for (i, seg) in segments.iter().enumerate() {
            stmt.execute(params![
                seg.segment_id,
                job_id,
                seg.start_ms,
                seg.end_ms,
                seg.speaker_id,
                seg.text,
                seg.raw_text,
                seg.confidence,
                i as i32,
            ])?;

            // Insert low-confidence reasons
            for reason in &seg.low_confidence_reasons {
                self.conn.execute(
                    "INSERT INTO low_confidence_reasons (segment_id, reason) VALUES (?1, ?2)",
                    params![seg.segment_id, reason],
                )?;
            }

            // Insert correction records produced by post-processing.
            for correction in &seg.corrections {
                self.record_correction(
                    &seg.segment_id,
                    &correction.field,
                    &correction.old_value,
                    &correction.new_value,
                    &correction.source,
                    correction.auto_applied,
                )?;
            }

            // Insert marks
            for mark in &seg.marks {
                self.conn.execute(
                    "INSERT INTO marks (segment_id, mark_ms, label, note) VALUES (?1, ?2, ?3, ?4)",
                    params![seg.segment_id, mark.mark_ms, mark.label, mark.note],
                )?;
            }
        }

        Ok(())
    }

    /// Update a single segment's editable fields.
    pub fn update_segment(
        &self,
        segment_id: &str,
        text: Option<&str>,
        speaker_id: Option<&str>,
    ) -> SqliteResult<()> {
        if let Some(t) = text {
            self.conn.execute(
                "UPDATE segments SET text = ?1 WHERE segment_id = ?2",
                params![t, segment_id],
            )?;
        }
        if let Some(s) = speaker_id {
            self.conn.execute(
                "UPDATE segments SET speaker_id = ?1 WHERE segment_id = ?2",
                params![s, segment_id],
            )?;
        }
        Ok(())
    }

    /// Update every segment for a speaker label within one job.
    pub fn update_speaker_label_for_job(
        &self,
        job_id: &str,
        from_speaker: &str,
        to_speaker: &str,
    ) -> SqliteResult<usize> {
        self.conn.execute(
            "UPDATE segments
             SET speaker_id = ?1
             WHERE job_id = ?2 AND COALESCE(speaker_id, 'Speaker') = ?3",
            params![to_speaker, job_id, from_speaker],
        )
    }

    /// Get a single transcript segment.
    pub fn get_segment(&self, segment_id: &str) -> SqliteResult<Option<SegmentRow>> {
        self.conn
            .query_row(
                "SELECT segment_id, start_ms, end_ms, speaker_id, text, raw_text, confidence
                 FROM segments
                 WHERE segment_id = ?1",
                params![segment_id],
                |row| {
                    Ok(SegmentRow {
                        segment_id: row.get(0)?,
                        start_ms: row.get(1)?,
                        end_ms: row.get(2)?,
                        speaker_id: row.get(3)?,
                        text: row.get(4)?,
                        raw_text: row.get(5)?,
                        confidence: row.get(6)?,
                    })
                },
            )
            .optional()
    }

    /// Get all segments for a job, ordered by start_ms.
    pub fn get_segments(&self, job_id: &str) -> SqliteResult<Vec<SegmentRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT segment_id, start_ms, end_ms, speaker_id, text, raw_text, confidence
             FROM segments
             WHERE job_id = ?1
             ORDER BY start_ms ASC",
        )?;

        let rows = stmt.query_map(params![job_id], |row| {
            Ok(SegmentRow {
                segment_id: row.get(0)?,
                start_ms: row.get(1)?,
                end_ms: row.get(2)?,
                speaker_id: row.get(3)?,
                text: row.get(4)?,
                raw_text: row.get(5)?,
                confidence: row.get(6)?,
            })
        })?;

        rows.collect()
    }

    /// Get low-confidence reasons attached to a segment.
    pub fn get_low_confidence_reasons(&self, segment_id: &str) -> SqliteResult<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT reason
             FROM low_confidence_reasons
             WHERE segment_id = ?1
             ORDER BY id ASC",
        )?;

        let rows = stmt.query_map(params![segment_id], |row| row.get(0))?;
        rows.collect()
    }

    /// Return whether a segment has user or automatic correction records.
    pub fn segment_has_corrections(&self, segment_id: &str) -> SqliteResult<bool> {
        self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM corrections WHERE segment_id = ?1)",
            params![segment_id],
            |row| row.get::<_, i64>(0).map(|value| value != 0),
        )
    }

    /// Return whether a segment has timestamp marks.
    pub fn segment_has_marks(&self, segment_id: &str) -> SqliteResult<bool> {
        self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM marks WHERE segment_id = ?1)",
            params![segment_id],
            |row| row.get::<_, i64>(0).map(|value| value != 0),
        )
    }

    /// Add a timestamp mark to a segment.
    pub fn add_mark(
        &self,
        segment_id: &str,
        mark_ms: i64,
        label: Option<&str>,
        note: Option<&str>,
    ) -> SqliteResult<MarkRow> {
        self.conn.execute(
            "INSERT INTO marks (segment_id, mark_ms, label, note)
             VALUES (?1, ?2, ?3, ?4)",
            params![segment_id, mark_ms, label, note],
        )?;
        let id = self.conn.last_insert_rowid();
        Ok(MarkRow {
            id,
            segment_id: segment_id.to_string(),
            mark_ms,
            label: label.map(ToOwned::to_owned),
            note: note.map(ToOwned::to_owned),
        })
    }

    /// Get timestamp marks for a segment.
    pub fn get_marks(&self, segment_id: &str) -> SqliteResult<Vec<MarkRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, segment_id, mark_ms, label, note
             FROM marks
             WHERE segment_id = ?1
             ORDER BY mark_ms ASC, id ASC",
        )?;

        let rows = stmt.query_map(params![segment_id], |row| {
            Ok(MarkRow {
                id: row.get(0)?,
                segment_id: row.get(1)?,
                mark_ms: row.get(2)?,
                label: row.get(3)?,
                note: row.get(4)?,
            })
        })?;

        rows.collect()
    }

    /// Remove one low-confidence reason from a segment.
    pub fn remove_low_confidence_reason(
        &self,
        segment_id: &str,
        reason: &str,
    ) -> SqliteResult<usize> {
        self.conn.execute(
            "DELETE FROM low_confidence_reasons
             WHERE segment_id = ?1 AND reason = ?2",
            params![segment_id, reason],
        )
    }

    /// Full-text search across transcripts.
    pub fn search_segments(&self, job_id: &str, query: &str) -> SqliteResult<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.segment_id
             FROM segments_fts f
             JOIN segments s ON f.rowid = s.rowid
             WHERE s.job_id = ?1 AND segments_fts MATCH ?2
             ORDER BY rank",
        )?;

        let rows = stmt.query_map(params![job_id, query], |row| row.get(0))?;
        rows.collect()
    }

    // ── Correction Operations ───────────────────────────────────────────────

    /// Record a correction (diff).
    pub fn record_correction(
        &self,
        segment_id: &str,
        field: &str,
        old_value: &str,
        new_value: &str,
        source: &CorrectionSource,
        auto_applied: bool,
    ) -> SqliteResult<()> {
        self.conn.execute(
            "INSERT INTO corrections (segment_id, field, old_value, new_value, source, auto_applied)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                segment_id,
                field,
                old_value,
                new_value,
                source_to_str(source),
                auto_applied as i32,
            ],
        )?;
        Ok(())
    }

    /// Get all corrections for a segment.
    pub fn get_corrections(&self, segment_id: &str) -> SqliteResult<Vec<CorrectionRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT field, old_value, new_value, source, auto_applied, created_at
             FROM corrections
             WHERE segment_id = ?1
             ORDER BY id ASC",
        )?;

        let rows = stmt.query_map(params![segment_id], |row| {
            Ok(CorrectionRecord {
                field: row.get(0)?,
                old_value: row.get(1)?,
                new_value: row.get(2)?,
                source: row.get(3)?,
                auto_applied: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;

        rows.collect()
    }

    // ── Glossary Operations ───────────────────────────────────────────────────

    /// Add or update a glossary entry and attach any new aliases.
    pub fn upsert_glossary_entry(
        &self,
        canonical: &str,
        aliases: &[String],
        category: Option<&str>,
    ) -> SqliteResult<GlossaryEntryRow> {
        let canonical = canonical.trim();
        let category = category.map(str::trim).filter(|value| !value.is_empty());
        let existing_id = self
            .conn
            .query_row(
                "SELECT id
                 FROM glossary
                 WHERE canonical = ?1 AND COALESCE(category, '') = COALESCE(?2, '')
                 ORDER BY id ASC
                 LIMIT 1",
                params![canonical, category],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;

        let id = if let Some(id) = existing_id {
            self.conn
                .execute("UPDATE glossary SET enabled = 1 WHERE id = ?1", params![id])?;
            id
        } else {
            self.conn.execute(
                "INSERT INTO glossary (canonical, category, enabled)
                 VALUES (?1, ?2, 1)",
                params![canonical, category],
            )?;
            self.conn.last_insert_rowid()
        };

        for alias in aliases {
            let alias = alias.trim();
            if alias.is_empty() || alias == canonical {
                continue;
            }
            let exists = self.conn.query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM glossary_aliases
                   WHERE glossary_id = ?1 AND alias = ?2
                 )",
                params![id, alias],
                |row| row.get::<_, i64>(0).map(|value| value != 0),
            )?;
            if !exists {
                self.conn.execute(
                    "INSERT INTO glossary_aliases (glossary_id, alias)
                     VALUES (?1, ?2)",
                    params![id, alias],
                )?;
            }
        }

        self.get_glossary_entry(id)?
            .ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    /// Replace an existing glossary entry and its aliases.
    pub fn replace_glossary_entry(
        &self,
        id: i64,
        canonical: &str,
        aliases: &[String],
        category: Option<&str>,
    ) -> SqliteResult<Option<GlossaryEntryRow>> {
        let canonical = canonical.trim();
        let category = category.map(str::trim).filter(|value| !value.is_empty());
        let changed = self.conn.execute(
            "UPDATE glossary
             SET canonical = ?1, category = ?2, enabled = 1
             WHERE id = ?3",
            params![canonical, category, id],
        )?;
        if changed == 0 {
            return Ok(None);
        }

        self.conn.execute(
            "DELETE FROM glossary_aliases WHERE glossary_id = ?1",
            params![id],
        )?;
        for alias in aliases {
            let alias = alias.trim();
            if alias.is_empty() || alias == canonical {
                continue;
            }
            self.conn.execute(
                "INSERT INTO glossary_aliases (glossary_id, alias)
                 VALUES (?1, ?2)",
                params![id, alias],
            )?;
        }

        self.get_glossary_entry(id)
    }

    /// Get one glossary entry with aliases.
    pub fn get_glossary_entry(&self, id: i64) -> SqliteResult<Option<GlossaryEntryRow>> {
        let entry = self
            .conn
            .query_row(
                "SELECT id, canonical, category, enabled, created_at
                 FROM glossary
                 WHERE id = ?1",
                params![id],
                glossary_entry_from_sql,
            )
            .optional()?;

        match entry {
            Some(mut entry) => {
                entry.aliases = self.get_glossary_aliases(entry.id)?;
                Ok(Some(entry))
            }
            None => Ok(None),
        }
    }

    /// List enabled glossary entries with aliases.
    pub fn list_glossary_entries(&self) -> SqliteResult<Vec<GlossaryEntryRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, canonical, category, enabled, created_at
             FROM glossary
             WHERE enabled = 1
             ORDER BY canonical ASC, id ASC",
        )?;
        let rows = stmt.query_map([], glossary_entry_from_sql)?;
        let mut entries = rows.collect::<SqliteResult<Vec<_>>>()?;

        for entry in &mut entries {
            entry.aliases = self.get_glossary_aliases(entry.id)?;
        }

        Ok(entries)
    }

    /// Soft-delete a glossary entry so existing audit history is preserved.
    pub fn disable_glossary_entry(&self, id: i64) -> SqliteResult<bool> {
        let changed = self.conn.execute(
            "UPDATE glossary SET enabled = 0 WHERE id = ?1 AND enabled = 1",
            params![id],
        )?;
        Ok(changed > 0)
    }

    fn get_glossary_aliases(&self, glossary_id: i64) -> SqliteResult<Vec<GlossaryAliasRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, glossary_id, alias, pinyin
             FROM glossary_aliases
             WHERE glossary_id = ?1
             ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![glossary_id], |row| {
            Ok(GlossaryAliasRow {
                id: row.get(0)?,
                glossary_id: row.get(1)?,
                alias: row.get(2)?,
                pinyin: row.get(3)?,
            })
        })?;
        rows.collect()
    }

    // ── Checkpoint Operations ───────────────────────────────────────────────

    /// Save a checkpoint snapshot.
    pub fn save_checkpoint(
        &self,
        checkpoint_id: &str,
        job_id: &str,
        last_segment_id: &str,
        state_blob: &[u8],
    ) -> SqliteResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO checkpoints (checkpoint_id, job_id, last_segment_id, state_blob)
             VALUES (?1, ?2, ?3, ?4)",
            params![checkpoint_id, job_id, last_segment_id, state_blob],
        )?;
        Ok(())
    }

    /// Get the latest checkpoint for a job.
    pub fn get_latest_checkpoint(&self, job_id: &str) -> SqliteResult<Option<CheckpointRecord>> {
        self.conn
            .query_row(
                "SELECT checkpoint_id, last_segment_id, state_blob
                 FROM checkpoints
                 WHERE job_id = ?1
                 ORDER BY created_at DESC
                 LIMIT 1",
                params![job_id],
                |row| {
                    Ok(CheckpointRecord {
                        checkpoint_id: row.get(0)?,
                        last_segment_id: row.get(1)?,
                        state_blob: row.get(2)?,
                    })
                },
            )
            .optional()
    }

    // ── Utility ─────────────────────────────────────────────────────────────

    /// Clear all local history for a job.
    pub fn clear_job_history(&self) -> SqliteResult<()> {
        self.conn.execute_batch(
            "DELETE FROM metrics_events;
             DELETE FROM metrics_sessions;
             DELETE FROM corrections;
             DELETE FROM marks;
             DELETE FROM low_confidence_reasons;
             DELETE FROM segments_fts;
             DELETE FROM segments;
             DELETE FROM checkpoints;
             DELETE FROM jobs;",
        )?;
        Ok(())
    }

    /// Get the storage size estimate.
    pub fn db_size_bytes(&self) -> SqliteResult<i64> {
        self.conn.query_row(
            "SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()",
            [],
            |row| row.get(0),
        )
    }

    /// Trigger FTS index rebuild (after bulk operations).
    pub fn rebuild_fts(&self) -> SqliteResult<()> {
        self.conn.execute(
            "INSERT INTO segments_fts(segments_fts) VALUES ('rebuild')",
            [],
        )?;
        Ok(())
    }
}

// ── Row types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct JobRow {
    pub job_id: String,
    pub file_path: String,
    pub file_hash: String,
    pub audio_duration_s: Option<f64>,
    pub sample_rate: Option<i64>,
    pub channels: Option<i64>,
    pub state: String,
    pub extreme_accuracy: bool,
    pub created_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SegmentRow {
    pub segment_id: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub speaker_id: Option<String>,
    pub text: String,
    pub raw_text: String,
    pub confidence: f64,
}

#[derive(Debug, Clone)]
pub struct MarkRow {
    pub id: i64,
    pub segment_id: String,
    pub mark_ms: i64,
    pub label: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CorrectionRecord {
    pub field: String,
    pub old_value: String,
    pub new_value: String,
    pub source: String,
    pub auto_applied: bool,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct GlossaryEntryRow {
    pub id: i64,
    pub canonical: String,
    pub category: Option<String>,
    pub enabled: bool,
    pub created_at: String,
    pub aliases: Vec<GlossaryAliasRow>,
}

#[derive(Debug, Clone)]
pub struct GlossaryAliasRow {
    pub id: i64,
    pub glossary_id: i64,
    pub alias: String,
    pub pinyin: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CheckpointRecord {
    pub checkpoint_id: String,
    pub last_segment_id: String,
    pub state_blob: Vec<u8>,
}

fn job_row_from_sql(row: &Row<'_>) -> SqliteResult<JobRow> {
    Ok(JobRow {
        job_id: row.get(0)?,
        file_path: row.get(1)?,
        file_hash: row.get(2)?,
        audio_duration_s: row.get(3)?,
        sample_rate: row.get(4)?,
        channels: row.get(5)?,
        state: row.get(6)?,
        extreme_accuracy: row.get::<_, i32>(7)? != 0,
        created_at: row.get(8)?,
        completed_at: row.get(9)?,
    })
}

fn glossary_entry_from_sql(row: &Row<'_>) -> SqliteResult<GlossaryEntryRow> {
    Ok(GlossaryEntryRow {
        id: row.get(0)?,
        canonical: row.get(1)?,
        category: row.get(2)?,
        enabled: row.get::<_, i32>(3)? != 0,
        created_at: row.get(4)?,
        aliases: Vec::new(),
    })
}

// ── Trait for optional query rows ──────────────────────────────────────────

trait OptionalExt<T> {
    fn optional(self) -> SqliteResult<Option<T>>;
}

impl<T> OptionalExt<T> for SqliteResult<T> {
    fn optional(self) -> SqliteResult<Option<T>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn source_to_str(source: &CorrectionSource) -> &'static str {
    match source {
        CorrectionSource::Lexicon => "lexicon",
        CorrectionSource::User => "user",
        CorrectionSource::Merge => "merge",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use audraflow_ipc::Segment;

    fn sample_segment(segment_id: &str) -> Segment {
        Segment {
            segment_id: segment_id.to_string(),
            start_ms: 1_000,
            end_ms: 4_000,
            speaker_id: Some("A".to_string()),
            text: "hello".to_string(),
            raw_text: "hello".to_string(),
            confidence: 0.95,
            low_confidence_reasons: vec![],
            corrections: vec![],
            marks: vec![],
        }
    }

    #[test]
    fn timestamp_marks_are_persisted_and_ordered() {
        let storage = Storage::open_in_memory().unwrap();
        storage
            .create_job("job-1", "sample.wav", "hash", false)
            .unwrap();
        storage
            .insert_segments("job-1", &[sample_segment("seg-1")])
            .unwrap();

        assert!(!storage.segment_has_marks("seg-1").unwrap());

        storage
            .add_mark("seg-1", 2_500, Some("Review"), Some("check term"))
            .unwrap();
        storage.add_mark("seg-1", 1_500, None, None).unwrap();

        let marks = storage.get_marks("seg-1").unwrap();
        assert!(storage.segment_has_marks("seg-1").unwrap());
        assert_eq!(marks.len(), 2);
        assert_eq!(marks[0].mark_ms, 1_500);
        assert_eq!(marks[1].mark_ms, 2_500);
        assert_eq!(marks[1].label.as_deref(), Some("Review"));
        assert_eq!(marks[1].note.as_deref(), Some("check term"));
    }

    #[test]
    fn speaker_label_update_is_scoped_to_one_job() {
        let storage = Storage::open_in_memory().unwrap();
        storage
            .create_job("job-1", "sample.wav", "hash", false)
            .unwrap();
        storage
            .create_job("job-2", "other.wav", "hash-2", false)
            .unwrap();

        let mut seg_a = sample_segment("seg-a");
        seg_a.speaker_id = Some("A".into());
        let mut seg_b = sample_segment("seg-b");
        seg_b.speaker_id = Some("B".into());
        let mut seg_other = sample_segment("seg-other");
        seg_other.speaker_id = Some("B".into());

        storage.insert_segments("job-1", &[seg_a, seg_b]).unwrap();
        storage.insert_segments("job-2", &[seg_other]).unwrap();

        let changed = storage
            .update_speaker_label_for_job("job-1", "B", "A")
            .unwrap();

        assert_eq!(changed, 1);
        let job_1_speakers = storage
            .get_segments("job-1")
            .unwrap()
            .into_iter()
            .map(|segment| segment.speaker_id.unwrap())
            .collect::<Vec<_>>();
        assert_eq!(job_1_speakers, vec!["A", "A"]);
        assert_eq!(
            storage
                .get_segment("seg-other")
                .unwrap()
                .unwrap()
                .speaker_id
                .as_deref(),
            Some("B")
        );
    }

    #[test]
    fn list_jobs_returns_recent_jobs_with_limit() {
        let storage = Storage::open_in_memory().unwrap();
        storage
            .create_job("job-a", "first.wav", "hash-a", false)
            .unwrap();
        storage
            .create_job("job-b", "second.wav", "hash-b", true)
            .unwrap();

        let jobs = storage.list_jobs(1).unwrap();

        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].job_id, "job-b");
        assert_eq!(jobs[0].file_path, "second.wav");
        assert!(jobs[0].extreme_accuracy);
    }

    #[test]
    fn low_confidence_reasons_are_persisted() {
        let storage = Storage::open_in_memory().unwrap();
        storage
            .create_job("job-1", "sample.wav", "hash", false)
            .unwrap();
        let mut segment = sample_segment("seg-risk");
        segment.low_confidence_reasons = vec![
            "low_snr".to_string(),
            "term_conflict".to_string(),
            "overlapping_speech".to_string(),
        ];

        storage.insert_segments("job-1", &[segment]).unwrap();

        assert_eq!(
            storage.get_low_confidence_reasons("seg-risk").unwrap(),
            vec!["low_snr", "term_conflict", "overlapping_speech"]
        );
    }

    #[test]
    fn glossary_entries_can_be_disabled_and_reenabled() {
        let storage = Storage::open_in_memory().unwrap();
        let entry = storage
            .upsert_glossary_entry("AudraFlow", &["奥德拉".to_string()], Some("product"))
            .unwrap();

        assert_eq!(storage.list_glossary_entries().unwrap().len(), 1);
        assert!(storage.disable_glossary_entry(entry.id).unwrap());
        assert!(storage.list_glossary_entries().unwrap().is_empty());
        assert!(!storage.disable_glossary_entry(entry.id).unwrap());

        let restored = storage
            .upsert_glossary_entry("AudraFlow", &["Audra Flow".to_string()], Some("product"))
            .unwrap();
        assert_eq!(restored.id, entry.id);
        assert!(restored.enabled);
        assert_eq!(storage.list_glossary_entries().unwrap().len(), 1);
        assert_eq!(restored.aliases.len(), 2);
    }

    #[test]
    fn glossary_entry_can_be_replaced() {
        let storage = Storage::open_in_memory().unwrap();
        let entry = storage
            .upsert_glossary_entry("AudraFlow", &["Audra Flow".to_string()], Some("product"))
            .unwrap();

        let updated = storage
            .replace_glossary_entry(
                entry.id,
                "AudraFlow Pro",
                &["AFP".to_string()],
                Some("plan"),
            )
            .unwrap()
            .unwrap();

        assert_eq!(updated.canonical, "AudraFlow Pro");
        assert_eq!(updated.category.as_deref(), Some("plan"));
        assert_eq!(updated.aliases.len(), 1);
        assert_eq!(updated.aliases[0].alias, "AFP");
    }

    #[test]
    fn v2_migration_adds_marks_table_to_existing_v1_database() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("legacy.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "
                CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
                INSERT INTO schema_version (version) VALUES (1);
                CREATE TABLE jobs (
                    job_id          TEXT PRIMARY KEY NOT NULL,
                    file_path       TEXT NOT NULL,
                    file_hash       TEXT NOT NULL,
                    audio_duration_s REAL,
                    sample_rate     INTEGER,
                    channels        INTEGER,
                    state           TEXT NOT NULL DEFAULT 'pending',
                    extreme_accuracy INTEGER NOT NULL DEFAULT 0,
                    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
                    completed_at    TEXT
                );
                CREATE TABLE segments (
                    segment_id      TEXT PRIMARY KEY NOT NULL,
                    job_id          TEXT NOT NULL REFERENCES jobs(job_id) ON DELETE CASCADE,
                    start_ms        INTEGER NOT NULL,
                    end_ms          INTEGER NOT NULL,
                    speaker_id      TEXT,
                    text            TEXT NOT NULL DEFAULT '',
                    raw_text        TEXT NOT NULL DEFAULT '',
                    confidence      REAL NOT NULL DEFAULT 0.0,
                    sort_order      INTEGER NOT NULL DEFAULT 0
                );
                ",
            )
            .unwrap();
        }

        let storage = Storage::open(&db_path).unwrap();
        storage
            .create_job("job-1", "sample.wav", "hash", false)
            .unwrap();
        storage
            .insert_segments("job-1", &[sample_segment("seg-1")])
            .unwrap();
        storage
            .add_mark("seg-1", 1_750, Some("Mark"), None)
            .unwrap();

        assert!(storage.segment_has_marks("seg-1").unwrap());
        assert_eq!(storage.get_marks("seg-1").unwrap()[0].mark_ms, 1_750);
    }
}
