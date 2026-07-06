#[derive(Clone)]
enum RuntimeComponentSourceKind {
    SingleFile { file_name: &'static str },
    ZipByFileName,
}

const YT_DLP_WINDOWS_REQUIRED_FILES: &[&str] = &["yt-dlp.exe"];
const YT_DLP_UNIX_REQUIRED_FILES: &[&str] = &["yt-dlp"];

#[derive(Clone)]
struct RuntimeComponentSpec {
    id: &'static str,
    kind: &'static str,
    env_url: &'static str,
    default_url: String,
    download_size_bytes: u64,
    min_download_bytes: usize,
    required_files: &'static [&'static str],
    source_kind: RuntimeComponentSourceKind,
}

fn runtime_component_specs() -> Vec<RuntimeComponentSpec> {
    let mut specs = Vec::new();

    if cfg!(windows) {
        specs.push(RuntimeComponentSpec {
            id: "whisper",
            kind: "required",
            env_url: "AUDRAFLOW_COMPONENT_WHISPER_URL",
            default_url: github_release_asset_url(&format!(
                "AudraFlow_{}_windows_whisper-runtime.zip",
                env!("CARGO_PKG_VERSION")
            )),
            download_size_bytes: 28 * 1024 * 1024,
            min_download_bytes: 512 * 1024,
            required_files: &[
                "whisper-cli.exe",
                "whisper.dll",
                "ggml.dll",
                "ggml-base.dll",
                "ggml-cpu.dll",
            ],
            source_kind: RuntimeComponentSourceKind::ZipByFileName,
        });
        specs.push(RuntimeComponentSpec {
            id: "ffmpeg",
            kind: "required",
            env_url: "AUDRAFLOW_COMPONENT_FFMPEG_URL",
            default_url: github_release_asset_url(&format!(
                "AudraFlow_{}_windows_ffmpeg-runtime.zip",
                env!("CARGO_PKG_VERSION")
            )),
            download_size_bytes: 95 * 1024 * 1024,
            min_download_bytes: 1024 * 1024,
            required_files: &["ffmpeg.exe", "ffprobe.exe"],
            source_kind: RuntimeComponentSourceKind::ZipByFileName,
        });
    }

    specs.push(RuntimeComponentSpec {
        id: "yt-dlp",
        kind: "optional",
        env_url: "AUDRAFLOW_COMPONENT_YT_DLP_URL",
        default_url: yt_dlp_download_url().into(),
        download_size_bytes: 18 * 1024 * 1024,
        min_download_bytes: 1024 * 1024,
        required_files: yt_dlp_component_required_files(),
        source_kind: RuntimeComponentSourceKind::SingleFile {
            file_name: yt_dlp_binary_name(),
        },
    });

    specs
}

fn yt_dlp_component_required_files() -> &'static [&'static str] {
    if cfg!(windows) {
        YT_DLP_WINDOWS_REQUIRED_FILES
    } else {
        YT_DLP_UNIX_REQUIRED_FILES
    }
}

fn github_release_asset_url(asset_name: &str) -> String {
    let tag = std::env::var("AUDRAFLOW_COMPONENT_RELEASE_TAG")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("v{}", env!("CARGO_PKG_VERSION")));
    let base = std::env::var("AUDRAFLOW_COMPONENT_BASE_URL")
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("https://github.com/500wango/audraflow/releases/download/{tag}"));
    format!("{base}/{asset_name}")
}

fn runtime_component_download_url(spec: &RuntimeComponentSpec) -> Option<String> {
    std::env::var(spec.env_url)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| Some(spec.default_url.clone()))
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

fn runtime_components_root() -> PathBuf {
    runtime_app_data_dir().join("runtime").join("components")
}

fn runtime_component_dir(component_id: &str) -> PathBuf {
    runtime_components_root().join(component_id)
}

fn runtime_component_bin_dir(component_id: &str) -> PathBuf {
    runtime_component_dir(component_id).join("bin")
}

fn runtime_components_root_for_app(app_handle: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(app_handle
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("runtime")
        .join("components"))
}

fn runtime_component_dir_for_app(
    app_handle: &tauri::AppHandle,
    component_id: &str,
) -> Result<PathBuf, String> {
    Ok(runtime_components_root_for_app(app_handle)?.join(component_id))
}

fn find_runtime_component_tool(component_id: &str, file_name: &str) -> Option<PathBuf> {
    let path = runtime_component_bin_dir(component_id).join(file_name);
    path.is_file().then_some(path)
}

fn find_runtime_component_spec(id: &str) -> Option<RuntimeComponentSpec> {
    let normalized = normalize_runtime_component_id(id)?;
    runtime_component_specs()
        .into_iter()
        .find(|spec| spec.id == normalized)
}

fn normalize_runtime_component_id(id: &str) -> Option<&'static str> {
    match id.trim() {
        "whisper" | "whisperCli" | "whisper-cli" => Some("whisper"),
        "ffmpeg" | "ffprobe" => Some("ffmpeg"),
        "ytDlp" | "yt-dlp" | "ytdlp" => Some("yt-dlp"),
        _ => None,
    }
}

fn runtime_component_status(
    app_handle: &tauri::AppHandle,
    spec: &RuntimeComponentSpec,
) -> RuntimeComponentDto {
    let component_dir = runtime_component_dir_for_app(app_handle, spec.id)
        .unwrap_or_else(|_| runtime_component_dir(spec.id));
    let bin_dir = component_dir.join("bin");
    let missing = spec
        .required_files
        .iter()
        .filter(|file| !bin_dir.join(file).is_file())
        .copied()
        .collect::<Vec<_>>();
    let installed_size_bytes = directory_size_bytes(&component_dir).unwrap_or(0);
    let download_url = runtime_component_download_url(spec);
    let (status, detail) = if missing.is_empty() {
        (
            "ready".to_string(),
            Some(format!("Installed in {}", component_dir.display())),
        )
    } else {
        (
            "missing".to_string(),
            Some(format!("Missing file(s): {}", missing.join(", "))),
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

fn runtime_components(app_handle: &tauri::AppHandle) -> Vec<RuntimeComponentDto> {
    runtime_component_specs()
        .iter()
        .map(|spec| runtime_component_status(app_handle, spec))
        .collect()
}

fn emit_runtime_component_progress(
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

async fn install_runtime_component_by_id(
    app_handle: &tauri::AppHandle,
    id: &str,
) -> Result<String, String> {
    let spec = find_runtime_component_spec(id)
        .ok_or_else(|| format!("Unknown runtime component: {id}"))?;
    install_runtime_component(app_handle, &spec).await
}

async fn install_runtime_component(
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
    .await?;

    install_component_payload(spec, &download_path, &staging_bin_dir)?;
    verify_component_files(spec, &staging_bin_dir)?;
    for file_name in spec.required_files {
        mark_executable(&staging_bin_dir.join(file_name))?;
    }

    let _ = tokio::fs::remove_dir_all(&component_dir).await;
    tokio::fs::rename(&staging_dir, &component_dir)
        .await
        .map_err(|e| format!("Failed to activate runtime component: {e}"))?;
    let _ = tokio::fs::remove_file(&download_path).await;
    emit_runtime_component_progress(
        app_handle,
        spec.id,
        spec.download_size_bytes,
        spec.download_size_bytes,
        "Installed",
    );

    Ok(format!(
        "{} runtime component installed in {}.",
        spec.id,
        component_dir.display()
    ))
}

async fn delete_runtime_component_by_id(
    app_handle: &tauri::AppHandle,
    id: &str,
) -> Result<String, String> {
    let spec = find_runtime_component_spec(id)
        .ok_or_else(|| format!("Unknown runtime component: {id}"))?;
    let component_dir = runtime_component_dir_for_app(app_handle, spec.id)?;
    let before = directory_size_bytes(&component_dir).unwrap_or(0);
    let _ = tokio::fs::remove_dir_all(&component_dir).await;
    Ok(format!(
        "{} runtime component removed. Freed {}.",
        spec.id,
        format_file_size(before)
    ))
}

async fn download_url_to_path_with_progress(
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

    let tmp_path = destination.with_extension("tmp");
    let _ = tokio::fs::remove_file(&tmp_path).await;
    let response = reqwest::get(url)
        .await
        .map_err(|e| format!("Failed to download {url}: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("Runtime component download failed with {}", response.status()));
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

fn file_looks_like_html(path: &Path) -> Result<bool, String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("Failed to inspect downloaded runtime component: {e}"))?;
    let mut bytes = [0_u8; 512];
    let len = file
        .read(&mut bytes)
        .map_err(|e| format!("Failed to inspect downloaded runtime component: {e}"))?;
    Ok(looks_like_html(&bytes[..len]))
}

fn install_component_payload(
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
        RuntimeComponentSourceKind::ZipByFileName => {
            extract_required_files_from_zip(payload_path, staging_bin_dir, spec.required_files)
        }
    }
}

fn extract_required_files_from_zip(
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

fn zip_entry_basename(name: &str) -> Option<String> {
    name.replace('\\', "/")
        .rsplit('/')
        .find(|part| !part.is_empty())
        .map(str::to_string)
}

fn verify_component_files(spec: &RuntimeComponentSpec, bin_dir: &Path) -> Result<(), String> {
    let missing = spec
        .required_files
        .iter()
        .filter(|file| !bin_dir.join(file).is_file())
        .copied()
        .collect::<Vec<_>>();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Runtime component {} is missing required file(s): {}",
            spec.id,
            missing.join(", ")
        ))
    }
}
