fn resolve_system_python_invocation() -> Option<RuntimeInvocation> {
    if let Some(path) = command_env_override("AUDRAFLOW_PYTHON_BIN")
        .or_else(|| command_env_override("FT_PYTHON_BIN"))
    {
        return Some(RuntimeInvocation {
            display: path.to_string_lossy().into_owned(),
            program: path,
            base_args: vec![],
        });
    }

    ["python3", "python", "py"].iter().find_map(|name| {
        find_system_command(name).map(|program| {
            let mut base_args = Vec::new();
            if is_py_launcher(&program) {
                base_args.push("-3".into());
            }
            RuntimeInvocation {
                display: command_display_path(&program),
                program,
                base_args,
            }
        })
    })
}

async fn run_runtime_invocation_with_timeout(
    invocation: &RuntimeInvocation,
    args: &[String],
    timeout: Duration,
    label: &str,
) -> Result<std::process::Output, String> {
    tokio::time::timeout(
        timeout,
        tokio::process::Command::new(&invocation.program)
            .args(args)
            .output(),
    )
    .await
    .map_err(|_| format!("{label} timed out after {} seconds.", timeout.as_secs()))?
    .map_err(|e| format!("Failed to start {label} at {}: {e}", invocation.display))
}

fn preflight_url_import_dependencies(
    app_handle: &tauri::AppHandle,
    url: &str,
    skip_start_seconds: f64,
) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("Invalid URL: {e}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => return Err("Only http and https links are supported".into()),
    }

    if skip_start_seconds > 0.0 {
        ensure_runtime_command_available(
            ffmpeg_command_for_app(app_handle),
            "FFmpeg",
            "FFmpeg is required when skipping the start of a direct media link. Download the FFmpeg runtime component in Settings or set AUDRAFLOW_FFMPEG_BIN.",
        )?;
    }

    if is_probable_platform_url(&parsed) {
        ensure_runtime_command_available(
            yt_dlp_command_for_app(app_handle),
            "yt-dlp",
            "This looks like a platform link. Download the yt-dlp runtime component in Settings or set AUDRAFLOW_YT_DLP_BIN before importing it.",
        )?;
    }

    Ok(())
}

fn preflight_transcription_dependencies(
    app_handle: &tauri::AppHandle,
    asr_engine: &str,
) -> Result<(), String> {
    ensure_runtime_command_available(
        ffmpeg_command_for_app(app_handle),
        "FFmpeg",
        "FFmpeg is required for local media decoding. Download the FFmpeg runtime component in Settings or set AUDRAFLOW_FFMPEG_BIN.",
    )?;
    ensure_runtime_command_available(
        ffprobe_command_for_app(app_handle),
        "FFprobe",
        "FFprobe is required for media metadata detection. Download the FFmpeg runtime component in Settings or set AUDRAFLOW_FFPROBE_BIN.",
    )?;

    match asr_engine {
        "whisper" => ensure_runtime_command_startable(
            whisper_cli_command_for_app(app_handle),
            &["--help"],
            Duration::from_secs(8),
            "Whisper CLI",
            "Whisper transcription requires a runnable whisper-cli plus its runtime DLLs. Download the Whisper runtime component in Settings or set AUDRAFLOW_WHISPER_CLI.",
        ),
        "sensevoice" => ensure_sensevoice_python_available(),
        "funasr" => {
            ensure_funasr_cli_startable(
                funasr_cli_command_for_app(app_handle),
                "Fun-ASR transcription requires a runnable llama-funasr-cli. Install the Fun-ASR CLI runtime component, use the Linux package with the bundled CLI, or set AUDRAFLOW_FUNASR_CLI.",
            )?;
            resolve_funasr_model_paths(app_handle).map_err(|error| {
                format!(
                    "Fun-ASR model files are required before using the Fun-ASR engine: {error}. Place GGUF files under the Fun-ASR model directory or set AUDRAFLOW_FUNASR_MODEL_DIR."
                )
            })?;
            Ok(())
        }
        _ => Ok(()),
    }
}

fn ensure_funasr_cli_startable(command: PathBuf, message: &str) -> Result<(), String> {
    let args = vec!["--help".into()];
    let output = run_runtime_probe_with_timeout(&command, &args, Duration::from_secs(8))
        .map_err(|error| format!("{message} Detail: {error}"))?;
    if output.status.success() || output_looks_like_funasr_usage(&output) {
        return Ok(());
    }
    Err(format!(
        "{message} Detail: {}",
        short_output(&output.stderr)
            .or_else(|| short_output(&output.stdout))
            .unwrap_or_else(|| format!("Probe exited with {}.", output.status))
    ))
}

fn preflight_requested_transcription_dependencies(
    app_handle: &tauri::AppHandle,
    asr_engine: Option<&str>,
    audio_mode: Option<&str>,
    extreme_accuracy: bool,
) -> Result<(), String> {
    let requested_asr_engine = normalize_asr_engine(asr_engine);
    let audio_mode = normalize_audio_mode(audio_mode);
    let manager = model_manager(app_handle)?;
    ensure_bundled_default_model(app_handle, &manager)?;
    let selected_model = manager.selected_model().map_err(|e| e.to_string())?;
    let installed_models = manager.list_installed_models().map_err(|e| e.to_string())?;
    let has_whisper_model = preferred_lyrics_whisper_model(
        &installed_models,
        selected_model.as_ref(),
        extreme_accuracy,
    )
    .is_some();
    let resolved_engine = resolve_asr_engine(&requested_asr_engine, &audio_mode, has_whisper_model);
    let selected_model = if resolved_engine == "funasr" {
        None
    } else {
        resolve_whisper_model_for_job(
            &resolved_engine,
            &audio_mode,
            selected_model,
            &installed_models,
            extreme_accuracy,
        )
    };

    if resolved_engine == "whisper" {
        let selected_model = selected_model.ok_or_else(|| {
            "No ASR model is selected. Import and select a ggml model in Settings before starting Whisper transcription.".to_string()
        })?;
        if !selected_model.path.is_file() {
            return Err(format!(
                "Selected ASR model file is missing: {}. Re-import or select another model in Settings.",
                selected_model.path.display()
            ));
        }
    }

    preflight_transcription_dependencies(app_handle, &resolved_engine)
}

fn ensure_runtime_command_available(
    command: PathBuf,
    label: &str,
    recovery_hint: &str,
) -> Result<(), String> {
    if is_runtime_command_available(&command) {
        Ok(())
    } else {
        Err(format!(
            "{label} was not found. {recovery_hint} Checked: {}",
            command.display()
        ))
    }
}

fn ensure_runtime_command_startable(
    command: PathBuf,
    args: &[&str],
    timeout: Duration,
    label: &str,
    recovery_hint: &str,
) -> Result<(), String> {
    ensure_runtime_command_available(command.clone(), label, recovery_hint)?;
    let args = args
        .iter()
        .map(|value| (*value).to_string())
        .collect::<Vec<_>>();
    match run_runtime_probe_with_timeout(&command, &args, timeout) {
        Ok(_) => Ok(()),
        Err(error) => Err(format!(
            "{label} could not be started. {recovery_hint} Checked: {}. Error: {error}",
            command.display()
        )),
    }
}

fn ensure_sensevoice_python_available() -> Result<(), String> {
    let invocation = resolve_python_invocation().ok_or_else(|| {
        "SenseVoice requires Python. Use Settings to create AudraFlow's isolated Python environment, install Python 3, or set AUDRAFLOW_PYTHON_BIN.".to_string()
    })?;
    let script = r#"import importlib.util, sys
missing = [name for name in ("funasr", "modelscope") if importlib.util.find_spec(name) is None]
if missing:
    print("missing Python package(s): " + ", ".join(missing), file=sys.stderr)
    sys.exit(1)
print("sensevoice dependencies ready")
"#;
    let mut args = invocation.base_args.clone();
    args.push("-c".into());
    args.push(script.into());
    let output =
        run_runtime_probe_with_timeout(&invocation.program, &args, Duration::from_secs(8))
            .map_err(|error| {
                format!(
                    "SenseVoice Python dependency check failed at {}: {error}. Use Settings to repair AudraFlow's isolated Python environment.",
                    invocation.display
                )
            })?;
    if output.status.success() {
        return Ok(());
    }

    Err(format!(
        "SenseVoice requires Python packages funasr and modelscope. {} Use Settings to repair AudraFlow's isolated Python environment.",
        short_output(&output.stderr)
            .or_else(|| short_output(&output.stdout))
            .unwrap_or_else(|| "Dependency check failed.".into())
    ))
}

fn run_runtime_probe_with_timeout(
    program: &Path,
    args: &[String],
    timeout: Duration,
) -> Result<std::process::Output, String> {
    let mut command = std::process::Command::new(program);
    if let Some(parent) = program.parent().filter(|path| !path.as_os_str().is_empty()) {
        command.current_dir(parent);
    }
    let mut child = command
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let start = Instant::now();
    loop {
        match child.try_wait().map_err(|error| error.to_string())? {
            Some(_) => return child.wait_with_output().map_err(|error| error.to_string()),
            None if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("probe timed out after {}s", timeout.as_secs()));
            }
            None => std::thread::sleep(Duration::from_millis(40)),
        }
    }
}

fn is_runtime_command_available(command: &Path) -> bool {
    if command.is_file() {
        return true;
    }
    if command.is_absolute() || command.components().count() > 1 {
        return false;
    }
    command
        .file_name()
        .and_then(|value| value.to_str())
        .and_then(find_system_command)
        .is_some()
}

fn is_probable_platform_url(parsed: &reqwest::Url) -> bool {
    let Some(host) = parsed.host_str().map(|host| host.to_ascii_lowercase()) else {
        return false;
    };
    let known_platform = [
        "youtube.com",
        "youtu.be",
        "music.youtube.com",
        "bilibili.com",
        "vimeo.com",
        "soundcloud.com",
        "x.com",
        "twitter.com",
        "facebook.com",
        "instagram.com",
        "tiktok.com",
        "douyin.com",
    ]
    .iter()
    .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")));

    known_platform && !is_probable_direct_media_url(parsed)
}

fn is_probable_direct_media_url(parsed: &reqwest::Url) -> bool {
    parsed
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .and_then(|name| name.rsplit_once('.').map(|(_, ext)| ext))
        .is_some_and(supported_media_extension)
}

#[derive(Debug, Clone)]
struct RuntimeInvocation {
    program: PathBuf,
    base_args: Vec<String>,
    display: String,
}

#[derive(Debug)]
struct FunAsrModelHealthPaths {
    model_dir: Option<PathBuf>,
    encoder_path: PathBuf,
    llm_path: PathBuf,
    vad_path: Option<PathBuf>,
}

fn resolve_python_invocation() -> Option<RuntimeInvocation> {
    if let Some(path) = command_env_override("AUDRAFLOW_PYTHON_BIN")
        .or_else(|| command_env_override("FT_PYTHON_BIN"))
    {
        return Some(RuntimeInvocation {
            display: path.to_string_lossy().into_owned(),
            program: path,
            base_args: vec![],
        });
    }

    if let Some(invocation) = find_managed_python_invocation() {
        return Some(invocation);
    }

    if let Some(invocation) = find_bundled_python_invocation() {
        return Some(invocation);
    }

    ["python3", "python", "py"].iter().find_map(|name| {
        find_system_command(name).map(|program| {
            let mut base_args = Vec::new();
            if is_py_launcher(&program) {
                base_args.push("-3".into());
            }
            RuntimeInvocation {
                display: command_display_path(&program),
                program,
                base_args,
            }
        })
    })
}

fn find_managed_python_invocation() -> Option<RuntimeInvocation> {
    let python = managed_python_bin();
    if python.is_file() {
        return Some(RuntimeInvocation {
            display: python.to_string_lossy().into_owned(),
            program: python,
            base_args: vec![],
        });
    }
    None
}

fn managed_python_bin() -> PathBuf {
    let venv_dir = runtime_component_dir("python-venv");
    if cfg!(windows) {
        venv_dir.join("Scripts").join("python.exe")
    } else {
        venv_dir.join("bin").join("python")
    }
}

fn find_bundled_python_invocation() -> Option<RuntimeInvocation> {
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
                return Some(RuntimeInvocation {
                    display: candidate.to_string_lossy().into_owned(),
                    program: candidate,
                    base_args: vec![],
                });
            }
        }
    }
    None
}

fn resolve_demucs_invocation_for_health() -> Option<RuntimeInvocation> {
    if let Some(path) = command_env_override("AUDRAFLOW_DEMUCS_BIN")
        .or_else(|| command_env_override("FT_DEMUCS_BIN"))
    {
        return Some(RuntimeInvocation {
            display: path.to_string_lossy().into_owned(),
            program: path,
            base_args: vec![],
        });
    }

    if let Some(mut invocation) = find_managed_python_invocation() {
        invocation.display = format!("{} -m demucs", invocation.display);
        invocation.base_args.push("-m".into());
        invocation.base_args.push("demucs".into());
        return Some(invocation);
    }

    if let Some(mut invocation) = find_bundled_python_invocation() {
        invocation.display = format!("{} -m demucs", invocation.display);
        invocation.base_args.push("-m".into());
        invocation.base_args.push("demucs".into());
        return Some(invocation);
    }

    if let Some(program) = find_system_command("demucs") {
        return Some(RuntimeInvocation {
            display: command_display_path(&program),
            program,
            base_args: vec![],
        });
    }

    for python in ["python3", "python", "py"] {
        if let Some(program) = find_system_command(python) {
            let mut base_args = Vec::new();
            let display = if is_py_launcher(&program) {
                base_args.push("-3".into());
                format!("{} -3 -m demucs", command_display_path(&program))
            } else {
                format!("{} -m demucs", command_display_path(&program))
            };
            base_args.push("-m".into());
            base_args.push("demucs".into());
            return Some(RuntimeInvocation {
                program,
                base_args,
                display,
            });
        }
    }

    None
}

fn is_py_launcher(program: &Path) -> bool {
    program
        .file_stem()
        .and_then(|value| value.to_str())
        .is_some_and(|stem| stem.eq_ignore_ascii_case("py"))
}

fn resolve_funasr_model_paths(
    app_handle: &tauri::AppHandle,
) -> Result<FunAsrModelHealthPaths, String> {
    if let (Some(encoder_path), Some(llm_path)) = (
        command_env_override("AUDRAFLOW_FUNASR_ENCODER")
            .or_else(|| command_env_override("FT_FUNASR_ENCODER")),
        command_env_override("AUDRAFLOW_FUNASR_LLM")
            .or_else(|| command_env_override("FT_FUNASR_LLM")),
    ) {
        ensure_runtime_file(&encoder_path, "Fun-ASR encoder")?;
        ensure_runtime_file(&llm_path, "Fun-ASR decoder")?;
        return Ok(FunAsrModelHealthPaths {
            model_dir: encoder_path.parent().map(Path::to_path_buf),
            encoder_path,
            llm_path,
            vad_path: command_env_override("AUDRAFLOW_FUNASR_VAD")
                .or_else(|| command_env_override("FT_FUNASR_VAD"))
                .filter(|path| path.is_file()),
        });
    }

    let model_dirs = funasr_model_dirs_for_health(app_handle);
    let encoder_path = find_first_named_file(&model_dirs, &["funasr-encoder-f16.gguf"])
        .ok_or_else(|| "Fun-ASR encoder model not found.".to_string())?;
    let llm_path = find_first_named_file(
        &model_dirs,
        &[
            "qwen3-0.6b-q5km.gguf",
            "qwen3-0.6b-q8_0.gguf",
            "qwen3-0.6b-q4km.gguf",
        ],
    )
    .ok_or_else(|| "Fun-ASR decoder model not found.".to_string())?;
    let vad_path = find_first_named_file(&model_dirs, &["fsmn-vad.gguf"]);

    Ok(FunAsrModelHealthPaths {
        model_dir: encoder_path.parent().map(Path::to_path_buf),
        encoder_path,
        llm_path,
        vad_path,
    })
}

fn funasr_model_dirs_for_health(app_handle: &tauri::AppHandle) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(path) = command_env_override("AUDRAFLOW_FUNASR_MODEL_DIR")
        .or_else(|| command_env_override("FT_FUNASR_MODEL_DIR"))
    {
        dirs.push(path);
    }
    if let Ok(app_data) = app_handle.path().app_data_dir() {
        dirs.push(app_data.join("models").join("funasr-nano"));
    }
    for root in runtime_search_roots() {
        dirs.push(root.join("funasr-gguf"));
        dirs.push(root.join("gguf"));
        dirs.push(root.join("models").join("funasr-nano"));
        dirs.push(root.join("external").join("funasr-llamacpp").join("gguf"));
    }
    dedupe_path_list(dirs)
}

fn find_first_named_file(dirs: &[PathBuf], names: &[&str]) -> Option<PathBuf> {
    dirs.iter()
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|path| path.is_file())
}

fn ensure_runtime_file(path: &Path, label: &str) -> Result<(), String> {
    if path.is_file() {
        Ok(())
    } else {
        Err(format!("{label} file not found: {}", path.display()))
    }
}
