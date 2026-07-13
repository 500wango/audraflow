use crate::*;
#[derive(Clone)]
pub(crate) enum RuntimeComponentSourceKind {
    SingleFile { file_name: &'static str },
    ArchiveByFileName,
    Installer {
        file_name: &'static str,
        args: &'static [&'static str],
    },
}

pub(crate) const YT_DLP_WINDOWS_REQUIRED_FILES: &[&str] = &["yt-dlp.exe"];
pub(crate) const YT_DLP_UNIX_REQUIRED_FILES: &[&str] = &["yt-dlp"];
pub(crate) const FUNASR_WINDOWS_REQUIRED_FILES: &[&str] = &["llama-funasr-cli.exe"];
pub(crate) const FUNASR_UNIX_REQUIRED_FILES: &[&str] = &["llama-funasr-cli"];
pub(crate) const FUNASR_LLAMA_CPP_RELEASE_TAG: &str = "runtime-llamacpp-v0.1.4";
pub(crate) const VC_REDIST_X64_URL: &str = "https://aka.ms/vc14/vc_redist.x64.exe";
pub(crate) const VC_REDIST_X64_REQUIRED_FILES: &[&str] =
    &["vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll"];

#[derive(Clone)]
pub(crate) struct RuntimeComponentSpec {
    id: &'static str,
    kind: &'static str,
    env_url: &'static str,
    default_url: Option<String>,
    download_size_bytes: u64,
    min_download_bytes: usize,
    required_files: &'static [&'static str],
    source_kind: RuntimeComponentSourceKind,
}

pub(crate) fn runtime_component_specs() -> Vec<RuntimeComponentSpec> {
    let mut specs = Vec::new();

    if cfg!(windows) {
        specs.push(RuntimeComponentSpec {
            id: "vc-redist",
            kind: "required",
            env_url: "AUDRAFLOW_COMPONENT_VC_REDIST_URL",
            default_url: Some(VC_REDIST_X64_URL.into()),
            download_size_bytes: 18 * 1024 * 1024,
            min_download_bytes: 512 * 1024,
            required_files: VC_REDIST_X64_REQUIRED_FILES,
            source_kind: RuntimeComponentSourceKind::Installer {
                file_name: "vc_redist.x64.exe",
                args: &["/install", "/quiet", "/norestart"],
            },
        });
        specs.push(RuntimeComponentSpec {
            id: "whisper",
            kind: "required",
            env_url: "AUDRAFLOW_COMPONENT_WHISPER_URL",
            default_url: Some(github_release_asset_url(&format!(
                "AudraFlow_{}_windows_whisper-runtime.zip",
                env!("CARGO_PKG_VERSION")
            ))),
            download_size_bytes: 28 * 1024 * 1024,
            min_download_bytes: 512 * 1024,
            required_files: &[
                "whisper-cli.exe",
                "whisper.dll",
                "ggml.dll",
                "ggml-base.dll",
                "ggml-cpu.dll",
            ],
            source_kind: RuntimeComponentSourceKind::ArchiveByFileName,
        });
        specs.push(RuntimeComponentSpec {
            id: "ffmpeg",
            kind: "required",
            env_url: "AUDRAFLOW_COMPONENT_FFMPEG_URL",
            default_url: Some("https://github.com/BtbN/FFmpeg-Builds/releases/latest/download/ffmpeg-master-latest-win64-gpl.zip".into()),
            download_size_bytes: 95 * 1024 * 1024,
            min_download_bytes: 1024 * 1024,
            required_files: &["ffmpeg.exe", "ffprobe.exe"],
            source_kind: RuntimeComponentSourceKind::ArchiveByFileName,
        });
    }

    let funasr_download = funasr_official_download();
    specs.push(RuntimeComponentSpec {
        id: "funasr",
        kind: "experimental",
        env_url: "AUDRAFLOW_COMPONENT_FUNASR_URL",
        default_url: funasr_download
            .as_ref()
            .map(|download| download.url.clone()),
        download_size_bytes: funasr_download
            .as_ref()
            .map(|download| download.size_bytes)
            .unwrap_or(0),
        min_download_bytes: 512 * 1024,
        required_files: funasr_component_required_files(),
        source_kind: RuntimeComponentSourceKind::ArchiveByFileName,
    });

    specs.push(RuntimeComponentSpec {
        id: "yt-dlp",
        kind: "optional",
        env_url: "AUDRAFLOW_COMPONENT_YT_DLP_URL",
        default_url: Some(yt_dlp_download_url().into()),
        // Official Windows yt-dlp.exe is ~17MB; keep a conservative floor so truncated
        // GitHub/proxy responses cannot pass verification.
        download_size_bytes: 18 * 1024 * 1024,
        min_download_bytes: 8 * 1024 * 1024,
        required_files: yt_dlp_component_required_files(),
        source_kind: RuntimeComponentSourceKind::SingleFile {
            file_name: yt_dlp_binary_name(),
        },
    });

    specs
}

pub(crate) struct RuntimeComponentDownload {
    url: String,
    size_bytes: u64,
}

pub(crate) fn yt_dlp_component_required_files() -> &'static [&'static str] {
    if cfg!(windows) {
        YT_DLP_WINDOWS_REQUIRED_FILES
    } else {
        YT_DLP_UNIX_REQUIRED_FILES
    }
}

pub(crate) fn funasr_component_required_files() -> &'static [&'static str] {
    if cfg!(windows) {
        FUNASR_WINDOWS_REQUIRED_FILES
    } else {
        FUNASR_UNIX_REQUIRED_FILES
    }
}

pub(crate) fn funasr_official_download() -> Option<RuntimeComponentDownload> {
    let (asset_name, size_bytes) = funasr_official_asset()?;
    Some(RuntimeComponentDownload {
        url: format!(
            "https://github.com/modelscope/FunASR/releases/download/{FUNASR_LLAMA_CPP_RELEASE_TAG}/{asset_name}"
        ),
        size_bytes,
    })
}

pub(crate) fn funasr_official_asset() -> Option<(&'static str, u64)> {
    let asset = if cfg!(all(windows, target_arch = "x86_64")) {
        Some(("funasr-llamacpp-windows-x64.zip", 4_663_344))
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some(("funasr-llamacpp-linux-x64.tar.gz", 7_610_705))
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some(("funasr-llamacpp-linux-arm64.tar.gz", 7_583_429))
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some(("funasr-llamacpp-macos-arm64.tar.gz", 6_816_662))
    } else {
        None
    };

    // Allow env var override for size when releases change
    if let (Some((name, _default_size)), Ok(env_size)) = (
        asset,
        std::env::var("AUDRAFLOW_FUNASR_SIZE_BYTES"),
    ) {
        if let Ok(parsed) = env_size.trim().parse::<u64>() {
            return Some((name, parsed));
        }
    }

    asset
}

pub(crate) fn github_release_asset_url(asset_name: &str) -> String {
    // Prefer runtime env (dev/CI overrides), then compile-time bake from release builds.
    // Never default to "unknown/audraflow" — that makes Settings repair unusable.
    let tag = std::env::var("AUDRAFLOW_COMPONENT_RELEASE_TAG")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            option_env!("AUDRAFLOW_COMPONENT_RELEASE_TAG")
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| format!("v{}", env!("CARGO_PKG_VERSION")));
    let base = std::env::var("AUDRAFLOW_COMPONENT_BASE_URL")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            option_env!("AUDRAFLOW_COMPONENT_BASE_URL")
                .map(|s| s.trim().trim_end_matches('/').to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| {
            let repo = std::env::var("AUDRAFLOW_BUILD_REPO")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    option_env!("AUDRAFLOW_BUILD_REPO")
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                })
                .unwrap_or_else(|| "500wango/audraflow".into());
            format!("https://github.com/{repo}/releases/download/{tag}")
        });
    format!("{base}/{asset_name}")
}

pub(crate) fn runtime_component_download_url(spec: &RuntimeComponentSpec) -> Option<String> {
    std::env::var(spec.env_url)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| spec.default_url.clone())
}

pub(crate) fn runtime_app_data_dir() -> PathBuf {
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

pub(crate) fn runtime_components_root() -> PathBuf {
    runtime_app_data_dir().join("runtime").join("components")
}

pub(crate) fn runtime_component_dir(component_id: &str) -> PathBuf {
    runtime_components_root().join(component_id)
}

pub(crate) fn runtime_component_bin_dir(component_id: &str) -> PathBuf {
    runtime_component_dir(component_id).join("bin")
}

pub(crate) fn runtime_components_root_for_app(app_handle: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(app_handle
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("runtime")
        .join("components"))
}

pub(crate) fn runtime_component_dir_for_app(
    app_handle: &tauri::AppHandle,
    component_id: &str,
) -> Result<PathBuf, String> {
    Ok(runtime_components_root_for_app(app_handle)?.join(component_id))
}

pub(crate) fn find_runtime_component_tool(component_id: &str, file_name: &str) -> Option<PathBuf> {
    let path = runtime_component_bin_dir(component_id).join(file_name);
    is_usable_tool_executable(&path).then_some(path)
}

pub(crate) fn find_runtime_component_tool_for_app(
    app_handle: &tauri::AppHandle,
    component_id: &str,
    file_name: &str,
) -> Option<PathBuf> {
    let component_dir = runtime_component_dir_for_app(app_handle, component_id).ok()?;
    let path = component_dir.join("bin").join(file_name);
    is_usable_tool_executable(&path).then_some(path)
}

pub(crate) fn find_runtime_component_spec(id: &str) -> Option<RuntimeComponentSpec> {
    let normalized = normalize_runtime_component_id(id)?;
    runtime_component_specs()
        .into_iter()
        .find(|spec| spec.id == normalized)
}

pub(crate) fn normalize_runtime_component_id(id: &str) -> Option<&'static str> {
    match id.trim() {
        "whisper" | "whisperCli" | "whisper-cli" => Some("whisper"),
        "ffmpeg" | "ffprobe" => Some("ffmpeg"),
        "vcRedist" | "vc-redist" | "vcredist" | "vc-runtime" | "vcruntime" => {
            Some("vc-redist")
        }
        "funasr" | "funasrCli" | "fun-asr" | "fun-asr-cli" | "llama-funasr-cli" => {
            Some("funasr")
        }
        "ytDlp" | "yt-dlp" | "ytdlp" => Some("yt-dlp"),
        _ => None,
    }
}

pub(crate) fn runtime_component_status(
    app_handle: &tauri::AppHandle,
    spec: &RuntimeComponentSpec,
) -> RuntimeComponentDto {
    if spec.id == "vc-redist" {
        return vc_redist_component_status(spec);
    }

    // Heal installs that downloaded successfully but failed to promote out of staging.
    let _ = recover_orphaned_component_install(app_handle, spec);

    let component_dir = runtime_component_dir_for_app(app_handle, spec.id)
        .unwrap_or_else(|_| runtime_component_dir(spec.id));
    let bin_dir = component_dir.join("bin");
    let mut missing = Vec::new();
    let mut too_small = Vec::new();
    let mut invalid = Vec::new();
    for file in spec.required_files {
        let path = bin_dir.join(file);
        if !path.is_file() {
            missing.push(*file);
            continue;
        }
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if size < 64 {
            too_small.push(format!("{file} ({size} B)"));
            continue;
        }
        if let Err(error) = validate_component_binary_file(spec, &path) {
            invalid.push(error);
        }
    }
    let installed_size_bytes = directory_size_bytes(&component_dir).unwrap_or(0);
    let download_url = runtime_component_download_url(spec);
    let (status, detail) = if missing.is_empty() && too_small.is_empty() && invalid.is_empty() {
        (
            "ready".to_string(),
            Some(format!("Installed in {}", component_dir.display())),
        )
    } else if !missing.is_empty() {
        let orphan = runtime_components_root_for_app(app_handle)
            .ok()
            .map(|root| root.join(format!(".{}.installing", spec.id)))
            .filter(|path| path.exists());
        let orphan_note = orphan
            .map(|path| format!(" Orphaned staging dir exists at {}.", path.display()))
            .unwrap_or_default();
        (
            "missing".to_string(),
            Some(format!(
                "Missing file(s) under {}: {}.{}",
                bin_dir.display(),
                missing.join(", "),
                orphan_note
            )),
        )
    } else if !too_small.is_empty() {
        (
            "missing".to_string(),
            Some(format!(
                "Installed files look incomplete under {}: {}",
                bin_dir.display(),
                too_small.join(", ")
            )),
        )
    } else {
        (
            "missing".to_string(),
            Some(format!(
                "Installed files are invalid under {}: {}",
                bin_dir.display(),
                invalid.join("; ")
            )),
        )
    };

    RuntimeComponentDto {
        id: spec.id.into(),
        status,
        kind: spec.kind.into(),
        install_dir: component_dir.to_string_lossy().into_owned(),
        download_url: download_url.clone(),
        download_size_bytes: spec.download_size_bytes,
        installed_size_bytes,
        required_files: spec.required_files.iter().map(|file| (*file).into()).collect(),
        detail,
        installable: download_url.is_some(),
    }
}

pub(crate) fn vc_redist_component_status(spec: &RuntimeComponentSpec) -> RuntimeComponentDto {
    let missing = vc_redist_missing_files();
    let download_url = runtime_component_download_url(spec);
    let install_dir = vc_redist_install_dir()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Microsoft Visual C++ Runtime".into());
    let (status, detail) = if missing.is_empty() {
        (
            "ready".to_string(),
            Some("Microsoft Visual C++ Runtime x64 is available.".into()),
        )
    } else {
        (
            "missing".to_string(),
            Some(format!("Missing DLL(s): {}", missing.join(", "))),
        )
    };

    RuntimeComponentDto {
        id: spec.id.into(),
        status,
        kind: spec.kind.into(),
        install_dir,
        download_url: download_url.clone(),
        download_size_bytes: spec.download_size_bytes,
        installed_size_bytes: 0,
        required_files: spec.required_files.iter().map(|file| (*file).into()).collect(),
        detail,
        installable: download_url.is_some(),
    }
}

pub(crate) fn vc_redist_install_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        return windows_system32_dir();
    }

    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

pub(crate) fn vc_redist_missing_files() -> Vec<&'static str> {
    #[cfg(target_os = "windows")]
    {
        let Some(system32) = windows_system32_dir() else {
            return VC_REDIST_X64_REQUIRED_FILES.to_vec();
        };
        return VC_REDIST_X64_REQUIRED_FILES
            .iter()
            .filter(|file| !system32.join(file).is_file())
            .copied()
            .collect();
    }

    #[cfg(not(target_os = "windows"))]
    {
        Vec::new()
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn windows_system32_dir() -> Option<PathBuf> {
    std::env::var_os("SystemRoot")
        .or_else(|| std::env::var_os("WINDIR"))
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .map(|path| path.join("System32"))
}

pub(crate) fn runtime_components(app_handle: &tauri::AppHandle) -> Vec<RuntimeComponentDto> {
    runtime_component_specs()
        .iter()
        .map(|spec| runtime_component_status(app_handle, spec))
        .collect()
}

/// Seed managed runtime component directories from files shipped with the installer.
///
/// Windows NSIS hooks attempt the same copy at install time. MSI has no NSIS hooks,
/// and NSIS may miss alternate resource layouts — so the app re-seeds on startup.
/// This is idempotent: existing complete component installs are left alone.
#[cfg(target_os = "windows")]
pub(crate) fn seed_bundled_runtime_components(app_handle: &tauri::AppHandle) {
    if let Err(error) = seed_whisper_runtime_component(app_handle) {
        log::warn!("Whisper runtime seed skipped: {error}");
    }
    if let Err(error) = seed_ffmpeg_runtime_component(app_handle) {
        log::warn!("FFmpeg runtime seed skipped: {error}");
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn seed_whisper_runtime_component(app_handle: &tauri::AppHandle) -> Result<(), String> {
    let required = [
        "whisper-cli.exe",
        "whisper.dll",
        "ggml.dll",
        "ggml-base.dll",
        "ggml-cpu.dll",
    ];
    let dest_bin = runtime_component_dir_for_app(app_handle, "whisper")?.join("bin");
    if required.iter().all(|name| dest_bin.join(name).is_file()) {
        return Ok(());
    }

    let source_dir = find_bundled_whisper_runtime_dir(app_handle)
        .ok_or_else(|| "bundled whisper runtime directory was not found".to_string())?;

    std::fs::create_dir_all(&dest_bin).map_err(|e| e.to_string())?;
    let mut copied = 0usize;
    for name in required {
        let mut source = source_dir.join(name);
        if !source.is_file() && name == "whisper-cli.exe" {
            // Bundled externalBin name
            let alt = source_dir.join("audraflow-whisper-cli.exe");
            if alt.is_file() {
                source = alt;
            }
        }
        if !source.is_file() {
            continue;
        }
        let dest = dest_bin.join(name);
        std::fs::copy(&source, &dest).map_err(|e| {
            format!(
                "failed to seed whisper file {} -> {}: {e}",
                source.display(),
                dest.display()
            )
        })?;
        copied += 1;
    }

    if copied == 0 {
        return Err(format!(
            "no whisper runtime files found under {}",
            source_dir.display()
        ));
    }
    // Partial copies (e.g. CLI without ggml DLLs) must not count as success.
    let missing = required
        .iter()
        .filter(|name| !dest_bin.join(name).is_file())
        .copied()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "whisper seed incomplete under {}: missing {}",
            dest_bin.display(),
            missing.join(", ")
        ));
    }
    for name in required {
        unblock_windows_file(&dest_bin.join(name));
    }
    log::info!(
        "Seeded {copied} Whisper runtime file(s) into {}",
        dest_bin.display()
    );
    Ok(())
}

#[cfg(target_os = "windows")]
pub(crate) fn seed_ffmpeg_runtime_component(app_handle: &tauri::AppHandle) -> Result<(), String> {
    let dest_bin = runtime_component_dir_for_app(app_handle, "ffmpeg")?.join("bin");
    let ffmpeg_dest = dest_bin.join("ffmpeg.exe");
    let ffprobe_dest = dest_bin.join("ffprobe.exe");
    if ffmpeg_dest.is_file() && ffprobe_dest.is_file() {
        return Ok(());
    }

    let ffmpeg_src = find_bundled_tool_file(app_handle, "ffmpeg")
        .ok_or_else(|| "bundled ffmpeg was not found".to_string())?;
    let ffprobe_src = find_bundled_tool_file(app_handle, "ffprobe")
        .ok_or_else(|| "bundled ffprobe was not found".to_string())?;

    std::fs::create_dir_all(&dest_bin).map_err(|e| e.to_string())?;
    std::fs::copy(&ffmpeg_src, &ffmpeg_dest).map_err(|e| {
        format!(
            "failed to seed ffmpeg {} -> {}: {e}",
            ffmpeg_src.display(),
            ffmpeg_dest.display()
        )
    })?;
    std::fs::copy(&ffprobe_src, &ffprobe_dest).map_err(|e| {
        format!(
            "failed to seed ffprobe {} -> {}: {e}",
            ffprobe_src.display(),
            ffprobe_dest.display()
        )
    })?;
    unblock_windows_file(&ffmpeg_dest);
    unblock_windows_file(&ffprobe_dest);
    log::info!("Seeded FFmpeg runtime into {}", dest_bin.display());
    Ok(())
}

#[cfg(target_os = "windows")]
pub(crate) fn find_bundled_whisper_runtime_dir(app_handle: &tauri::AppHandle) -> Option<PathBuf> {
    let mut roots = bundled_sidecar_roots(app_handle);
    roots.extend(runtime_search_roots());
    for root in dedupe_path_list(roots) {
        for candidate in [
            root.join("windows-runtime"),
            root.join("resources").join("windows-runtime"),
            root.clone(),
            root.join("resources"),
        ] {
            if candidate.join("whisper-cli.exe").is_file()
                || candidate.join("audraflow-whisper-cli.exe").is_file()
            {
                // Prefer dirs that also have at least one ggml DLL (full runtime).
                if candidate.join("ggml.dll").is_file()
                    || candidate.join("whisper.dll").is_file()
                    || candidate.join("ggml-base.dll").is_file()
                {
                    return Some(candidate);
                }
            }
        }
    }
    // Fall back to any dir that has the CLI even without DLLs.
    let mut roots = bundled_sidecar_roots(app_handle);
    roots.extend(runtime_search_roots());
    for root in dedupe_path_list(roots) {
        for candidate in [
            root.join("windows-runtime"),
            root.join("resources").join("windows-runtime"),
            root.clone(),
        ] {
            if candidate.join("whisper-cli.exe").is_file()
                || candidate.join("audraflow-whisper-cli.exe").is_file()
            {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(target_os = "windows")]
pub(crate) fn find_bundled_tool_file(app_handle: &tauri::AppHandle, stem: &str) -> Option<PathBuf> {
    let names = vec![
        format!("audraflow-{stem}.exe"),
        format!("{stem}.exe"),
        stem.to_string(),
    ];

    for root in bundled_sidecar_roots(app_handle) {
        for name in &names {
            for candidate in [
                root.join(name),
                root.join("bin").join(name),
                root.join("windows-runtime").join(name),
                root.join("resources").join("windows-runtime").join(name),
            ] {
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    find_bundled_command(stem).or_else(|| find_bundled_command(&format!("{stem}.exe")))
}

pub(crate) fn emit_runtime_component_progress(
    app_handle: &tauri::AppHandle,
    id: &str,
    downloaded_bytes: u64,
    total_bytes: u64,
    message: impl Into<String>,
) {
    let progress_pct = if total_bytes > 0 {
        downloaded_bytes as f64 / total_bytes as f64 * 100.0
    } else {
        0.0
    };
    let _ = app_handle.emit(
        "runtime://component-download-progress",
        RuntimeComponentProgressEvent {
            id: id.to_string(),
            downloaded_bytes,
            total_bytes,
            progress_pct: progress_pct.clamp(0.0, 100.0),
            message: message.into(),
        },
    );
}

pub(crate) async fn install_runtime_component_by_id(
    app_handle: &tauri::AppHandle,
    id: &str,
) -> Result<String, String> {
    let spec = find_runtime_component_spec(id)
        .ok_or_else(|| format!("Unknown runtime component: {id}"))?;
    install_runtime_component(app_handle, &spec).await
}

pub(crate) async fn install_runtime_component(
    app_handle: &tauri::AppHandle,
    spec: &RuntimeComponentSpec,
) -> Result<String, String> {
    let url = runtime_component_download_url(spec)
        .ok_or_else(|| format!("No download URL is configured for runtime component: {}", spec.id))?;
    let parsed = reqwest::Url::parse(&url)
        .map_err(|e| format!("Invalid runtime component URL for {}: {e}", spec.id))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err("Only http and https runtime component URLs are supported.".into());
    }

    if let RuntimeComponentSourceKind::Installer { file_name, args } = &spec.source_kind {
        return install_runtime_component_installer(app_handle, spec, &url, file_name, args).await;
    }

    let root = runtime_components_root_for_app(app_handle)?;
    let component_dir = root.join(spec.id);
    let staging_dir = root.join(format!(".{}.installing", spec.id));
    let staging_bin_dir = staging_dir.join("bin");
    let downloads_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("runtime")
        .join("downloads");
    let download_path = downloads_dir.join(format!("{}.download", spec.id));

    let _ = tokio::fs::remove_dir_all(&staging_dir).await;
    tokio::fs::create_dir_all(&staging_bin_dir)
        .await
        .map_err(|e| format!("Failed to create runtime component directory: {e}"))?;

    emit_runtime_component_progress(app_handle, spec.id, 0, spec.download_size_bytes, "Starting download");
    download_url_to_path_with_progress(
        app_handle,
        spec.id,
        &url,
        &download_path,
        spec.min_download_bytes,
    )
    .await
    .map_err(|e| {
        emit_runtime_component_progress(
            app_handle,
            spec.id,
            0,
            0,
            format!("Download failed: {e}"),
        );
        e
    })?;

    emit_runtime_component_progress(app_handle, spec.id, 0, 0, "Extracting archive...");
    install_component_payload(spec, &download_path, &staging_bin_dir)
        .map_err(|e| {
            emit_runtime_component_progress(app_handle, spec.id, 0, 0, format!("Extraction failed: {e}"));
            e
        })?;
    emit_runtime_component_progress(
        app_handle,
        spec.id,
        0,
        0,
        format!("Verifying files in {}", staging_bin_dir.display()),
    );
    verify_component_files(spec, &staging_bin_dir)
        .map_err(|e| {
            emit_runtime_component_progress(app_handle, spec.id, 0, 0, format!("Verification failed: {e}"));
            e
        })?;
    for file_name in spec.required_files {
        let path = staging_bin_dir.join(file_name);
        // Validate PE/ELF magic before promotion so truncated downloads fail loudly.
        validate_component_binary_file(spec, &path)?;
        unblock_windows_file(&path);
        mark_executable(&path)?;
    }

    // IMPORTANT: promote staging → live BEFORE smoke-testing.
    // Running the exe from staging (previous order) can leave Windows/AV file locks
    // that make rename/copy of the staging tree fail. The live path then stays empty
    // and the UI shows "missing" even though the download finished.
    emit_runtime_component_progress(app_handle, spec.id, 0, 0, "Activating component files...");
    activate_component_directory(&staging_dir, &component_dir)
        .await
        .map_err(|e| {
            emit_runtime_component_progress(
                app_handle,
                spec.id,
                0,
                0,
                format!("Activation failed: {e}"),
            );
            e
        })?;

    let live_bin_dir = component_dir.join("bin");
    verify_component_files(spec, &live_bin_dir).map_err(|e| {
        format!(
            "{} appeared to install, but verification of the live path failed: {e}. Look for leftover files under {}",
            spec.id,
            root.join(format!(".{}.installing", spec.id)).display()
        )
    })?;

    for file_name in spec.required_files {
        unblock_windows_file(&live_bin_dir.join(file_name));
    }

    emit_runtime_component_progress(app_handle, spec.id, 0, 0, "Smoke-testing installed tools...");
    if let Err(error) = smoke_test_installed_component(spec, &live_bin_dir).await {
        // Roll back a broken install so status does not flip between ready/missing.
        let _ = tokio::fs::remove_dir_all(&component_dir).await;
        emit_runtime_component_progress(
            app_handle,
            spec.id,
            0,
            0,
            format!("Smoke test failed: {error}"),
        );
        return Err(error);
    }

    let _ = tokio::fs::remove_file(&download_path).await;
    emit_runtime_component_progress(
        app_handle,
        spec.id,
        spec.download_size_bytes,
        spec.download_size_bytes,
        "Installed",
    );

    // Final guard: the same status path the UI uses must report ready.
    let status = runtime_component_status(app_handle, spec);
    if status.status != "ready" {
        return Err(format!(
            "{} files were written but status is still '{}': {}. Expected files under {}",
            spec.id,
            status.status,
            status.detail.unwrap_or_else(|| "no detail".into()),
            live_bin_dir.display()
        ));
    }

    Ok(format!(
        "{} runtime component installed in {}.",
        spec.id,
        component_dir.display()
    ))
}

/// Promote a staged component tree into the live component directory.
/// Windows antivirus can hold locks that make `rename` fail; fall back to copy.
pub(crate) async fn activate_component_directory(
    staging_dir: &Path,
    component_dir: &Path,
) -> Result<(), String> {
    let _ = tokio::fs::remove_dir_all(component_dir).await;
    match tokio::fs::rename(staging_dir, component_dir).await {
        Ok(()) => Ok(()),
        Err(rename_err) => {
            log::warn!(
                "rename {} -> {} failed ({rename_err}); falling back to copy",
                staging_dir.display(),
                component_dir.display()
            );
            copy_dir_recursive(staging_dir, component_dir)?;
            let _ = tokio::fs::remove_dir_all(staging_dir).await;
            Ok(())
        }
    }
}

pub(crate) fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst)
        .map_err(|e| format!("Failed to create {}: {e}", dst.display()))?;
    for entry in std::fs::read_dir(src)
        .map_err(|e| format!("Failed to read {}: {e}", src.display()))?
    {
        let entry = entry.map_err(|e| format!("Failed to read directory entry: {e}"))?;
        let file_type = entry
            .file_type()
            .map_err(|e| format!("Failed to inspect {}: {e}", entry.path().display()))?;
        let target = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target).map_err(|e| {
                format!(
                    "Failed to copy {} -> {}: {e}",
                    entry.path().display(),
                    target.display()
                )
            })?;
            unblock_windows_file(&target);
        }
    }
    Ok(())
}

/// Run a quick `--version`/`--help` check for CLI components after extraction.
pub(crate) async fn smoke_test_installed_component(
    spec: &RuntimeComponentSpec,
    bin_dir: &Path,
) -> Result<(), String> {
    let (binary_name, args): (&str, &[&str]) = match spec.id {
        "yt-dlp" => (yt_dlp_binary_name(), &["--version"]),
        "funasr" => (funasr_cli_binary_name(), &["--help"]),
        "ffmpeg" => (tool_binary_name("ffmpeg"), &["-version"]),
        "whisper" => (whisper_cli_binary_name(), &["--help"]),
        _ => return Ok(()),
    };
    let program = bin_dir.join(binary_name);
    if !program.is_file() {
        return Err(format!("Smoke test binary missing: {}", program.display()));
    }
    let size = std::fs::metadata(&program)
        .map(|m| m.len())
        .unwrap_or(0);
    if size < 1024 {
        return Err(format!(
            "Smoke test binary is too small ({} bytes): {}",
            size,
            program.display()
        ));
    }
    unblock_windows_file(&program);

    let mut command = tokio::process::Command::new(&program);
    command.args(args);
    apply_no_window_tokio(&mut command);
    let output = tokio::time::timeout(Duration::from_secs(10), command.output())
        .await
        .map_err(|_| {
            format!(
                "Smoke test timed out for {}. Windows may be blocking the downloaded executable.",
                program.display()
            )
        })?
        .map_err(|e| {
            format!(
                "Failed to start smoke test for {}: {e}. If this is a fresh download, check antivirus quarantine or install the VC++ runtime.",
                program.display()
            )
        })?;

    let ok = output.status.success()
        || (spec.id == "funasr" && output_looks_like_funasr_usage(&output));
    if ok {
        return Ok(());
    }

    Err(format!(
        "Installed {} but it exited with {}. {}",
        program.display(),
        output.status,
        short_output(&output.stderr)
            .or_else(|| short_output(&output.stdout))
            .unwrap_or_else(|| "No output.".into())
    ))
}

pub(crate) async fn install_runtime_component_installer(
    app_handle: &tauri::AppHandle,
    spec: &RuntimeComponentSpec,
    url: &str,
    file_name: &str,
    args: &[&str],
) -> Result<String, String> {
    let downloads_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("runtime")
        .join("downloads");
    let download_path = downloads_dir.join(file_name);

    emit_runtime_component_progress(
        app_handle,
        spec.id,
        0,
        spec.download_size_bytes,
        "Starting download",
    );
    download_url_to_path_with_progress(
        app_handle,
        spec.id,
        url,
        &download_path,
        spec.min_download_bytes,
    )
    .await
    .map_err(|e| {
        emit_runtime_component_progress(
            app_handle,
            spec.id,
            0,
            0,
            format!("Download failed: {e}"),
        );
        e
    })?;

    emit_runtime_component_progress(
        app_handle,
        spec.id,
        spec.download_size_bytes,
        spec.download_size_bytes,
        "Installing",
    );
    let output =
        run_component_installer(&download_path, args, Duration::from_secs(15 * 60), spec.id)
            .await?;

    if !installer_exit_succeeded(&output) {
        return Err(format!(
            "{} installer exited with {}. {}",
            spec.id,
            output.status,
            short_output(&output.stderr)
                .or_else(|| short_output(&output.stdout))
                .unwrap_or_else(|| "No output.".into())
        ));
    }

    if spec.id == "vc-redist" {
        let missing = vc_redist_missing_files();
        if !missing.is_empty() {
            return Err(format!(
                "Microsoft Visual C++ Runtime installer finished, but DLL(s) are still missing: {}",
                missing.join(", ")
            ));
        }
    }

    let _ = tokio::fs::remove_file(&download_path).await;
    emit_runtime_component_progress(
        app_handle,
        spec.id,
        spec.download_size_bytes,
        spec.download_size_bytes,
        "Installed",
    );

    Ok(format!("{} runtime component installed.", spec.id))
}

pub(crate) fn installer_exit_succeeded(output: &std::process::Output) -> bool {
    matches!(output.status.code(), Some(0 | 3010 | 1638))
}

#[cfg(target_os = "windows")]
pub(crate) async fn run_component_installer(
    path: &Path,
    args: &[&str],
    timeout: Duration,
    label: &str,
) -> Result<std::process::Output, String> {
    let quoted_args = args
        .iter()
        .map(|arg| powershell_single_quote(arg))
        .collect::<Vec<_>>()
        .join(", ");
    let script = format!(
        "$p = Start-Process -FilePath {} -ArgumentList @({}) -Wait -PassThru -Verb RunAs; exit $p.ExitCode",
        powershell_single_quote(path.to_string_lossy().as_ref()),
        quoted_args
    );
    let mut command = tokio::process::Command::new("powershell.exe");
    command.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &script]);
    // CREATE_NO_WINDOW hides the PowerShell host; UAC for -Verb RunAs still shows.
    apply_no_window_tokio(&mut command);
    tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| format!("{label} installer timed out."))?
        .map_err(|e| format!("Failed to start {label} installer: {e}"))
}

#[cfg(not(target_os = "windows"))]
pub(crate) async fn run_component_installer(
    path: &Path,
    args: &[&str],
    timeout: Duration,
    label: &str,
) -> Result<std::process::Output, String> {
    tokio::time::timeout(timeout, tokio::process::Command::new(path).args(args).output())
        .await
        .map_err(|_| format!("{label} installer timed out."))?
        .map_err(|e| format!("Failed to start {label} installer: {e}"))
}

#[cfg(target_os = "windows")]
pub(crate) fn powershell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

pub(crate) async fn delete_runtime_component_by_id(
    app_handle: &tauri::AppHandle,
    id: &str,
) -> Result<String, String> {
    let spec = find_runtime_component_spec(id)
        .ok_or_else(|| format!("Unknown runtime component: {id}"))?;
    if matches!(
        spec.source_kind,
        RuntimeComponentSourceKind::Installer { .. }
    ) {
        return Ok(format!(
            "{} is managed by Windows and cannot be removed from AudraFlow.",
            spec.id
        ));
    }
    let component_dir = runtime_component_dir_for_app(app_handle, spec.id)?;
    let before = directory_size_bytes(&component_dir).unwrap_or(0);
    let _ = tokio::fs::remove_dir_all(&component_dir).await;
    Ok(format!(
        "{} runtime component removed. Freed {}.",
        spec.id,
        format_file_size(before)
    ))
}

pub(crate) async fn download_url_to_path_with_progress(
    app_handle: &tauri::AppHandle,
    id: &str,
    url: &str,
    destination: &Path,
    min_bytes: usize,
) -> Result<(), String> {
    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create runtime download directory: {e}"))?;
    }

    // Do NOT use Path::with_extension here: for "yt-dlp.download" it becomes
    // "yt-dlp.tmp", which is fine, but keep an explicit unique suffix so multi-dot
    // component ids never collide with unrelated files.
    let tmp_path = destination.with_file_name(format!(
        "{}.part",
        destination
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("component.download")
    ));
    let _ = tokio::fs::remove_file(&tmp_path).await;
    // Token must come from runtime env only — never option_env! — so release
    // binaries cannot embed CI secrets baked in at compile time.
    let token = std::env::var("AUDRAFLOW_GITHUB_TOKEN")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    // yt-dlp.exe alone is ~17MB from GitHub. A 30s whole-request timeout is too
    // short on many networks and produces truncated downloads that later look
    // like "download finished but still missing".
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15 * 60))
        .connect_timeout(Duration::from_secs(30))
        .user_agent("AudraFlow/1.0")
        .default_headers({
            let mut headers = reqwest::header::HeaderMap::new();
            if let Some(token) = token.as_deref() {
                if let Ok(mut auth) =
                    reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
                {
                    auth.set_sensitive(true);
                    headers.insert(reqwest::header::AUTHORIZATION, auth);
                }
            }
            headers
        })
        .build()
        .map_err(|e| format!("Failed to create download client: {e}"))?;

    let has_auth = token.is_some();
    emit_runtime_component_progress(
        app_handle,
        id,
        0,
        0,
        format!("Requesting {url}"),
    );

    log::debug!("Runtime component download: {url} (auth: {has_auth})");

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Failed to download {url}: {e}"))?;

    let status = response.status();
    if !status.is_success() {
        let hint = match status.as_u16() {
            404 => "The runtime component archive was not found. If this is a local build, the release artifacts may not have been uploaded to GitHub yet. Set the AUDRAFLOW_COMPONENT_WHISPER_URL or AUDRAFLOW_COMPONENT_FFMPEG_URL environment variable to a local zip path.".to_string(),
            403 => "Access to the download URL was denied. If the repo is private, set the AUDRAFLOW_GITHUB_TOKEN environment variable or configure secrets.AUDRAFLOW_RELEASE_READ_TOKEN in CI.".to_string(),
            _ => format!("HTTP {}", status),
        };
        return Err(format!("Runtime component download failed: {hint} (URL: {url})"));
    }

    let total = response.content_length().unwrap_or(0);
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::File::create(&tmp_path)
        .await
        .map_err(|e| format!("Failed to create runtime component download file: {e}"))?;
    let mut downloaded = 0_u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Failed while downloading runtime component: {e}"))?;
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("Failed to write runtime component download: {e}"))?;
        downloaded = downloaded.saturating_add(chunk.len() as u64);
        emit_runtime_component_progress(
            app_handle,
            id,
            downloaded,
            total,
            format!(
                "Downloaded {} / {}",
                format_file_size(downloaded),
                if total > 0 {
                    format_file_size(total)
                } else {
                    "unknown".into()
                }
            ),
        );
    }
    file.flush()
        .await
        .map_err(|e| format!("Failed to flush runtime component download: {e}"))?;
    drop(file);

    let size = tokio::fs::metadata(&tmp_path)
        .await
        .map_err(|e| format!("Failed to inspect runtime component download: {e}"))?
        .len();
    if size < min_bytes as u64 {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(format!("Runtime component download is too small: {size} bytes"));
    }
    if file_looks_like_html(&tmp_path)? {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err("Downloaded HTML instead of a runtime component file.".into());
    }

    let _ = tokio::fs::remove_file(destination).await;
    tokio::fs::rename(&tmp_path, destination)
        .await
        .map_err(|e| format!("Failed to save runtime component download: {e}"))
}

pub(crate) fn file_looks_like_html(path: &Path) -> Result<bool, String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("Failed to inspect downloaded runtime component: {e}"))?;
    let mut bytes = [0_u8; 512];
    let len = file
        .read(&mut bytes)
        .map_err(|e| format!("Failed to inspect downloaded runtime component: {e}"))?;
    Ok(looks_like_html(&bytes[..len]))
}

pub(crate) fn install_component_payload(
    spec: &RuntimeComponentSpec,
    payload_path: &Path,
    staging_bin_dir: &Path,
) -> Result<(), String> {
    match &spec.source_kind {
        RuntimeComponentSourceKind::SingleFile { file_name } => {
            std::fs::copy(payload_path, staging_bin_dir.join(file_name))
                .map_err(|e| format!("Failed to install runtime component file: {e}"))?;
            Ok(())
        }
        RuntimeComponentSourceKind::ArchiveByFileName => {
            extract_required_files_from_archive(payload_path, staging_bin_dir, spec.required_files)
        }
        RuntimeComponentSourceKind::Installer { .. } => Err(format!(
            "{} installer components must be handled before payload extraction.",
            spec.id
        )),
    }
}

pub(crate) fn extract_required_files_from_archive(
    archive_path: &Path,
    destination_dir: &Path,
    required_files: &[&str],
) -> Result<(), String> {
    if file_has_zip_header(archive_path)? {
        return extract_required_files_from_zip(archive_path, destination_dir, required_files);
    }
    extract_required_files_from_tar_gz(archive_path, destination_dir, required_files)
}

pub(crate) fn file_has_zip_header(path: &Path) -> Result<bool, String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("Failed to inspect runtime component archive: {e}"))?;
    let mut bytes = [0_u8; 4];
    let len = file
        .read(&mut bytes)
        .map_err(|e| format!("Failed to inspect runtime component archive: {e}"))?;
    Ok(len >= 4 && bytes == [b'P', b'K', 3, 4])
}

pub(crate) fn extract_required_files_from_zip(
    archive_path: &Path,
    destination_dir: &Path,
    required_files: &[&str],
) -> Result<(), String> {
    let file = std::fs::File::open(archive_path)
        .map_err(|e| format!("Failed to open runtime component archive: {e}"))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| format!("Failed to read runtime component archive: {e}"))?;

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| format!("Failed to read runtime component archive entry: {e}"))?;
        if entry.is_dir() {
            continue;
        }
        let Some(entry_name) = zip_entry_basename(entry.name()) else {
            continue;
        };
        let Some(required_name) = required_files
            .iter()
            .find(|required| required.eq_ignore_ascii_case(&entry_name))
        else {
            continue;
        };
        let out_path = destination_dir.join(*required_name);
        let mut output = std::fs::File::create(&out_path)
            .map_err(|e| format!("Failed to create runtime component file: {e}"))?;
        std::io::copy(&mut entry, &mut output)
            .map_err(|e| format!("Failed to extract runtime component file: {e}"))?;
    }

    Ok(())
}

pub(crate) fn extract_required_files_from_tar_gz(
    archive_path: &Path,
    destination_dir: &Path,
    required_files: &[&str],
) -> Result<(), String> {
    let file = std::fs::File::open(archive_path)
        .map_err(|e| format!("Failed to open runtime component archive: {e}"))?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|e| format!("Failed to read runtime component archive: {e}"))?;

    for entry in entries {
        let mut entry =
            entry.map_err(|e| format!("Failed to read runtime component archive entry: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("Failed to read runtime component archive entry path: {e}"))?;
        let Some(entry_name) = archive_entry_basename(path.to_string_lossy().as_ref()) else {
            continue;
        };
        let Some(required_name) = required_files
            .iter()
            .find(|required| required.eq_ignore_ascii_case(&entry_name))
        else {
            continue;
        };
        let out_path = destination_dir.join(*required_name);
        let mut output = std::fs::File::create(&out_path)
            .map_err(|e| format!("Failed to create runtime component file: {e}"))?;
        std::io::copy(&mut entry, &mut output)
            .map_err(|e| format!("Failed to extract runtime component file: {e}"))?;
    }

    Ok(())
}

pub(crate) fn zip_entry_basename(name: &str) -> Option<String> {
    archive_entry_basename(name)
}

pub(crate) fn archive_entry_basename(name: &str) -> Option<String> {
    name.replace('\\', "/")
        .rsplit('/')
        .find(|part| !part.is_empty())
        .map(str::to_string)
}

pub(crate) fn verify_component_files(spec: &RuntimeComponentSpec, bin_dir: &Path) -> Result<(), String> {
    let mut problems = Vec::new();
    for file in spec.required_files {
        let path = bin_dir.join(file);
        if !path.is_file() {
            problems.push(format!("missing {file}"));
            continue;
        }
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if size < 64 {
            problems.push(format!("{file} is only {size} bytes"));
            continue;
        }
        if let Err(error) = validate_component_binary_file(spec, &path) {
            problems.push(error);
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        let listing = list_dir_names(bin_dir).unwrap_or_else(|_| "(unreadable)".into());
        Err(format!(
            "Runtime component {} failed verification in {}: {}. Directory contains: {}",
            spec.id,
            bin_dir.display(),
            problems.join("; "),
            listing
        ))
    }
}

/// Reject truncated/HTML downloads before they are promoted into the live component path.
pub(crate) fn validate_component_binary_file(
    spec: &RuntimeComponentSpec,
    path: &Path,
) -> Result<(), String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("Failed to open {}: {e}", path.display()))?;
    let mut header = [0_u8; 4];
    let len = file
        .read(&mut header)
        .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    if len < 2 {
        return Err(format!("{} is too small to be an executable", path.display()));
    }

    // Windows PE begins with "MZ". Unix ELF begins with 0x7F ELF.
    let is_pe = header[0] == b'M' && header[1] == b'Z';
    let is_elf = len >= 4 && header == [0x7f, b'E', b'L', b'F'];
    let is_mach_o = len >= 4
        && matches!(
            header,
            [0xfe, 0xed, 0xfa, 0xce]
                | [0xce, 0xfa, 0xed, 0xfe]
                | [0xfe, 0xed, 0xfa, 0xcf]
                | [0xcf, 0xfa, 0xed, 0xfe]
                | [0xca, 0xfe, 0xba, 0xbe]
        );

    let expect_windows = cfg!(windows)
        && matches!(spec.id, "yt-dlp" | "funasr" | "whisper" | "ffmpeg" | "vc-redist");
    if expect_windows && !is_pe {
        return Err(format!(
            "{} is not a Windows PE executable (missing MZ header). The download was likely truncated or replaced by an HTML error page.",
            path.display()
        ));
    }
    if cfg!(target_os = "linux") && matches!(spec.id, "yt-dlp" | "funasr") && !is_elf {
        return Err(format!(
            "{} is not a Linux ELF executable. The download was likely truncated or invalid.",
            path.display()
        ));
    }
    if cfg!(target_os = "macos") && matches!(spec.id, "yt-dlp" | "funasr") && !(is_mach_o || is_elf)
    {
        return Err(format!(
            "{} does not look like a macOS binary. The download was likely truncated or invalid.",
            path.display()
        ));
    }

    let _ = (is_pe, is_elf, is_mach_o);
    Ok(())
}

pub(crate) fn list_dir_names(path: &Path) -> Result<String, String> {
    if !path.exists() {
        return Ok("(missing)".into());
    }
    let mut names = std::fs::read_dir(path)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort();
    if names.is_empty() {
        Ok("(empty)".into())
    } else {
        Ok(names.join(", "))
    }
}

/// Promote a leftover `.id.installing` tree if a previous install downloaded files
/// but failed while activating them (common when AV locks the staged exe).
pub(crate) fn recover_orphaned_component_install(
    app_handle: &tauri::AppHandle,
    spec: &RuntimeComponentSpec,
) -> bool {
    let Ok(root) = runtime_components_root_for_app(app_handle) else {
        return false;
    };
    let live = root.join(spec.id);
    let live_bin = live.join("bin");
    if verify_component_files(spec, &live_bin).is_ok() {
        return false;
    }
    let staging = root.join(format!(".{}.installing", spec.id));
    let staging_bin = staging.join("bin");
    if verify_component_files(spec, &staging_bin).is_err() {
        return false;
    }
    let _ = std::fs::remove_dir_all(&live);
    match std::fs::rename(&staging, &live) {
        Ok(()) => {
            log::info!(
                "Recovered orphaned {} install from {}",
                spec.id,
                staging.display()
            );
            true
        }
        Err(error) => {
            log::warn!(
                "Failed to recover orphaned {} install from {}: {error}",
                spec.id,
                staging.display()
            );
            if copy_dir_recursive(&staging, &live).is_ok() {
                let _ = std::fs::remove_dir_all(&staging);
                log::info!("Recovered orphaned {} install via copy", spec.id);
                true
            } else {
                false
            }
        }
    }
}
