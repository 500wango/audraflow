fn diagnostics_preview(app_handle: &tauri::AppHandle) -> Result<DiagnosticsPreview, String> {
    let db_path = storage_db_path()?;
    let telemetry_path = telemetry_events_path(app_handle)?;
    let model_dir = model_cache_dir(app_handle)?;
    let consent = read_telemetry_consent(app_handle)?;

    Ok(DiagnosticsPreview {
        fields: vec![
            "app_version".into(),
            "os".into(),
            "arch".into(),
            "telemetry_enabled".into(),
            "local_history_bytes".into(),
            "telemetry_events_bytes".into(),
            "model_cache_bytes".into(),
            "model_cache_items".into(),
        ],
        local_history_bytes: file_size_or_zero(&db_path),
        telemetry_events_bytes: file_size_or_zero(&telemetry_path),
        model_cache_bytes: directory_size_bytes(&model_dir)?,
        model_cache_items: count_directory_children(&model_dir)?,
        telemetry_enabled: consent.enabled,
    })
}

fn record_local_telemetry(
    app_handle: &tauri::AppHandle,
    request: TelemetryEventRequest,
) -> Result<(), String> {
    if !read_telemetry_consent(app_handle)?.enabled {
        return Ok(());
    }
    let record = telemetry_request_to_record(request)?;
    append_local_telemetry(app_handle, &record)
}

fn correction_op_type(before: &str, after: &str) -> &'static str {
    if before.is_empty() && !after.is_empty() {
        "insert"
    } else if !before.is_empty() && after.is_empty() {
        "delete"
    } else {
        "replace"
    }
}

fn supported_media_extension(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "mp3" | "wav" | "m4a" | "mp4" | "mov" | "aac" | "flac" | "ogg" | "webm" | "mkv"
    )
}

fn is_supported_media_path(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(supported_media_extension)
}

fn scan_media_folder(folder_path: &Path) -> Result<Vec<String>, String> {
    if !folder_path.is_dir() {
        return Err(format!("Not a folder: {}", folder_path.display()));
    }

    let mut files = Vec::new();
    scan_media_folder_inner(folder_path, &mut files)?;
    files.sort();
    files.dedup();
    Ok(files)
}

fn scan_media_folder_inner(folder_path: &Path, files: &mut Vec<String>) -> Result<(), String> {
    let entries = std::fs::read_dir(folder_path)
        .map_err(|e| format!("Failed to read folder {}: {e}", folder_path.display()))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read folder entry: {e}"))?;
        let path = entry.path();
        if path.is_dir() {
            scan_media_folder_inner(&path, files)?;
        } else if is_supported_media_path(&path) {
            files.push(path.display().to_string());
        }
    }

    Ok(())
}

fn inspect_media_file(path: &Path) -> Result<MediaFileInfo, String> {
    if !path.is_file() {
        return Err(format!("Not a file: {}", path.display()));
    }
    if !is_supported_media_path(path) {
        return Err(format!("Unsupported media file: {}", path.display()));
    }

    let metadata = std::fs::metadata(path)
        .map_err(|e| format!("Failed to inspect file {}: {e}", path.display()))?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("media")
        .to_string();
    let format = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_uppercase();

    Ok(MediaFileInfo {
        file_path: path.display().to_string(),
        file_name,
        format,
        size_bytes: metadata.len(),
        duration_seconds: probe_media_duration_seconds(path),
    })
}

fn job_summary_to_dto(
    storage: &audraflow_storage::Storage,
    job: audraflow_storage::JobRow,
) -> Result<JobSummaryDto, String> {
    let path = PathBuf::from(&job.file_path);
    let segments = storage
        .get_segments(&job.job_id)
        .map_err(|e| e.to_string())?;
    let duration_seconds = job.audio_duration_s.or_else(|| {
        segments
            .last()
            .map(|segment| segment.end_ms.max(0) as f64 / 1000.0)
    });

    Ok(JobSummaryDto {
        job_id: job.job_id,
        file_name: path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("media")
            .to_string(),
        format: path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_uppercase(),
        size_bytes: file_size_or_zero(&path),
        file_path: job.file_path,
        duration_seconds,
        state: job.state,
        extreme_accuracy: job.extreme_accuracy,
        segment_count: segments.len() as u32,
        created_at: job.created_at,
        completed_at: job.completed_at,
    })
}

fn probe_media_duration_seconds(path: &Path) -> Option<f64> {
    let output = std::process::Command::new(ffprobe_command())
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|duration| duration.is_finite() && *duration > 0.0)
}

fn yt_dlp_command() -> PathBuf {
    if let Some(path) = command_env_override("AUDRAFLOW_YT_DLP_BIN")
        .or_else(|| command_env_override("FT_YT_DLP_BIN"))
    {
        return path;
    }

    if let Some(path) = find_managed_tool(yt_dlp_binary_name()) {
        return path;
    }

    if let Some(path) = find_runtime_component_tool("yt-dlp", yt_dlp_binary_name()) {
        return path;
    }

    if let Some(path) =
        find_bundled_command("yt-dlp").or_else(|| find_dev_or_portable_tool(yt_dlp_binary_name()))
    {
        return path;
    }

    let winget_path = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|path| {
            path.join("Microsoft")
                .join("WinGet")
                .join("Packages")
                .join("yt-dlp.yt-dlp_Microsoft.Winget.Source_8wekyb3d8bbwe")
                .join("yt-dlp.exe")
        });

    match winget_path {
        Some(path) if path.exists() => path,
        _ => find_system_command("yt-dlp").unwrap_or_else(|| PathBuf::from("yt-dlp")),
    }
}

fn yt_dlp_command_for_app(app_handle: &tauri::AppHandle) -> PathBuf {
    if let Some(path) =
        command_env_override("AUDRAFLOW_YT_DLP_BIN").or_else(|| command_env_override("FT_YT_DLP_BIN"))
    {
        return path;
    }

    if let Some(path) = find_runtime_component_tool_for_app(app_handle, "yt-dlp", yt_dlp_binary_name())
    {
        return path;
    }

    yt_dlp_command()
}

fn yt_dlp_binary_name() -> &'static str {
    if cfg!(windows) {
        "yt-dlp.exe"
    } else {
        "yt-dlp"
    }
}

fn managed_tools_bin_dir() -> PathBuf {
    app_data_dir().join("tools").join("bin")
}

fn managed_tool_path(name: &str) -> PathBuf {
    managed_tools_bin_dir().join(name)
}

fn find_managed_tool(name: &str) -> Option<PathBuf> {
    let path = managed_tool_path(name);
    path.is_file().then_some(path)
}

fn yt_dlp_download_url() -> &'static str {
    if cfg!(windows) {
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe"
    } else if cfg!(target_os = "macos") {
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos"
    } else {
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux"
    }
}

fn apply_yt_dlp_youtube_compat(command: &mut tokio::process::Command) {
    let extractor_args = std::env::var("AUDRAFLOW_YT_DLP_EXTRACTOR_ARGS")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "youtube:player_client=android_vr".to_string());
    command.arg("--extractor-args").arg(extractor_args);
}

fn ffmpeg_command_for_app(app_handle: &tauri::AppHandle) -> PathBuf {
    command_env_override("AUDRAFLOW_FFMPEG_BIN")
        .or_else(|| command_env_override("FT_FFMPEG_BIN"))
        .or_else(|| find_runtime_component_tool_for_app(app_handle, "ffmpeg", tool_binary_name("ffmpeg")))
        .or_else(|| find_runtime_component_tool("ffmpeg", tool_binary_name("ffmpeg")))
        .or_else(|| find_bundled_command("ffmpeg"))
        .or_else(|| find_dev_or_portable_tool(tool_binary_name("ffmpeg")))
        .unwrap_or_else(|| PathBuf::from("ffmpeg"))
}

fn ffprobe_command() -> PathBuf {
    command_env_override("AUDRAFLOW_FFPROBE_BIN")
        .or_else(|| command_env_override("FT_FFPROBE_BIN"))
        .or_else(|| find_runtime_component_tool("ffmpeg", tool_binary_name("ffprobe")))
        .or_else(|| find_bundled_command("ffprobe"))
        .or_else(|| find_dev_or_portable_tool(tool_binary_name("ffprobe")))
        .unwrap_or_else(|| PathBuf::from("ffprobe"))
}

fn ffprobe_command_for_app(app_handle: &tauri::AppHandle) -> PathBuf {
    command_env_override("AUDRAFLOW_FFPROBE_BIN")
        .or_else(|| command_env_override("FT_FFPROBE_BIN"))
        .or_else(|| find_runtime_component_tool_for_app(app_handle, "ffmpeg", tool_binary_name("ffprobe")))
        .or_else(|| find_runtime_component_tool("ffmpeg", tool_binary_name("ffprobe")))
        .or_else(|| find_bundled_command("ffprobe"))
        .or_else(|| find_dev_or_portable_tool(tool_binary_name("ffprobe")))
        .unwrap_or_else(|| PathBuf::from("ffprobe"))
}

fn command_env_override(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

fn find_system_command(name: &str) -> Option<PathBuf> {
    if let Some(path) = find_command_in_path(name) {
        return Some(path);
    }

    let mut candidates = vec![
        PathBuf::from("/usr/bin").join(name),
        PathBuf::from("/usr/local/bin").join(name),
        PathBuf::from("/snap/bin").join(name),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join(".local/bin").join(name));
    }

    candidates.into_iter().find(|path| path.exists())
}

fn find_command_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
        if cfg!(windows) {
            let exe_candidate = dir.join(format!("{name}.exe"));
            if exe_candidate.exists() {
                return Some(exe_candidate);
            }
        }
    }
    None
}

fn find_bundled_command(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    for root in exe.ancestors() {
        for candidate in bundled_command_candidates(root, name) {
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

fn bundled_command_candidates(root: &Path, name: &str) -> Vec<PathBuf> {
    let prefixed_name = format!("audraflow-{name}");
    let windows_name = if cfg!(windows) && !name.ends_with(".exe") {
        Some(format!("{name}.exe"))
    } else {
        None
    };
    let prefixed_windows_name = windows_name
        .as_ref()
        .map(|value| format!("audraflow-{value}"));
    let mut candidates = vec![
        root.join("bin").join(name),
        root.join("bin").join(&prefixed_name),
        root.join("resources").join("bin").join(name),
        root.join("resources").join("bin").join(&prefixed_name),
        root.join("resources").join(name),
        root.join("resources").join(&prefixed_name),
        root.join(name),
        root.join(&prefixed_name),
        root.join("external").join("ffmpeg").join("bin").join(name),
        root.join("tools").join("ffmpeg").join("bin").join(name),
    ];
    if let Some(windows_name) = windows_name {
        candidates.extend([
            root.join("bin").join(&windows_name),
            root.join("resources").join("bin").join(&windows_name),
            root.join("resources").join(&windows_name),
            root.join(&windows_name),
            root.join("external")
                .join("ffmpeg")
                .join("bin")
                .join(&windows_name),
            root.join("tools")
                .join("ffmpeg")
                .join("bin")
                .join(&windows_name),
        ]);
    }
    if let Some(prefixed_windows_name) = prefixed_windows_name {
        candidates.extend([
            root.join("bin").join(&prefixed_windows_name),
            root.join("resources").join("bin").join(&prefixed_windows_name),
            root.join("resources").join(&prefixed_windows_name),
            root.join(&prefixed_windows_name),
        ]);
    }
    candidates
}

fn format_seconds_arg(seconds: f64) -> String {
    let rounded = seconds.round();
    if (seconds - rounded).abs() < 0.001 {
        format!("{}", rounded as u64)
    } else {
        let formatted = format!("{seconds:.3}");
        formatted
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

fn normalize_skip_start_seconds(value: Option<f64>) -> Result<f64, String> {
    let seconds = value.unwrap_or(0.0);
    if !seconds.is_finite() || seconds < 0.0 {
        return Err("Skip intro must be 0 or a positive number of seconds".into());
    }
    Ok(seconds.min(MAX_SKIP_START_SECONDS))
}

fn normalize_url_preview_seconds(value: Option<f64>) -> Result<f64, String> {
    let seconds = value.unwrap_or(DEFAULT_URL_PREVIEW_SECONDS);
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err("Preview duration must be a positive number of seconds".into());
    }
    Ok(seconds.min(MAX_URL_PREVIEW_SECONDS))
}

fn trim_stderr(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr).trim().to_string();
    let mut chars = text.chars();
    let shortened: String = chars.by_ref().take(1200).collect();
    if chars.next().is_none() {
        text
    } else {
        format!("{shortened}...")
    }
}

#[cfg(target_os = "windows")]
fn pipe_exists() -> bool {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(ORCHESTRATOR_PIPE)
        .is_ok()
}

#[cfg(not(target_os = "windows"))]
fn orchestrator_socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("audraflow-orchestrator.sock")
}

#[cfg(not(target_os = "windows"))]
fn socket_exists() -> bool {
    std::os::unix::net::UnixStream::connect(orchestrator_socket_path()).is_ok()
}

fn workspace_root_from_current_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    for ancestor in exe.ancestors() {
        if ancestor.join("Cargo.toml").exists() && ancestor.join("orchestrator").exists() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

fn bundled_sidecar_names(stem: &str) -> Vec<String> {
    let extension = if cfg!(windows) { ".exe" } else { "" };
    let mut names = vec![format!("{stem}{extension}")];
    if let Some(target_triple) = option_env!("TAURI_ENV_TARGET_TRIPLE") {
        names.push(format!("{stem}-{target_triple}{extension}"));
    }
    names
}

fn bundled_sidecar_roots(app_handle: &tauri::AppHandle) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(app_dir) = exe.parent() {
            roots.push(app_dir.to_path_buf());
            roots.push(app_dir.join("resources"));
            roots.push(app_dir.join("../Resources"));
        }
    }
    if let Ok(resource_dir) = app_handle.path().resource_dir() {
        roots.push(resource_dir.clone());
        roots.push(resource_dir.join("bin"));
    }
    dedupe_path_list(roots)
}

fn find_bundled_sidecar(app_handle: &tauri::AppHandle, stem: &str) -> Option<PathBuf> {
    let names = bundled_sidecar_names(stem);
    for root in bundled_sidecar_roots(app_handle) {
        for name in &names {
            let candidate = root.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn start_orchestrator(app_handle: &tauri::AppHandle) {
    if pipe_exists() {
        log::info!("Orchestrator pipe already available");
        return;
    }

    let mut command = if cfg!(debug_assertions) {
        let Some(workspace_root) = workspace_root_from_current_exe() else {
            log::warn!("Could not locate workspace root; orchestrator was not started");
            return;
        };

        let mut command = std::process::Command::new("cargo");
        command
            .arg("run")
            .arg("-p")
            .arg("audraflow-orchestrator")
            .arg("--bin")
            .arg("audraflow-orchestrator")
            .current_dir(&workspace_root);
        command
    } else {
        let Some(orchestrator_exe) = find_bundled_sidecar(app_handle, "audraflow-orchestrator")
        else {
            log::error!(
                "Could not locate bundled audraflow-orchestrator.exe; searched roots: {:?}",
                bundled_sidecar_roots(app_handle)
            );
            return;
        };
        let app_dir = orchestrator_exe
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));

        let mut command = std::process::Command::new(orchestrator_exe);
        command.current_dir(app_dir);
        command
    };

    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Ok(resource_dir) = app_handle.path().resource_dir() {
        command.env("AUDRAFLOW_RESOURCE_DIR", resource_dir);
    }
    if let Ok(app_data_dir) = app_handle.path().app_data_dir() {
        command.env("AUDRAFLOW_APP_DATA_DIR", app_data_dir);
    }

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    match command.spawn() {
        Ok(child) => log::info!("Started orchestrator process: {}", child.id()),
        Err(e) => log::error!("Failed to start orchestrator: {}", e),
    }
}

#[cfg(not(target_os = "windows"))]
fn start_orchestrator(app_handle: &tauri::AppHandle) {
    if socket_exists() {
        log::info!("Orchestrator socket already available");
        return;
    }

    let mut command = if cfg!(debug_assertions) {
        let Some(workspace_root) = workspace_root_from_current_exe() else {
            log::warn!("Could not locate workspace root; orchestrator was not started");
            return;
        };

        let debug_orchestrator = workspace_root
            .join("target")
            .join("debug")
            .join("audraflow-orchestrator");
        if debug_orchestrator.is_file() {
            let mut command = std::process::Command::new(debug_orchestrator);
            command.current_dir(&workspace_root);
            command
        } else {
            let mut command = std::process::Command::new("cargo");
            command
                .arg("run")
                .arg("-p")
                .arg("audraflow-orchestrator")
                .arg("--bin")
                .arg("audraflow-orchestrator")
                .current_dir(&workspace_root);
            command
        }
    } else {
        let Some(orchestrator_exe) = find_bundled_sidecar(app_handle, "audraflow-orchestrator")
        else {
            log::error!(
                "Could not locate bundled audraflow-orchestrator; searched roots: {:?}",
                bundled_sidecar_roots(app_handle)
            );
            return;
        };
        let app_dir = orchestrator_exe
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));

        let mut command = std::process::Command::new(orchestrator_exe);
        command.current_dir(app_dir);
        command
    };

    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Ok(resource_dir) = app_handle.path().resource_dir() {
        command.env("AUDRAFLOW_RESOURCE_DIR", resource_dir);
    }
    if let Ok(app_data_dir) = app_handle.path().app_data_dir() {
        command.env("AUDRAFLOW_APP_DATA_DIR", app_data_dir);
    }

    match command.spawn() {
        Ok(child) => log::info!("Started orchestrator process: {}", child.id()),
        Err(e) => log::error!("Failed to start orchestrator: {}", e),
    }
}

fn sanitize_remote_filename(raw: &str) -> String {
    let name = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("remote-audio")
        .split('?')
        .next()
        .unwrap_or("remote-audio")
        .split('#')
        .next()
        .unwrap_or("remote-audio");

    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();

    let trimmed = sanitized.trim_matches(['.', '_', '-']);
    if trimmed.is_empty() {
        "remote-audio".into()
    } else {
        trimmed.chars().take(120).collect()
    }
}

fn extension_from_content_type(content_type: Option<&str>) -> Option<&'static str> {
    let value = content_type?.split(';').next()?.trim().to_ascii_lowercase();
    match value.as_str() {
        "audio/mpeg" | "audio/mp3" => Some("mp3"),
        "audio/wav" | "audio/x-wav" | "audio/vnd.wave" => Some("wav"),
        "audio/mp4" | "audio/aac" => Some("m4a"),
        "audio/flac" | "audio/x-flac" => Some("flac"),
        "audio/ogg" | "application/ogg" => Some("ogg"),
        "video/mp4" => Some("mp4"),
        "video/quicktime" => Some("mov"),
        "video/webm" | "audio/webm" => Some("webm"),
        "video/x-matroska" => Some("mkv"),
        _ => None,
    }
}

fn filename_from_headers_or_url(url: &str, content_type: Option<&str>) -> Result<String, String> {
    let mut name = sanitize_remote_filename(url);
    let ext = Path::new(&name)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());

    if let Some(ext) = ext {
        if supported_media_extension(&ext) {
            return Ok(name);
        }
    }

    if let Some(ext) = extension_from_content_type(content_type) {
        name = format!("{name}.{ext}");
        return Ok(name);
    }

    Err("URL must point directly to a supported audio/video file".into())
}
