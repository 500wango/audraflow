//! Whisper engine wrapper.
//!
//! Thin wrapper around whisper.cpp for local ASR inference.
//! Supports CUDA (GPU) and CPU fallback.

use anyhow::Context;
use audraflow_ipc::Segment;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Device capability information.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub cuda_available: bool,
    pub vram_gb: Option<f64>,
    pub cpu_cores: u32,
}

/// The Whisper ASR engine.
#[allow(dead_code)]
pub struct WhisperEngine {
    pub device: DeviceInfo,
    model_path: Option<PathBuf>,
    whisper_cli: PathBuf,
    language: String,
    lyrics_mode: bool,
    suppress_regex_supported: bool,
    pub threads: u32,
}

impl WhisperEngine {
    /// Create a new engine, detecting hardware capabilities.
    pub fn new(device: &DeviceInfo) -> anyhow::Result<Self> {
        let whisper_cli = resolve_whisper_cli(None);
        let suppress_regex_supported = supports_suppress_regex(&whisper_cli);
        let threads = std::env::var("AUDRAFLOW_WHISPER_THREADS")
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(4);
        Ok(Self {
            device: device.clone(),
            model_path: None,
            whisper_cli,
            language: "zh".into(),
            lyrics_mode: false,
            suppress_regex_supported,
            threads,
        })
    }

    /// Set the model path.
    pub fn with_model(mut self, path: PathBuf) -> Self {
        self.model_path = Some(path);
        self
    }

    /// Set the whisper.cpp CLI executable.
    pub fn with_whisper_cli(mut self, path: PathBuf) -> Self {
        self.suppress_regex_supported = supports_suppress_regex(&path);
        self.whisper_cli = path;
        self
    }

    /// Set the transcription language code passed to whisper.cpp.
    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = language.into();
        self
    }

    /// Tune decoding for sung vocals over music.
    pub fn with_lyrics_mode(mut self, enabled: bool) -> Self {
        self.lyrics_mode = enabled;
        self
    }

    /// Set the thread count.
    pub fn with_threads(mut self, threads: u32) -> Self {
        self.threads = threads;
        self
    }

    /// Transcribe an audio file and return time-stamped segments.
    ///
    /// For Alpha-0 MVP, this calls the whisper.cpp CLI as a subprocess.
    /// In later iterations, this will use the whisper.cpp C API directly via FFI.
    pub fn transcribe(&self, audio_path: &Path) -> anyhow::Result<Vec<Segment>> {
        let model_path = self
            .model_path
            .as_ref()
            .context("No model path configured")?;

        let output_path = audio_path.with_extension("json");
        let output_prefix = output_path.with_extension("");

        let mut command = command_in_binary_dir(&self.whisper_cli);
        command
            .arg("-m")
            .arg(model_path)
            .arg("-f")
            .arg(audio_path)
            .arg("-oj") // JSON output
            .arg("-of")
            .arg(&output_prefix)
            .arg("-l")
            .arg(&self.language)
            .arg("-t")
            .arg(self.threads.to_string())
            .arg("--print-progress")
            .arg("false");

        if self.lyrics_mode {
            if let Some(prompt) = lyrics_prompt(&self.language) {
                command.arg("--prompt").arg(prompt);
            }
            if self.suppress_regex_supported {
                if let Some(regex) = lyrics_suppress_regex() {
                    command.arg("--suppress-regex").arg(regex);
                }
            }
        }

        let output = command
            .output()
            .with_context(|| {
                format!(
                    "Failed to run whisper-cli at {}. Ensure whisper.cpp is built or pass a valid CLI path.",
                    self.whisper_cli.display()
                )
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("whisper-cli failed: {}", stderr);
        }

        // Parse JSON output
        let json_path = output_path.with_extension("json");
        let json_str = std::fs::read_to_string(&json_path)
            .context("whisper-cli did not produce JSON output")?;

        let segments = parse_whisper_json(&json_str)?;

        // Clean up temp file
        let _ = std::fs::remove_file(&json_path);

        Ok(segments)
    }

    /// Get audio duration in seconds using FFmpeg.
    pub fn audio_duration_seconds(&self, audio_path: &Path) -> anyhow::Result<f64> {
        let output = Command::new(ffprobe_command())
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
            ])
            .arg(audio_path)
            .output()
            .context("Failed to run ffprobe. Ensure FFmpeg is available.")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let duration: f64 = stdout.trim().parse().unwrap_or(0.0);
        Ok(duration)
    }
}

fn lyrics_suppress_regex() -> Option<String> {
    optional_env_value("AUDRAFLOW_LYRICS_SUPPRESS_REGEX")
}

fn lyrics_prompt(_language: &str) -> Option<String> {
    optional_env_value("AUDRAFLOW_LYRICS_PROMPT")
}

fn optional_env_value(name: &str) -> Option<String> {
    normalize_optional_setting(std::env::var(name).ok())
}

fn normalize_optional_setting(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn supports_suppress_regex(whisper_cli: &Path) -> bool {
    command_in_binary_dir(whisper_cli)
        .arg("--help")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map(|output| {
            String::from_utf8_lossy(&output.stdout).contains("--suppress-regex")
                || String::from_utf8_lossy(&output.stderr).contains("--suppress-regex")
        })
        .unwrap_or(false)
}

fn command_in_binary_dir(program: &Path) -> Command {
    let mut command = Command::new(program);
    if let Some(parent) = program.parent().filter(|path| !path.as_os_str().is_empty()) {
        command.current_dir(parent);

        // Shared libraries (libwhisper.so, libggml.so on Linux; whisper.dll,
        // ggml.dll on Windows) are co-located with the binary.  Ensure the
        // dynamic linker can discover them at runtime.
        #[cfg(target_os = "linux")]
        {
            let mut ld_path = std::ffi::OsString::from(parent);
            if let Some(existing) = std::env::var_os("LD_LIBRARY_PATH") {
                if !existing.is_empty() {
                    ld_path.push(":");
                    ld_path.push(existing);
                }
            }
            command.env("LD_LIBRARY_PATH", ld_path);
        }

        #[cfg(target_os = "windows")]
        {
            let mut new_path = std::ffi::OsString::from(parent);
            if let Some(existing) = std::env::var_os("PATH") {
                if !existing.is_empty() {
                    new_path.push(";");
                    new_path.push(existing);
                }
            }
            command.env("PATH", new_path);
        }
    }
    command
}

fn ffprobe_command() -> PathBuf {
    if let Some(path) = std::env::var_os("AUDRAFLOW_FFPROBE_BIN")
        .or_else(|| std::env::var_os("FT_FFPROBE_BIN"))
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
    {
        return path;
    }

    if let Some(path) = managed_component_binary("ffmpeg", "ffprobe") {
        return path;
    }

    if let Ok(exe_path) = std::env::current_exe() {
        for root in exe_path.ancestors() {
            for candidate in [
                root.join("ffprobe"),
                root.join("bin").join("ffprobe"),
                root.join("external")
                    .join("ffmpeg")
                    .join("bin")
                    .join("ffprobe"),
                root.join("tools")
                    .join("ffmpeg")
                    .join("bin")
                    .join("ffprobe"),
            ] {
                if candidate.exists() {
                    return candidate;
                }
            }
        }
    }

    PathBuf::from("ffprobe")
}

/// Resolve the whisper.cpp CLI executable used by local inference.
pub fn resolve_whisper_cli(explicit: Option<PathBuf>) -> PathBuf {
    if let Some(path) = explicit {
        return path;
    }

    whisper_cli_env_override()
        .or_else(|| managed_component_binary("whisper", whisper_cli_binary_name()))
        .or_else(find_bundled_whisper_cli)
        .or_else(|| which::which(whisper_cli_binary_name()).ok())
        .unwrap_or_else(|| PathBuf::from(whisper_cli_binary_name()))
}

fn whisper_cli_env_override() -> Option<PathBuf> {
    std::env::var("AUDRAFLOW_WHISPER_CLI")
        .ok()
        .or_else(|| std::env::var("FT_WHISPER_CLI").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn find_bundled_whisper_cli() -> Option<PathBuf> {
    let mut roots = Vec::new();
    if let Some(resource_dir) = std::env::var_os("AUDRAFLOW_RESOURCE_DIR") {
        roots.push(PathBuf::from(resource_dir));
    }
    if let Ok(exe) = std::env::current_exe() {
        roots.extend(exe.ancestors().map(Path::to_path_buf));
    }
    if let Ok(cwd) = std::env::current_dir() {
        roots.extend(cwd.ancestors().map(Path::to_path_buf));
    }

    for root in roots {
        for candidate in whisper_cli_candidates(&root) {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

fn whisper_cli_candidates(root: &Path) -> Vec<PathBuf> {
    vec![
        root.join("bin").join(whisper_cli_binary_name()),
        root.join("resources")
            .join("bin")
            .join(whisper_cli_binary_name()),
        root.join("resources").join(whisper_cli_binary_name()),
        root.join(whisper_cli_binary_name()),
        root.join("external")
            .join("whisper.cpp")
            .join("build-linux")
            .join("bin")
            .join(whisper_cli_binary_name()),
        root.join("external")
            .join("whisper.cpp")
            .join("build")
            .join("bin")
            .join(whisper_cli_binary_name()),
        root.join("whisper.cpp")
            .join("build-linux")
            .join("bin")
            .join(whisper_cli_binary_name()),
        root.join("whisper.cpp")
            .join("build")
            .join("bin")
            .join(whisper_cli_binary_name()),
    ]
}

fn whisper_cli_binary_name() -> &'static str {
    if cfg!(windows) {
        "whisper-cli.exe"
    } else {
        "whisper-cli"
    }
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

/// Detect hardware capabilities for the current machine.
pub fn detect_device() -> DeviceInfo {
    let cuda_available = check_cuda();
    let vram_gb = if cuda_available {
        estimate_vram().ok()
    } else {
        None
    };
    let cpu_cores = num_cpus::get() as u32;

    DeviceInfo {
        cuda_available,
        vram_gb,
        cpu_cores,
    }
}

fn check_cuda() -> bool {
    // Check for nvidia-smi
    Command::new("nvidia-smi")
        .arg("-L")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn estimate_vram() -> anyhow::Result<f64> {
    let output = Command::new("nvidia-smi")
        .args(["--query-gpu=memory.total", "--format=csv,noheader,nounits"])
        .output()
        .context("nvidia-smi not available")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mb: f64 = stdout.trim().parse().unwrap_or(0.0);
    Ok(mb / 1024.0) // Convert MB to GB
}

/// Parse whisper.cpp JSON output into segments.
pub fn parse_whisper_json(json: &str) -> anyhow::Result<Vec<Segment>> {
    let value: serde_json::Value = serde_json::from_str(json.trim_start_matches('\u{feff}'))?;

    let transcriptions = value
        .get("transcription")
        .or_else(|| value.get("transcriptions"))
        .and_then(|value| value.as_array())
        .context("Missing 'transcription' array in whisper JSON")?;

    let mut segments = Vec::new();

    for (i, seg) in transcriptions.iter().enumerate() {
        let start_ms = read_whisper_segment_ms(seg, "from");
        let end_ms = read_whisper_segment_ms(seg, "to");
        let text = seg["text"].as_str().unwrap_or("").to_string();
        let confidence = 0.9; // whisper.cpp doesn't expose per-segment confidence easily

        segments.push(Segment {
            segment_id: format!("seg-{}", i),
            start_ms,
            end_ms,
            speaker_id: None,
            text: text.clone(),
            raw_text: text,
            confidence,
            low_confidence_reasons: vec![],
            corrections: vec![],
            marks: vec![],
        });
    }

    Ok(segments)
}

fn read_whisper_segment_ms(segment: &serde_json::Value, field: &str) -> i64 {
    segment
        .get("offsets")
        .and_then(|offsets| offsets.get(field))
        .and_then(value_as_milliseconds)
        .or_else(|| {
            segment
                .get("timestamps")
                .and_then(|timestamps| timestamps.get(field))
                .and_then(value_as_milliseconds)
        })
        .unwrap_or(0)
}

fn value_as_milliseconds(value: &serde_json::Value) -> Option<i64> {
    if let Some(ms) = value.as_i64() {
        return Some(ms);
    }
    if let Some(number) = value.as_f64() {
        return Some(number.round() as i64);
    }
    value.as_str().and_then(|text| {
        text.parse::<i64>()
            .ok()
            .or_else(|| parse_timestamp_ms(text))
    })
}

fn parse_timestamp_ms(value: &str) -> Option<i64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    let parts: Vec<&str> = trimmed.split(':').collect();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [seconds] => (0, 0, *seconds),
        [minutes, seconds] => (0, minutes.parse::<i64>().ok()?, *seconds),
        [hours, minutes, seconds] => (
            hours.parse::<i64>().ok()?,
            minutes.parse::<i64>().ok()?,
            *seconds,
        ),
        _ => return None,
    };

    let (seconds, millis) = parse_seconds_and_millis(seconds)?;
    Some(((hours * 3600 + minutes * 60 + seconds) * 1000) + millis)
}

fn parse_seconds_and_millis(value: &str) -> Option<(i64, i64)> {
    let normalized = value.replace(',', ".");
    let mut parts = normalized.splitn(2, '.');
    let seconds = parts.next()?.parse::<i64>().ok()?;
    let millis = parts
        .next()
        .map(|fraction| {
            let digits: String = fraction.chars().take(3).collect();
            format!("{digits:0<3}").parse::<i64>().ok()
        })
        .unwrap_or(Some(0))?;
    Some((seconds, millis))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_lyrics_settings_ignore_empty_values() {
        assert_eq!(
            normalize_optional_setting(Some(" keep ".into())),
            Some("keep".into())
        );
        assert!(normalize_optional_setting(Some("  ".into())).is_none());
        assert!(normalize_optional_setting(None).is_none());
    }
}
