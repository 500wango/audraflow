# Alpha-1 Interface Freeze

Status: frozen for Alpha-1 handoff on 2026-06-23.

This document freezes the IPC schema, SQLite storage schema, and export schema for Beta work.
During Beta, compatible changes may add optional fields, add new enum variants, or add new tables.
Beta work must not rename fields, remove fields, change existing field meaning, or change persisted table semantics without a new schema version and migration.

## IPC Contract

Transport is JSON over Windows Named Pipe. Every payload is wrapped in `IpcEnvelope`:

- `messageId: string`
- `timestampMs: number`
- `type: string`
- payload fields encoded in `camelCase`

Frozen message variants:

- `jobCreate`
- `jobStatus`
- `jobCancel`
- `jobPause`
- `jobResume`
- `segmentStream`
- `segmentUpdate`
- `correctionApply`
- `diarizationResult`
- `errorReport`
- `checkpointSave`
- `checkpointRestore`
- `exportRequest`
- `exportComplete`
- `diagnosticsRequest`
- `jobPlan`

Frozen transcript segment fields:

- `segmentId`
- `startMs`
- `endMs`
- `speakerId`
- `text`
- `rawText`
- `confidence`
- `lowConfidenceReasons`
- `corrections`
- `marks`

Frozen job states:

- `pending`
- `running`
- `paused`
- `completed`
- `cancelled`
- `failed`
- `notFound`

Frozen export formats:

- `txt`
- `markdown`
- `srt`
- `vtt`
- `json`
- `docx`
- `clipboardObsidian`
- `clipboardNotion`
- `stdoutJson`
- `stdoutMarkdown`

Frozen error-code ranges:

- `1xxx`: file/import errors
- `2xxx`: model/runtime inference errors
- `3xxx`: lexicon/post-processing errors
- `4xxx`: VAD/diarization errors
- `5xxx`: IPC/runtime health errors
- `9xxx`: unknown or crash recovery errors

## Storage Contract

Current `schema_version` is `2`.

Frozen tables:

- `jobs`
- `segments`
- `low_confidence_reasons`
- `corrections`
- `marks`
- `glossary`
- `glossary_aliases`
- `metrics_sessions`
- `metrics_events`
- `checkpoints`
- `segments_fts`

Frozen persistence rules:

- Audio paths stay in `jobs`; audio bytes are not stored in SQLite.
- Transcript text lives in `segments.text`; original ASR output remains in `segments.raw_text`.
- Speaker labels are stored in `segments.speaker_id`.
- Low-confidence queue inputs are stored in `low_confidence_reasons`.
- User and automated edits are stored as diff records in `corrections`.
- Ctrl+T timestamp marks are stored in `marks`.
- Recovery state is stored in `checkpoints.state_blob`.
- Full-text indexing is provided by `segments_fts`.

Compatible Beta storage changes:

- Add nullable columns.
- Add new tables.
- Add indexes.
- Add a new schema version with an idempotent migration.

Incompatible storage changes:

- Rename or drop frozen columns.
- Change existing column meaning.
- Store large media blobs in SQLite.
- Replace existing correction/checkpoint semantics without migration.

## Export Contract

Frozen export options:

- `includeTimestamps`
- `includeSpeakers`
- `includeMarks`
- `speakerFilter`

Frozen speaker filters:

- `all`
- `namedOnly`
- `hidden`

Export behavior:

- TXT and Markdown can include timestamps and speakers.
- Markdown and JSON can include timestamp marks.
- SRT and VTT preserve cue timing.
- DOCX must be readable by Word/WPS-compatible tools.
- Clipboard formats are derived from the same segment list, not separate state.
- JSON export must preserve segment identity, timing, speaker labels, corrections, marks, confidence, and low-confidence reasons.

## Validation Commands

Run these before changing any frozen contract:

```powershell
npm run build
npm run lint
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```
