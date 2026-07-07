struct ActiveStatusUpdate<'a> {
    job_id: &'a str,
    state: JobState,
    status_message: Option<String>,
    completed_segments: u32,
    total_segments: u32,
    last_segment_id: Option<String>,
    rtf_estimate: Option<f64>,
}

async fn update_active_status(state: &Arc<Mutex<AppState>>, update: ActiveStatusUpdate<'_>) {
    let mut app = state.lock().await;
    if let Some(active) = app.active_jobs.get_mut(update.job_id) {
        active.state = update.state;
        active.completed_segments = update.completed_segments;
        active.total_segments = update.total_segments.max(1);
        if let Some(message) = update.status_message {
            active.status_message = Some(message);
        }
        if let Some(segment_id) = update.last_segment_id {
            active.last_segment_id = Some(segment_id);
        }
        if let Some(rtf) = update.rtf_estimate {
            active.rtf_estimate = rtf;
        }
    }
}

fn validate_runtime_model_path(model_path: &str) -> Result<(), String> {
    let path = Path::new(model_path);
    let metadata =
        std::fs::metadata(path).map_err(|error| format!("ASR model is not available: {error}"))?;
    if !metadata.is_file() {
        return Err("ASR model path is not a file".into());
    }
    if metadata.len() == 0 {
        return Err("ASR model file is empty".into());
    }
    Ok(())
}

fn runtime_transcribe_command(job: &QueueItem) -> Result<tokio::process::Command, String> {
    let asr_engine = normalize_asr_engine_hint(job.asr_engine.as_deref());
    let mut command = runtime_base_command();
    command
        .arg("transcribe")
        .arg(&job.file_path)
        .arg("--engine")
        .arg(asr_engine)
        .arg("--file-hash")
        .arg(&job.file_hash)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(model_path) = job.model_path.as_deref() {
        command.arg("--model").arg(model_path);
    }
    if job.extreme_accuracy {
        command.arg("--extreme-accuracy");
    }
    if let Some(whisper_cli) = resolve_whisper_cli() {
        command.arg("--whisper-cli").arg(whisper_cli);
    }
    if let Some(language) = job.language.as_deref().and_then(normalize_language_hint) {
        command.arg("--language").arg(language);
    }
    if let Some(audio_mode) = job
        .audio_mode
        .as_deref()
        .and_then(normalize_audio_mode_hint)
    {
        command.arg("--audio-mode").arg(audio_mode);
    }
    if let Some(vocal_separation) = job
        .vocal_separation
        .as_deref()
        .and_then(normalize_vocal_separation_hint)
    {
        command.arg("--vocal-separation").arg(vocal_separation);
    }
    Ok(command)
}

fn normalize_asr_engine_hint(asr_engine: Option<&str>) -> &'static str {
    match asr_engine
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "sensevoice" | "sense_voice" => "sensevoice",
        "funasr" | "fun-asr" | "fun_asr" | "funasr-nano" | "fun-asr-nano" => "funasr",
        _ => "whisper",
    }
}

fn starting_runtime_message(job: &QueueItem) -> String {
    if job
        .vocal_separation
        .as_deref()
        .and_then(normalize_vocal_separation_hint)
        .is_some()
    {
        "Starting ASR runtime; Demucs vocal separation may run first".into()
    } else {
        "Starting ASR runtime".into()
    }
}

fn normalize_language_hint(language: &str) -> Option<String> {
    let normalized = language.trim().to_ascii_lowercase();
    if normalized.is_empty() || matches!(normalized.as_str(), "auto" | "detect" | "auto_detect") {
        None
    } else {
        Some(normalized)
    }
}

fn normalize_audio_mode_hint(audio_mode: &str) -> Option<&'static str> {
    match audio_mode.trim().to_ascii_lowercase().as_str() {
        "music" | "lyrics" | "lyric" => Some("music"),
        _ => None,
    }
}

fn normalize_vocal_separation_hint(vocal_separation: &str) -> Option<&'static str> {
    match vocal_separation.trim().to_ascii_lowercase().as_str() {
        "demucs" | "vocal" | "vocals" | "on" | "true" => Some("demucs"),
        _ => None,
    }
}

fn runtime_base_command() -> tokio::process::Command {
    if let Ok(path) =
        std::env::var("AUDRAFLOW_ASR_RUNTIME_BIN").or_else(|_| std::env::var("FT_ASR_RUNTIME_BIN"))
    {
        return tokio::process::Command::new(path);
    }

    if let Some(path) = sibling_runtime_exe() {
        return tokio::process::Command::new(path);
    }

    if let Some(workspace_root) = find_workspace_root() {
        let mut command = tokio::process::Command::new("cargo");
        command
            .current_dir(workspace_root)
            .arg("run")
            .arg("--quiet")
            .arg("--bin")
            .arg("audraflow-asr-runtime")
            .arg("--");
        return command;
    }

    tokio::process::Command::new("audraflow-asr-runtime")
}

fn sibling_runtime_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let runtime_name = if cfg!(windows) {
        "audraflow-asr-runtime.exe"
    } else {
        "audraflow-asr-runtime"
    };
    let candidate = exe.parent()?.join(runtime_name);
    candidate.is_file().then_some(candidate)
}

fn find_workspace_root() -> Option<PathBuf> {
    let mut current = std::env::current_dir().ok()?;
    loop {
        if current.join("Cargo.toml").is_file() && current.join("asr-runtime").is_dir() {
            return Some(current);
        }
        if !current.pop() {
            break;
        }
    }
    None
}

fn resolve_whisper_cli() -> Option<PathBuf> {
    whisper_cli_override()
        .map(PathBuf::from)
        .or_else(|| managed_component_binary("whisper", whisper_cli_binary_name()))
        .or_else(find_bundled_whisper_cli)
        .or_else(|| which::which(whisper_cli_binary_name()).ok())
}

fn whisper_cli_override() -> Option<String> {
    std::env::var("AUDRAFLOW_WHISPER_CLI")
        .ok()
        .or_else(|| std::env::var("FT_WHISPER_CLI").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.trim().is_empty())
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
    let bundled_name = format!("audraflow-{}", whisper_cli_binary_name());
    vec![
        root.join("bin").join(whisper_cli_binary_name()),
        root.join("bin").join(&bundled_name),
        root.join("resources")
            .join("bin")
            .join(whisper_cli_binary_name()),
        root.join("resources").join("bin").join(&bundled_name),
        root.join("resources").join(whisper_cli_binary_name()),
        root.join("resources").join(&bundled_name),
        root.join(whisper_cli_binary_name()),
        root.join(&bundled_name),
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
    let path = runtime_app_data_dir()
        .join("runtime")
        .join("components")
        .join(component_id)
        .join("bin")
        .join(file_name);
    path.is_file().then_some(path)
}

fn runtime_app_data_dir() -> PathBuf {
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

fn preview_bytes(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    if trimmed.chars().count() <= 2000 {
        trimmed.to_string()
    } else {
        format!("{}...", trimmed.chars().take(2000).collect::<String>())
    }
}
