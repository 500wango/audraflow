//! Fun-ASR-Nano llama.cpp/GGUF engine wrapper.
//!
//! This is an experimental lyrics/speech path. It uses the Fun-ASR llama.cpp
//! CLI with JSON output and chunk-level timestamps.

use anyhow::Context;
use audraflow_ipc::Segment;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone)]
pub struct FunAsrEngine {
    cli_path: PathBuf,
    encoder_path: PathBuf,
    llm_path: PathBuf,
    vad_path: Option<PathBuf>,
    language: String,
}

#[derive(Debug, Clone)]
struct FunAsrModelPaths {
    encoder_path: PathBuf,
    llm_path: PathBuf,
    vad_path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct FunAsrJsonOutput {
    segments: Vec<FunAsrJsonSegment>,
}

#[derive(Debug, Deserialize)]
struct FunAsrJsonSegment {
    start_ms: i64,
    end_ms: i64,
    text: String,
}

impl FunAsrEngine {
    pub fn new(language: impl Into<String>) -> anyhow::Result<Self> {
        let cli_path = resolve_funasr_cli();
        let models = resolve_funasr_models()?;
        Ok(Self {
            cli_path,
            encoder_path: models.encoder_path,
            llm_path: models.llm_path,
            vad_path: models.vad_path,
            language: language.into(),
        })
    }

    pub fn transcribe_wav(
        &self,
        wav_path: &Path,
        lyrics_mode: bool,
    ) -> anyhow::Result<Vec<Segment>> {
        let prompt = funasr_prompt(&self.language, lyrics_mode);
        let mut command = Command::new(&self.cli_path);
        command
            .arg("--enc")
            .arg(&self.encoder_path)
            .arg("-m")
            .arg(&self.llm_path)
            .arg("-a")
            .arg(wav_path)
            .arg("--prompt")
            .arg(prompt)
            .arg("--json")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if lyrics_mode {
            command.arg("--chunk").arg("15");
        } else if let Some(vad_path) = self.vad_path.as_ref() {
            command.arg("--vad").arg(vad_path);
        } else {
            command.arg("--chunk").arg("15");
        }

        let output = command.output().with_context(|| {
            format!(
                "Failed to run llama-funasr-cli at {}. Build Fun-ASR llama.cpp runtime or set AUDRAFLOW_FUNASR_CLI.",
                self.cli_path.display()
            )
        })?;

        if !output.status.success() {
            anyhow::bail!("llama-funasr-cli failed: {}", preview_text(&output.stderr));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed: FunAsrJsonOutput =
            serde_json::from_str(stdout.trim()).context("llama-funasr-cli did not return JSON")?;
        Ok(parsed
            .segments
            .into_iter()
            .enumerate()
            .filter_map(|(index, segment)| json_segment_to_segment(index, segment, &self.language))
            .collect())
    }
}

fn json_segment_to_segment(
    index: usize,
    segment: FunAsrJsonSegment,
    language: &str,
) -> Option<Segment> {
    let text = clean_funasr_text(&segment.text, language)?;
    let start_ms = segment.start_ms.max(0);
    let end_ms = segment.end_ms.max(start_ms + 1);
    Some(Segment {
        segment_id: format!("funasr-{index:04}"),
        start_ms,
        end_ms,
        speaker_id: None,
        text: text.clone(),
        raw_text: text,
        confidence: 0.82,
        low_confidence_reasons: vec!["experimental_funasr_chunk_timestamp".into()],
        corrections: vec![],
        marks: vec![],
    })
}

fn clean_funasr_text(text: &str, language: &str) -> Option<String> {
    let cleaned = text
        .replace("/sil", "")
        .replace("<sil>", "")
        .replace("[sil]", "")
        .trim()
        .to_string();
    let cleaned = cleaned
        .trim_matches(|ch: char| ch.is_whitespace())
        .to_string();
    if cleaned.is_empty() {
        return None;
    }

    if is_english_hint(language) {
        let normalized = cleaned
            .trim_matches(|ch: char| ch.is_ascii_punctuation() || ch.is_whitespace())
            .to_ascii_lowercase();
        if matches!(normalized.as_str(), "the" | "a" | "um" | "uh" | "hmm") {
            return None;
        }
        if is_short_cjk_filler(&cleaned) {
            return None;
        }
    }

    Some(cleaned)
}

fn is_short_cjk_filler(text: &str) -> bool {
    let chars = text
        .chars()
        .filter(|ch| !ch.is_whitespace() && !ch.is_ascii_punctuation() && *ch != '。')
        .collect::<Vec<_>>();
    !chars.is_empty()
        && chars.len() <= 12
        && chars
            .iter()
            .all(|ch| matches!(*ch, '嗯' | '啊' | '呃' | '唔' | '哼' | '哦' | '噢'))
}

fn funasr_prompt(language: &str, lyrics_mode: bool) -> &'static str {
    if is_english_hint(language) {
        if lyrics_mode {
            "Transcribe the English lyrics exactly:"
        } else {
            "Transcribe the English speech exactly:"
        }
    } else if is_chinese_hint(language) {
        "语音转写："
    } else if lyrics_mode {
        "Transcribe the lyrics exactly:"
    } else {
        "Transcribe the audio exactly:"
    }
}

fn is_english_hint(language: &str) -> bool {
    matches!(
        language.trim().to_ascii_lowercase().as_str(),
        "en" | "eng" | "english"
    )
}

fn is_chinese_hint(language: &str) -> bool {
    matches!(
        language.trim().to_ascii_lowercase().as_str(),
        "zh" | "cn" | "chinese" | "mandarin"
    )
}

fn resolve_funasr_models() -> anyhow::Result<FunAsrModelPaths> {
    if let (Some(encoder_path), Some(llm_path)) = (
        path_env("AUDRAFLOW_FUNASR_ENCODER").or_else(|| path_env("FT_FUNASR_ENCODER")),
        path_env("AUDRAFLOW_FUNASR_LLM").or_else(|| path_env("FT_FUNASR_LLM")),
    ) {
        ensure_file(&encoder_path, "Fun-ASR encoder")?;
        ensure_file(&llm_path, "Fun-ASR LLM")?;
        let vad_path = path_env("AUDRAFLOW_FUNASR_VAD")
            .or_else(|| path_env("FT_FUNASR_VAD"))
            .filter(|path| path.is_file());
        return Ok(FunAsrModelPaths {
            encoder_path,
            llm_path,
            vad_path,
        });
    }

    let model_dirs = funasr_model_dirs();
    let encoder_path = find_first_file(&model_dirs, &["funasr-encoder-f16.gguf"])
        .context("Fun-ASR encoder model not found. Set AUDRAFLOW_FUNASR_MODEL_DIR or AUDRAFLOW_FUNASR_ENCODER.")?;
    let llm_path = find_first_file(
        &model_dirs,
        &[
            "qwen3-0.6b-q5km.gguf",
            "qwen3-0.6b-q8_0.gguf",
            "qwen3-0.6b-q4km.gguf",
        ],
    )
    .context(
        "Fun-ASR decoder model not found. Set AUDRAFLOW_FUNASR_MODEL_DIR or AUDRAFLOW_FUNASR_LLM.",
    )?;
    let vad_path = find_first_file(&model_dirs, &["fsmn-vad.gguf"]);

    Ok(FunAsrModelPaths {
        encoder_path,
        llm_path,
        vad_path,
    })
}

fn funasr_model_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) =
        path_env("AUDRAFLOW_FUNASR_MODEL_DIR").or_else(|| path_env("FT_FUNASR_MODEL_DIR"))
    {
        dirs.push(dir);
    }
    if let Some(dir) = default_app_funasr_model_dir() {
        dirs.push(dir);
    }

    for root in search_roots() {
        dirs.push(root.join("funasr-gguf"));
        dirs.push(root.join("gguf"));
        dirs.push(root.join("models").join("funasr-nano"));
        dirs.push(root.join("external").join("funasr-llamacpp").join("gguf"));
    }
    dedupe_paths(dirs)
}

fn default_app_funasr_model_dir() -> Option<PathBuf> {
    Some(app_data_dir().join("models").join("funasr-nano"))
}

pub fn resolve_funasr_cli() -> PathBuf {
    path_env("AUDRAFLOW_FUNASR_CLI")
        .or_else(|| path_env("FT_FUNASR_CLI"))
        .or_else(|| managed_component_binary("funasr", funasr_cli_binary_name()))
        .or_else(find_bundled_funasr_cli)
        .or_else(|| which::which(funasr_cli_binary_name()).ok())
        .unwrap_or_else(|| PathBuf::from(funasr_cli_binary_name()))
}

fn find_bundled_funasr_cli() -> Option<PathBuf> {
    for root in search_roots() {
        for candidate in [
            root.join(funasr_cli_binary_name()),
            root.join("bin").join(funasr_cli_binary_name()),
            root.join("external")
                .join("Fun-ASR")
                .join("runtime")
                .join("llama.cpp")
                .join("build")
                .join("bin")
                .join(funasr_cli_binary_name()),
            root.join("external")
                .join("funasr-llamacpp")
                .join("bin")
                .join(funasr_cli_binary_name()),
        ] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn funasr_cli_binary_name() -> &'static str {
    if cfg!(windows) {
        "llama-funasr-cli.exe"
    } else {
        "llama-funasr-cli"
    }
}

fn search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        roots.extend(exe.ancestors().map(Path::to_path_buf));
    }
    if let Ok(cwd) = std::env::current_dir() {
        roots.extend(cwd.ancestors().map(Path::to_path_buf));
    }
    dedupe_paths(roots)
}

fn find_first_file(dirs: &[PathBuf], names: &[&str]) -> Option<PathBuf> {
    dirs.iter()
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|path| path.is_file())
}

fn path_env(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn managed_component_binary(component_id: &str, file_name: &str) -> Option<PathBuf> {
    let path = app_data_dir()
        .join("runtime")
        .join("components")
        .join(component_id)
        .join("bin")
        .join(platform_binary_name(file_name));
    path.is_file().then_some(path)
}

fn platform_binary_name(file_name: &str) -> String {
    if cfg!(windows) && !file_name.ends_with(".exe") {
        format!("{file_name}.exe")
    } else {
        file_name.to_string()
    }
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

fn dedupe_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut deduped = Vec::new();
    for path in paths {
        if !deduped.iter().any(|existing| existing == &path) {
            deduped.push(path);
        }
    }
    deduped
}

fn ensure_file(path: &Path, label: &str) -> anyhow::Result<()> {
    if !path.is_file() {
        anyhow::bail!("{label} file not found: {}", path.display());
    }
    Ok(())
}

fn preview_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
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
    fn english_cleanup_drops_short_filler() {
        assert!(clean_funasr_text("嗯嗯嗯。", "en").is_none());
        assert!(clean_funasr_text("/sil", "en").is_none());
        assert!(clean_funasr_text("the", "en").is_none());
        assert_eq!(
            clean_funasr_text("And you're never alone", "en").unwrap(),
            "And you're never alone"
        );
    }

    #[test]
    fn prompt_respects_language_and_mode() {
        assert_eq!(
            funasr_prompt("en", true),
            "Transcribe the English lyrics exactly:"
        );
        assert_eq!(funasr_prompt("zh", false), "语音转写：");
    }
}
