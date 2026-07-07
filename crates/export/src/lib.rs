//! AudraFlow Export Layer
//!
//! Generates output in all supported formats from the unified Transcript Schema.
//! PRD §15: TXT, Markdown, SRT, VTT, JSON, DOCX, clipboard formats.
//!
//! All exports read from the same segment list — no per-format business logic duplication.

use audraflow_ipc::Segment;
use serde::{Deserialize, Serialize};
use std::io::Write;

// ── Export Options ─────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOptions {
    pub include_timestamps: bool,
    pub include_speakers: bool,
    pub include_marks: bool,
    pub speaker_filter: SpeakerFilter,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SpeakerFilter {
    All,
    NamedOnly,
    Hidden,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptExport {
    pub transcript_id: String,
    pub source_hash: String,
    pub title: String,
    pub segments: Vec<Segment>,
    pub corrections: Vec<ExportCorrection>,
    pub marks: Vec<ExportTimestampMark>,
    pub export_options: ExportOptions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportCorrection {
    pub segment_id: String,
    #[serde(flatten)]
    pub correction: audraflow_ipc::Correction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportTimestampMark {
    pub segment_id: String,
    pub mark_ms: i64,
    pub label: Option<String>,
    pub note: Option<String>,
}

impl TranscriptExport {
    pub fn new(
        transcript_id: impl Into<String>,
        source_hash: impl Into<String>,
        title: impl Into<String>,
        segments: Vec<Segment>,
        export_options: ExportOptions,
    ) -> Self {
        let corrections = segments
            .iter()
            .flat_map(|segment| {
                segment
                    .corrections
                    .iter()
                    .cloned()
                    .map(|correction| ExportCorrection {
                        segment_id: segment.segment_id.clone(),
                        correction,
                    })
            })
            .collect();
        let marks = if export_options.include_marks {
            segments
                .iter()
                .flat_map(|segment| {
                    segment
                        .marks
                        .iter()
                        .cloned()
                        .map(|mark| ExportTimestampMark {
                            segment_id: segment.segment_id.clone(),
                            mark_ms: mark.mark_ms,
                            label: mark.label,
                            note: mark.note,
                        })
                })
                .collect()
        } else {
            Vec::new()
        };

        Self {
            transcript_id: transcript_id.into(),
            source_hash: source_hash.into(),
            title: title.into(),
            segments,
            corrections,
            marks,
            export_options,
        }
    }
}

fn should_render_speaker(speaker: Option<&str>, options: &ExportOptions) -> bool {
    if !options.include_speakers {
        return false;
    }

    match options.speaker_filter {
        SpeakerFilter::Hidden => false,
        SpeakerFilter::All => speaker.is_some_and(|value| !value.trim().is_empty()),
        SpeakerFilter::NamedOnly => speaker.is_some_and(is_named_speaker),
    }
}

pub fn is_named_speaker(speaker: &str) -> bool {
    let normalized = speaker.trim().to_ascii_lowercase();
    !normalized.is_empty()
        && normalized != "speaker"
        && !normalized.strip_prefix("speaker ").is_some_and(|suffix| {
            suffix.len() == 1 && suffix.chars().all(|ch| ch.is_ascii_alphabetic())
        })
}

// ── TXT ────────────────────────────────────────────────────────────────────

pub fn export_txt(segments: &[Segment], options: &ExportOptions) -> String {
    let mut out = String::new();
    for seg in segments {
        if should_render_speaker(seg.speaker_id.as_deref(), options) {
            if let Some(ref spk) = seg.speaker_id {
                out.push_str(&format!("[{}] ", spk));
            }
        }
        if options.include_timestamps {
            out.push_str(&format!("[{}] ", format_timestamp(seg.start_ms)));
        }
        out.push_str(&seg.text);
        out.push('\n');
    }
    out
}

// ── Markdown ───────────────────────────────────────────────────────────────

pub fn export_markdown(segments: &[Segment], title: &str, options: &ExportOptions) -> String {
    let mut out = format!("# {}\n\n", title);
    for seg in segments {
        if should_render_speaker(seg.speaker_id.as_deref(), options) {
            if let Some(ref spk) = seg.speaker_id {
                out.push_str(&format!("**{}**: ", spk));
            }
        }
        if options.include_timestamps {
            out.push_str(&format!("`[{}]` ", format_timestamp(seg.start_ms)));
        }
        out.push_str(&seg.text);
        out.push_str("\n\n");
    }
    out
}

/// Obsidian Callout format: > [!quote]- Speaker (HH:MM:SS)
pub fn export_obsidian_callout(segments: &[Segment]) -> String {
    let mut out = String::new();
    for seg in segments {
        let speaker = seg.speaker_id.as_deref().unwrap_or("Speaker");
        let time = format_timestamp(seg.start_ms);
        out.push_str(&format!(
            "> [!quote]- {} ({})\n> {}\n\n",
            speaker, time, seg.text
        ));
    }
    out
}

/// Notion Toggle format (Markdown details block)
pub fn export_notion_toggle(segments: &[Segment]) -> String {
    let mut out = String::new();
    for seg in segments {
        let speaker = seg.speaker_id.as_deref().unwrap_or("Speaker");
        let time = format_timestamp(seg.start_ms);
        out.push_str(&format!(
            "<details>\n<summary>{} — {}</summary>\n\n{}\n</details>\n\n",
            speaker, time, seg.text
        ));
    }
    out
}

// ── SRT ────────────────────────────────────────────────────────────────────

pub fn export_srt(segments: &[Segment]) -> String {
    export_srt_with_options(
        segments,
        &ExportOptions {
            include_timestamps: true,
            include_speakers: true,
            include_marks: false,
            speaker_filter: SpeakerFilter::All,
        },
    )
}

pub fn export_srt_with_options(segments: &[Segment], options: &ExportOptions) -> String {
    let mut out = String::new();
    for (i, seg) in segments.iter().enumerate() {
        out.push_str(&format!("{}\n", i + 1));
        out.push_str(&format!(
            "{} --> {}\n",
            format_srt_time(seg.start_ms),
            format_srt_time(seg.end_ms)
        ));
        let speaker_prefix = if should_render_speaker(seg.speaker_id.as_deref(), options) {
            seg.speaker_id
                .as_ref()
                .map(|s| format!("[{}] ", s))
                .unwrap_or_default()
        } else {
            String::new()
        };
        out.push_str(&format!("{}{}\n\n", speaker_prefix, seg.text));
    }
    out
}

// ── VTT ────────────────────────────────────────────────────────────────────

pub fn export_vtt(segments: &[Segment]) -> String {
    export_vtt_with_options(
        segments,
        &ExportOptions {
            include_timestamps: true,
            include_speakers: true,
            include_marks: false,
            speaker_filter: SpeakerFilter::All,
        },
    )
}

pub fn export_vtt_with_options(segments: &[Segment], options: &ExportOptions) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for (i, seg) in segments.iter().enumerate() {
        out.push_str(&format!("{}\n", i + 1));
        out.push_str(&format!(
            "{} --> {}\n",
            format_vtt_time(seg.start_ms),
            format_vtt_time(seg.end_ms)
        ));
        let speaker_prefix = if should_render_speaker(seg.speaker_id.as_deref(), options) {
            seg.speaker_id
                .as_ref()
                .map(|s| format!("<v {}>", s))
                .unwrap_or_default()
        } else {
            String::new()
        };
        out.push_str(&format!("{}{}\n\n", speaker_prefix, seg.text));
    }
    out
}

// ── JSON ───────────────────────────────────────────────────────────────────

pub fn export_json(segments: &[Segment]) -> String {
    serde_json::to_string_pretty(segments).unwrap_or_default()
}

pub fn export_transcript_json(export: &TranscriptExport) -> String {
    serde_json::to_string_pretty(export).unwrap_or_default()
}

// ── DOCX ───────────────────────────────────────────────────────────────────

/// Generate a minimal DOCX file (Office Open XML).
/// A .docx is a ZIP containing word/document.xml and supporting files.
pub fn export_docx(
    segments: &[Segment],
    title: &str,
    export_opts: &ExportOptions,
) -> anyhow::Result<Vec<u8>> {
    use std::io::Cursor;
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));

    let zip_opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    // ── [Content_Types].xml ────────────────────────────────────────────────
    zip.start_file("[Content_Types].xml", zip_opts)?;
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#,
    )?;

    // ── _rels/.rels ────────────────────────────────────────────────────────
    zip.start_file("_rels/.rels", zip_opts)?;
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#,
    )?;

    // ── word/_rels/document.xml.rels ───────────────────────────────────────
    zip.start_file("word/_rels/document.xml.rels", zip_opts)?;
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
</Relationships>"#,
    )?;

    // ── word/document.xml ──────────────────────────────────────────────────
    zip.start_file("word/document.xml", zip_opts)?;
    write_docx_body(&mut zip, segments, title, export_opts)?;

    let finished = zip.finish()?;
    Ok(finished.into_inner())
}

fn write_docx_body<W: Write>(
    w: &mut W,
    segments: &[Segment],
    title: &str,
    options: &ExportOptions,
) -> anyhow::Result<()> {
    write!(
        w,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#
    )?;
    write!(
        w,
        r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#
    )?;
    write!(w, r#"<w:body>"#)?;

    // Title
    write!(
        w,
        r#"<w:p><w:r><w:rPr><w:b/><w:sz w:val="32"/></w:rPr><w:t xml:space="preserve">{}"#,
        escape_xml(title)
    )?;
    write!(w, r#"</w:t></w:r></w:p>"#)?;

    // Segments
    for seg in segments {
        write!(w, r#"<w:p>"#)?;

        if should_render_speaker(seg.speaker_id.as_deref(), options) {
            if let Some(ref spk) = seg.speaker_id {
                write!(
                    w,
                    r#"<w:r><w:rPr><w:b/></w:rPr><w:t xml:space="preserve">[{}] </w:t></w:r>"#,
                    escape_xml(spk)
                )?;
            }
        }
        if options.include_timestamps {
            write!(
                w,
                r#"<w:r><w:rPr><w:color w:val="808080"/></w:rPr><w:t xml:space="preserve">[{}] </w:t></w:r>"#,
                format_timestamp(seg.start_ms)
            )?;
        }

        write!(
            w,
            r#"<w:r><w:t xml:space="preserve">{}</w:t></w:r>"#,
            escape_xml(&seg.text)
        )?;
        write!(w, r#"</w:p>"#)?;
    }

    write!(w, r#"</w:body></w:document>"#)?;
    Ok(())
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn format_timestamp(ms: i64) -> String {
    let total_sec = ms / 1000;
    let hours = total_sec / 3600;
    let minutes = (total_sec % 3600) / 60;
    let seconds = total_sec % 60;
    format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
}

fn format_srt_time(ms: i64) -> String {
    let total_sec = ms / 1000;
    let hours = total_sec / 3600;
    let minutes = (total_sec % 3600) / 60;
    let seconds = total_sec % 60;
    let millis = ms % 1000;
    format!("{:02}:{:02}:{:02},{:03}", hours, minutes, seconds, millis)
}

fn format_vtt_time(ms: i64) -> String {
    let total_sec = ms / 1000;
    let hours = total_sec / 3600;
    let minutes = (total_sec % 3600) / 60;
    let seconds = total_sec % 60;
    let millis = ms % 1000;
    format!("{:02}:{:02}:{:02}.{:03}", hours, minutes, seconds, millis)
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_seg(id: &str, start: i64, end: i64, text: &str, speaker: Option<&str>) -> Segment {
        Segment {
            segment_id: id.to_string(),
            start_ms: start,
            end_ms: end,
            speaker_id: speaker.map(|s| s.to_string()),
            text: text.to_string(),
            raw_text: text.to_string(),
            confidence: 0.9,
            low_confidence_reasons: vec![],
            corrections: vec![],
            marks: vec![],
        }
    }

    fn make_marked_corrected_seg() -> Segment {
        let mut segment = make_seg("s1", 0, 5000, "Tencent Cloud", Some("Alice"));
        segment.raw_text = "Tengxun Cloud".to_string();
        segment.low_confidence_reasons = vec!["term_conflict".to_string()];
        segment.corrections = vec![audraflow_ipc::Correction {
            field: "text".to_string(),
            old_value: "Tengxun Cloud".to_string(),
            new_value: "Tencent Cloud".to_string(),
            source: audraflow_ipc::CorrectionSource::Lexicon,
            auto_applied: true,
        }];
        segment.marks = vec![audraflow_ipc::TimestampMark {
            mark_ms: 2500,
            label: Some("Review".to_string()),
            note: Some("confirm product name".to_string()),
        }];
        segment
    }

    fn sample_segments() -> Vec<Segment> {
        vec![
            make_seg("s1", 0, 5000, "Hello world", Some("A")),
            make_seg("s2", 5500, 12000, "This is a test", Some("B")),
        ]
    }

    fn default_options() -> ExportOptions {
        ExportOptions {
            include_timestamps: true,
            include_speakers: true,
            include_marks: false,
            speaker_filter: SpeakerFilter::All,
        }
    }

    fn named_only_options() -> ExportOptions {
        ExportOptions {
            include_timestamps: false,
            include_speakers: true,
            include_marks: false,
            speaker_filter: SpeakerFilter::NamedOnly,
        }
    }

    #[test]
    fn test_export_txt() {
        let txt = export_txt(&sample_segments(), &default_options());
        assert!(txt.contains("[A]"));
        assert!(txt.contains("00:00:00"));
        assert!(txt.contains("Hello world"));
    }

    #[test]
    fn test_export_markdown() {
        let md = export_markdown(&sample_segments(), "Test", &default_options());
        assert!(md.contains("# Test"));
        assert!(md.contains("**A**"));
    }

    #[test]
    fn test_export_srt() {
        let srt = export_srt(&sample_segments());
        assert!(srt.contains("00:00:00,000 --> 00:00:05,000"));
        assert!(srt.contains("[A] Hello world"));
    }

    #[test]
    fn test_export_vtt() {
        let vtt = export_vtt(&sample_segments());
        assert!(vtt.starts_with("WEBVTT"));
        assert!(vtt.contains("00:00:00.000 --> 00:00:05.000"));
    }

    #[test]
    fn test_export_json() {
        let json = export_json(&sample_segments());
        assert!(json.contains("\"segmentId\": \"s1\""));
        assert!(json.contains("\"text\": \"Hello world\""));
    }

    #[test]
    fn test_transcript_export_json_preserves_full_schema() {
        let export = TranscriptExport::new(
            "job-1",
            "source-hash",
            "Meeting",
            vec![make_marked_corrected_seg()],
            ExportOptions {
                include_timestamps: true,
                include_speakers: true,
                include_marks: true,
                speaker_filter: SpeakerFilter::All,
            },
        );

        let json = export_transcript_json(&export);

        assert!(json.contains("\"transcriptId\": \"job-1\""));
        assert!(json.contains("\"sourceHash\": \"source-hash\""));
        assert!(json.contains("\"segments\""));
        assert!(json.contains("\"lowConfidenceReasons\""));
        assert!(json.contains("\"corrections\""));
        assert!(json.contains("\"oldValue\": \"Tengxun Cloud\""));
        assert!(json.contains("\"marks\""));
        assert!(json.contains("\"markMs\": 2500"));
        assert!(json.contains("\"exportOptions\""));
        assert!(json.contains("\"speakerFilter\": \"all\""));
    }

    #[test]
    fn test_transcript_export_json_omits_top_level_marks_when_not_requested() {
        let export = TranscriptExport::new(
            "job-1",
            "source-hash",
            "Meeting",
            vec![make_marked_corrected_seg()],
            ExportOptions {
                include_timestamps: true,
                include_speakers: true,
                include_marks: false,
                speaker_filter: SpeakerFilter::All,
            },
        );

        assert!(export.marks.is_empty());
        assert_eq!(export.segments[0].marks.len(), 1);
    }

    #[test]
    fn test_export_docx() {
        let docx = export_docx(&sample_segments(), "Test Doc", &default_options()).unwrap();
        assert!(!docx.is_empty());
        // DOCX is a ZIP — check ZIP magic bytes
        assert_eq!(&docx[0..2], b"PK");
    }

    #[test]
    fn test_export_obsidian_callout() {
        let callout = export_obsidian_callout(&sample_segments());
        assert!(callout.contains("> [!quote]"));
    }

    #[test]
    fn test_speaker_filter_named_only_hides_placeholder_speakers() {
        let segments = vec![
            make_seg("s1", 0, 1000, "Hello", Some("Alice")),
            make_seg("s2", 1000, 2000, "Placeholder", Some("Speaker A")),
        ];

        let txt = export_txt(&segments, &named_only_options());

        assert!(txt.contains("[Alice] Hello"));
        assert!(txt.contains("Placeholder"));
        assert!(!txt.contains("[Speaker A]"));
    }

    #[test]
    fn test_format_timestamp() {
        assert_eq!(format_timestamp(0), "00:00:00");
        assert_eq!(format_timestamp(62000), "00:01:02");
        assert_eq!(format_timestamp(3661000), "01:01:01");
    }

    #[test]
    fn test_escape_xml() {
        assert_eq!(escape_xml("a & b"), "a &amp; b");
        assert_eq!(escape_xml("<tag>"), "&lt;tag&gt;");
    }
}
