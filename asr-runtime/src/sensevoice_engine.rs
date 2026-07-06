//! SenseVoice ASR adapter.
//!
//! This adapter keeps AudraFlow's Rust pipeline and segment schema, while
//! delegating model inference to FunASR's SenseVoice Python package.

use anyhow::Context;
use audraflow_ipc::Segment;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::audio_pipeline::AudioChunk;

const JSON_PREFIX: &str = "__AUDRAFLOW_SENSEVOICE_JSON__";

const SENSEVOICE_RUNNER: &str = r#"
import json
import sys

PREFIX = "__AUDRAFLOW_SENSEVOICE_JSON__"

def emit_error(message):
    print(PREFIX + json.dumps({"error": message}, ensure_ascii=False))

try:
    from funasr import AutoModel
    from funasr.utils.postprocess_utils import rich_transcription_postprocess
except Exception as exc:
    emit_error("FunASR is not installed or failed to import: " + repr(exc))
    sys.exit(0)

try:
    payload = json.load(sys.stdin)
    model_name = payload.get("model") or "iic/SenseVoiceSmall"
    vad_model = payload.get("vadModel") or "fsmn-vad"
    device = payload.get("device") or "cpu"
    language = payload.get("language") or "auto"
    chunks = payload.get("chunks") or []

    internal_vad = bool(payload.get("internalVad", True))
    model_kwargs = {
        "model": model_name,
        "device": device,
        "disable_update": True,
    }
    if internal_vad:
        model_kwargs["vad_model"] = vad_model
        model_kwargs["vad_kwargs"] = {"max_single_segment_time": 30000}
    model = AutoModel(**model_kwargs)

    segments = []
    for chunk in chunks:
        path = chunk["path"]
        chunk_index = int(chunk["index"])
        chunk_start = int(chunk["startMs"])
        chunk_end = int(chunk["endMs"])
        generate_kwargs = {
            "input": path,
            "cache": {},
            "language": language,
            "use_itn": True,
            "batch_size_s": 60,
        }
        if internal_vad:
            generate_kwargs["merge_vad"] = True
            generate_kwargs["merge_length_s"] = 15
        result = model.generate(**generate_kwargs)
        if not result:
            continue
        item = result[0]
        sentence_info = item.get("sentence_info") or []
        if sentence_info:
            for sentence_index, sentence in enumerate(sentence_info):
                text = sentence.get("text") or ""
                text = rich_transcription_postprocess(text).strip()
                if not text:
                    continue
                start = int(sentence.get("start", 0))
                end = int(sentence.get("end", max(0, chunk_end - chunk_start)))
                segments.append({
                    "segmentId": f"sense{chunk_index:04}-sent{sentence_index:04}",
                    "startMs": chunk_start + max(0, start),
                    "endMs": chunk_start + max(max(0, start), end),
                    "text": text,
                })
            continue

        text = item.get("text") or ""
        text = rich_transcription_postprocess(text).strip()
        if text:
            segments.append({
                "segmentId": f"sense{chunk_index:04}-seg0000",
                "startMs": chunk_start,
                "endMs": chunk_end,
                "text": text,
            })

    print(PREFIX + json.dumps({"segments": segments}, ensure_ascii=False))
except Exception as exc:
    emit_error("SenseVoice inference failed: " + repr(exc))
"#;

#[derive(Debug, Clone)]
pub struct SenseVoiceEngine {
    python_bin: PathBuf,
    model_name: String,
    vad_model: String,
    device: String,
    language: String,
    internal_vad: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SenseVoiceRequest {
    model: String,
    vad_model: String,
    device: String,
    language: String,
    internal_vad: bool,
    chunks: Vec<SenseVoiceChunkRequest>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SenseVoiceChunkRequest {
    index: usize,
    path: String,
    start_ms: i64,
    end_ms: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SenseVoiceResponse {
    #[serde(default)]
    segments: Vec<SenseVoiceSegment>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SenseVoiceSegment {
    segment_id: String,
    start_ms: i64,
    end_ms: i64,
    text: String,
}

impl SenseVoiceEngine {
    pub fn new(language: impl Into<String>) -> anyhow::Result<Self> {
        let python_bin = resolve_python_bin().context(
            "SenseVoice Python was not found. Use Settings to repair AudraFlow's isolated Python environment, install Python 3, or set AUDRAFLOW_PYTHON_BIN.",
        )?;
        Ok(Self {
            python_bin,
            model_name: std::env::var("AUDRAFLOW_SENSEVOICE_MODEL")
                .unwrap_or_else(|_| "iic/SenseVoiceSmall".into()),
            vad_model: std::env::var("AUDRAFLOW_SENSEVOICE_VAD_MODEL")
                .unwrap_or_else(|_| "fsmn-vad".into()),
            device: std::env::var("AUDRAFLOW_SENSEVOICE_DEVICE").unwrap_or_else(|_| "cpu".into()),
            language: normalize_sensevoice_language(language.into()),
            internal_vad: true,
        })
    }

    pub fn with_internal_vad(mut self, internal_vad: bool) -> Self {
        self.internal_vad = internal_vad;
        self
    }

    pub fn transcribe_chunks(&self, chunks: &[AudioChunk]) -> anyhow::Result<Vec<Segment>> {
        let request = SenseVoiceRequest {
            model: self.model_name.clone(),
            vad_model: self.vad_model.clone(),
            device: self.device.clone(),
            language: self.language.clone(),
            internal_vad: self.internal_vad,
            chunks: chunks
                .iter()
                .map(|chunk| SenseVoiceChunkRequest {
                    index: chunk.index,
                    path: chunk.wav_path.to_string_lossy().into_owned(),
                    start_ms: chunk.start_ms,
                    end_ms: chunk.end_ms,
                })
                .collect(),
        };
        let request_json = serde_json::to_vec(&request)?;

        let mut child = Command::new(&self.python_bin)
            .arg("-c")
            .arg(SENSEVOICE_RUNNER)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("Failed to start Python at {}", self.python_bin.display()))?;

        {
            use std::io::Write;
            let stdin = child
                .stdin
                .as_mut()
                .context("Failed to open SenseVoice Python stdin")?;
            stdin.write_all(&request_json)?;
        }

        let output = child
            .wait_with_output()
            .context("Failed to read SenseVoice Python output")?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        let json = extract_prefixed_json(&stdout).ok_or_else(|| {
            anyhow::anyhow!(
                "SenseVoice returned no JSON. status={} stderr={} stdout={}",
                output.status,
                preview_text(&stderr),
                preview_text(&stdout)
            )
        })?;
        let response: SenseVoiceResponse = serde_json::from_str(json)?;
        if let Some(error) = response.error {
            anyhow::bail!(
                "{error}. Use Settings to repair AudraFlow's isolated Python environment."
            );
        }
        if !output.status.success() {
            anyhow::bail!(
                "SenseVoice Python exited with {}. stderr: {}",
                output.status,
                preview_text(&stderr)
            );
        }

        Ok(response
            .segments
            .into_iter()
            .flat_map(segments_from_sensevoice_segment)
            .collect())
    }
}

fn segments_from_sensevoice_segment(segment: SenseVoiceSegment) -> Vec<Segment> {
    let parts = split_sensevoice_text(&segment.text);
    if parts.is_empty() {
        return Vec::new();
    }

    let start_ms = segment.start_ms;
    let end_ms = segment.end_ms.max(segment.start_ms);
    let duration_ms = (end_ms - start_ms).max(parts.len() as i64);
    let total_chars = parts
        .iter()
        .map(|part| part.chars().count().max(1))
        .sum::<usize>() as i64;
    let part_count = parts.len();
    let mut cursor_ms = start_ms;

    parts
        .into_iter()
        .enumerate()
        .map(|(index, text)| {
            let part_chars = text.chars().count().max(1) as i64;
            let mut part_end_ms = if index == 0 && total_chars == part_chars {
                end_ms
            } else {
                cursor_ms + (duration_ms * part_chars / total_chars).max(1)
            };
            if index + 1 == part_count {
                part_end_ms = end_ms;
            }
            let item = Segment {
                segment_id: if part_count == 1 {
                    segment.segment_id.clone()
                } else {
                    format!("{}-part{:02}", segment.segment_id, index)
                },
                start_ms: cursor_ms,
                end_ms: part_end_ms.max(cursor_ms),
                speaker_id: None,
                text: text.clone(),
                raw_text: text,
                confidence: 0.88,
                low_confidence_reasons: Vec::new(),
                corrections: Vec::new(),
                marks: Vec::new(),
            };
            cursor_ms = part_end_ms;
            item
        })
        .collect()
}

fn split_sensevoice_text(text: &str) -> Vec<String> {
    let cleaned = clean_sensevoice_text(text);
    if cleaned.is_empty() {
        return Vec::new();
    }

    let mut parts = Vec::new();
    let mut current = String::new();
    for ch in cleaned.chars() {
        current.push(ch);
        let current_len = current.chars().count();
        let should_split = matches!(ch, '.' | '!' | '?' | ';' | '。' | '！' | '？' | '；')
            || (matches!(ch, ',' | '，') && current_len >= 36);
        if should_split {
            push_clean_part(&mut parts, &mut current);
        }
    }
    push_clean_part(&mut parts, &mut current);

    if parts.len() <= 1 {
        parts
    } else {
        parts
            .into_iter()
            .filter(|part| part.chars().count() > 1)
            .collect()
    }
}

fn push_clean_part(parts: &mut Vec<String>, current: &mut String) {
    let part = current.trim();
    if !part.is_empty() {
        parts.push(part.to_string());
    }
    current.clear();
}

fn clean_sensevoice_text(text: &str) -> String {
    text.chars()
        .filter(|ch| !is_sensevoice_event_marker(*ch))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

fn is_sensevoice_event_marker(ch: char) -> bool {
    matches!(
        ch,
        '🎼' | '😔' | '😊' | '😡' | '😢' | '🤧' | '👏' | '😀' | '😃'
    )
}

fn normalize_sensevoice_language(language: String) -> String {
    match language.trim().to_ascii_lowercase().as_str() {
        "" | "multi" | "multilingual" => "auto".into(),
        "zh" | "cn" | "chinese" => "zh".into(),
        "en" | "english" => "en".into(),
        other => other.into(),
    }
}

fn resolve_python_bin() -> Option<PathBuf> {
    std::env::var_os("AUDRAFLOW_PYTHON_BIN")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(find_managed_python_bin)
        .or_else(find_bundled_python_bin)
        .or_else(|| which::which("python3").ok())
        .or_else(|| which::which("python").ok())
        .or_else(|| which::which("py").ok())
}

fn find_managed_python_bin() -> Option<PathBuf> {
    let venv_dir = app_data_dir()
        .join("runtime")
        .join("components")
        .join("python-venv");
    let candidate = if cfg!(windows) {
        venv_dir.join("Scripts").join("python.exe")
    } else {
        venv_dir.join("bin").join("python")
    };
    candidate.is_file().then_some(candidate)
}

fn find_bundled_python_bin() -> Option<PathBuf> {
    for root in runtime_search_roots() {
        for candidate in [
            root.join("bin").join("python").join("python.exe"),
            root.join("resources")
                .join("bin")
                .join("python")
                .join("python.exe"),
            root.join("python").join("python.exe"),
        ] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn app_data_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("AUDRAFLOW_APP_DATA_DIR")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
    {
        return path;
    }

    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("com.audraflow.app");
    }

    #[cfg(not(target_os = "windows"))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
            .unwrap_or_else(|| PathBuf::from("."))
            .join("com.audraflow.app")
    }
}

fn runtime_search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        roots.extend(exe.ancestors().map(Path::to_path_buf));
    }
    if let Ok(cwd) = std::env::current_dir() {
        roots.extend(cwd.ancestors().map(Path::to_path_buf));
    }

    let mut deduped = Vec::new();
    for root in roots {
        if !deduped.contains(&root) {
            deduped.push(root);
        }
    }
    deduped
}

fn extract_prefixed_json(output: &str) -> Option<&str> {
    output
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix(JSON_PREFIX))
}

fn preview_text(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= 500 {
        trimmed.to_string()
    } else {
        format!("{}...", trimmed.chars().take(500).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_last_prefixed_json_line() {
        let output = "log\n__AUDRAFLOW_SENSEVOICE_JSON__{\"segments\":[]}\n";
        assert_eq!(extract_prefixed_json(output), Some("{\"segments\":[]}"));
    }

    #[test]
    fn normalizes_language_hints_for_sensevoice() {
        assert_eq!(normalize_sensevoice_language("".into()), "auto");
        assert_eq!(normalize_sensevoice_language(" ZH ".into()), "zh");
        assert_eq!(normalize_sensevoice_language("english".into()), "en");
    }

    #[test]
    fn parses_sensevoice_response_segments() {
        let response: SenseVoiceResponse = serde_json::from_str(
            r#"{"segments":[{"segmentId":"s1","startMs":10,"endMs":20,"text":"hello"}]}"#,
        )
        .unwrap();
        assert_eq!(response.segments[0].segment_id, "s1");
    }

    #[test]
    fn splits_plain_sensevoice_text_without_sentence_info() {
        let segments = segments_from_sensevoice_segment(SenseVoiceSegment {
            segment_id: "sense0001-seg0000".into(),
            start_ms: 0,
            end_ms: 30_000,
            text: "🎼Echoes carved on stones old. Tales of flame and hands grown cold.".into(),
        });

        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].segment_id, "sense0001-seg0000-part00");
        assert_eq!(segments[0].text, "Echoes carved on stones old.");
        assert_eq!(segments[1].text, "Tales of flame and hands grown cold.");
        assert_eq!(segments[0].start_ms, 0);
        assert_eq!(segments[1].end_ms, 30_000);
    }
}
