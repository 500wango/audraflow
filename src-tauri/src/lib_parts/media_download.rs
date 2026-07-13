use crate::*;
pub(crate) async fn download_remote_media(
    app_handle: &tauri::AppHandle,
    client_job_id: &str,
    url: &str,
    skip_start_seconds: f64,
) -> Result<PathBuf, String> {
    emit_job_log(
        app_handle,
        client_job_id,
        "info",
        "Checking direct media URL",
    );
    emit_job_progress(
        app_handle,
        client_job_id,
        "import",
        5.0,
        "Checking direct media URL",
    );
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("Invalid URL: {e}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => return Err("Only http and https links are supported".into()),
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(REMOTE_MEDIA_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| e.to_string())?;

    let response = client
        .get(parsed)
        .send()
        .await
        .map_err(|e| format!("Failed to download URL: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Failed to download URL: {e}"))?;

    let content_length = response.content_length();
    if content_length.is_some_and(|len| len > MAX_REMOTE_MEDIA_BYTES) {
        return Err("Remote media is larger than the 2 GB limit".into());
    }
    if let Some(len) = content_length {
        emit_job_log(
            app_handle,
            client_job_id,
            "info",
            format!("Direct media size: {:.1} MB", len as f64 / 1024.0 / 1024.0),
        );
        emit_job_progress(
            app_handle,
            client_job_id,
            "download",
            10.0,
            format!(
                "Downloading direct media ({:.1} MB)",
                len as f64 / 1024.0 / 1024.0
            ),
        );
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string());

    if content_type
        .as_deref()
        .is_some_and(|value| value.to_ascii_lowercase().contains("text/html"))
    {
        return Err("URL returned an HTML page, not a direct media file".into());
    }

    let filename = filename_from_headers_or_url(url, content_type.as_deref())?;
    emit_job_log(
        app_handle,
        client_job_id,
        "info",
        format!("Downloading direct media as {filename}"),
    );
    if content_length.is_none() {
        emit_job_progress(
            app_handle,
            client_job_id,
            "download",
            10.0,
            "Downloading direct media",
        );
    }
    let cache_dir = app_handle
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("remote-media");
    std::fs::create_dir_all(&cache_dir).map_err(|e| format!("Failed to create cache dir: {e}"))?;

    let output_path = cache_dir.join(format!("{}-{filename}", uuid::Uuid::new_v4()));
    let mut file = std::fs::File::create(&output_path)
        .map_err(|e| format!("Failed to create downloaded file: {e}"))?;

    let mut downloaded = 0_u64;
    let mut stream = response.bytes_stream();
    use futures_util::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Failed while downloading URL: {e}"))?;
        downloaded = downloaded
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| "Remote media size overflowed".to_string())?;
        if downloaded > MAX_REMOTE_MEDIA_BYTES {
            let _ = std::fs::remove_file(&output_path);
            return Err("Remote media is larger than the 2 GB limit".into());
        }

        if let Some(total) = content_length {
            let pct = 10.0 + (downloaded as f64 / total as f64) * 70.0;
            emit_job_progress(
                app_handle,
                client_job_id,
                "download",
                pct,
                format!(
                    "Downloaded {:.1} / {:.1} MB",
                    downloaded as f64 / 1024.0 / 1024.0,
                    total as f64 / 1024.0 / 1024.0
                ),
            );
        } else {
            emit_job_progress(
                app_handle,
                client_job_id,
                "download",
                40.0,
                format!("Downloaded {:.1} MB", downloaded as f64 / 1024.0 / 1024.0),
            );
        }

        use std::io::Write;
        file.write_all(&chunk)
            .map_err(|e| format!("Failed to save downloaded file: {e}"))?;
    }

    if downloaded == 0 {
        let _ = std::fs::remove_file(&output_path);
        return Err("Remote media download was empty".into());
    }

    emit_job_log(
        app_handle,
        client_job_id,
        "info",
        format!(
            "Direct download complete: {:.1} MB",
            downloaded as f64 / 1024.0 / 1024.0
        ),
    );
    emit_job_progress(
        app_handle,
        client_job_id,
        "download",
        85.0,
        "Direct download complete",
    );
    trim_media_start_if_needed(app_handle, client_job_id, output_path, skip_start_seconds).await
}

pub(crate) async fn trim_media_start_if_needed(
    app_handle: &tauri::AppHandle,
    client_job_id: &str,
    input_path: PathBuf,
    skip_start_seconds: f64,
) -> Result<PathBuf, String> {
    if skip_start_seconds <= 0.0 {
        return Ok(input_path);
    }

    let skip_arg = format_seconds_arg(skip_start_seconds);
    emit_job_log(
        app_handle,
        client_job_id,
        "info",
        format!("Skipping first {skip_arg} seconds with ffmpeg"),
    );
    emit_job_progress(
        app_handle,
        client_job_id,
        "trim",
        86.0,
        format!("Skipping first {skip_arg} seconds"),
    );

    let output_path = input_path.with_file_name(format!(
        "{}-skip-{}.m4a",
        input_path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("media"),
        skip_arg.replace('.', "_")
    ));

    let mut ffmpeg = tokio::process::Command::new(ffmpeg_command_for_app(app_handle));
    apply_no_window_tokio(&mut ffmpeg);
    let output = ffmpeg
        .arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-ss")
        .arg(&skip_arg)
        .arg("-i")
        .arg(&input_path)
        .arg("-vn")
        .arg("-c:a")
        .arg("aac")
        .arg("-b:a")
        .arg("128k")
        .arg(&output_path)
        .output()
        .await
        .map_err(|e| format!("ffmpeg is required to skip the intro for direct media links: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "ffmpeg could not skip the intro: {}",
            trim_stderr(&output.stderr)
        ));
    }

    let size = std::fs::metadata(&output_path)
        .map_err(|e| format!("Failed to inspect trimmed media: {e}"))?
        .len();
    if size == 0 {
        let _ = std::fs::remove_file(&output_path);
        return Err("Trimmed media was empty after skipping the intro".into());
    }

    emit_job_log(
        app_handle,
        client_job_id,
        "info",
        format!(
            "Intro skipped: created {} ({:.1} MB)",
            output_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("trimmed media"),
            size as f64 / 1024.0 / 1024.0
        ),
    );
    emit_job_progress(
        app_handle,
        client_job_id,
        "trim",
        89.0,
        "Intro skip complete",
    );

    Ok(output_path)
}

pub(crate) async fn download_platform_media(
    app_handle: &tauri::AppHandle,
    client_job_id: &str,
    url: &str,
    audio_quality: &str,
    audio_format: &str,
    skip_start_seconds: f64,
) -> Result<PathBuf, String> {
    emit_job_log(
        app_handle,
        client_job_id,
        "info",
        "Resolving platform link with yt-dlp",
    );
    emit_job_progress(
        app_handle,
        client_job_id,
        "resolve",
        15.0,
        "Resolving platform link with yt-dlp",
    );
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("Invalid URL: {e}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => return Err("Only http and https links are supported".into()),
    }

    let cache_dir = app_handle
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("platform-media")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&cache_dir).map_err(|e| format!("Failed to create cache dir: {e}"))?;

    let audio_format = normalize_audio_format(audio_format);
    let audio_quality = normalize_audio_quality(audio_quality);
    let skip_arg = format_seconds_arg(skip_start_seconds);
    let output_template = cache_dir.join("media.%(ext)s");
    let mut command = tokio::process::Command::new(yt_dlp_command_for_app(app_handle));
    apply_no_window_tokio(&mut command);
    apply_yt_dlp_youtube_compat(&mut command);
    command
        .arg("--no-playlist")
        .arg("--newline")
        .arg("--windows-filenames")
        .arg("--max-filesize")
        .arg(format!("{}M", MAX_REMOTE_MEDIA_BYTES / 1024 / 1024))
        .arg("-f")
        .arg(yt_dlp_format_selector(audio_quality))
        .arg("-o")
        .arg(&output_template);

    if skip_start_seconds > 0.0 {
        command
            .arg("--download-sections")
            .arg(format!("*{skip_arg}-inf"))
            .arg("--force-keyframes-at-cuts");
        emit_job_log(
            app_handle,
            client_job_id,
            "info",
            format!("Skipping first {skip_arg} seconds before platform download"),
        );
    }

    if audio_format != "source" {
        command
            .arg("-x")
            .arg("--audio-format")
            .arg(audio_format)
            .arg("--audio-quality")
            .arg(yt_dlp_audio_quality_arg(audio_quality));
    } else {
        command.arg("--merge-output-format").arg("mp4");
    }

    command
        .arg(url)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1");

    emit_job_log(
        app_handle,
        client_job_id,
        "info",
        format!(
            "Starting platform download (quality: {}, format: {}, skip: {} sec)",
            audio_quality, audio_format, skip_arg
        ),
    );
    emit_job_progress(
        app_handle,
        client_job_id,
        "download",
        25.0,
        "Starting platform download",
    );
    let mut child = command
        .spawn()
        .map_err(|e| {
            format!(
                "yt-dlp is required for platform links but was not found: {e}. Download the yt-dlp runtime component in Settings or set AUDRAFLOW_YT_DLP_BIN to its executable path."
            )
        })?;

    let mut log_tasks = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        let app_handle = app_handle.clone();
        let client_job_id = client_job_id.to_string();
        log_tasks.push(tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim();
                if !line.is_empty() {
                    emit_job_log(&app_handle, &client_job_id, "info", line.to_string());
                    if let Some(pct) = parse_yt_dlp_progress(line) {
                        emit_job_progress(
                            &app_handle,
                            &client_job_id,
                            "download",
                            25.0 + pct * 0.55,
                            line.to_string(),
                        );
                    }
                }
            }
        }));
    }
    if let Some(stderr) = child.stderr.take() {
        let app_handle = app_handle.clone();
        let client_job_id = client_job_id.to_string();
        log_tasks.push(tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim();
                if !line.is_empty() {
                    let level = if line.to_ascii_lowercase().contains("error") {
                        "error"
                    } else {
                        "warn"
                    };
                    emit_job_log(&app_handle, &client_job_id, level, line.to_string());
                    if let Some(pct) = parse_yt_dlp_progress(line) {
                        emit_job_progress(
                            &app_handle,
                            &client_job_id,
                            "download",
                            25.0 + pct * 0.55,
                            line.to_string(),
                        );
                    }
                }
            }
        }));
    }

    let output = tokio::time::timeout(
        Duration::from_secs(PLATFORM_DOWNLOAD_TIMEOUT_SECS),
        child.wait(),
    )
    .await
    .map_err(|_| "Platform download timed out".to_string())?
    .map_err(|e| format!("Failed to run yt-dlp: {e}"))?;

    for task in log_tasks {
        let _ = task.await;
    }

    if !output.success() {
        let message = "yt-dlp could not download this link";
        emit_job_log(app_handle, client_job_id, "error", message);
        return Err(format!("Platform download failed: {message}"));
    }

    let mut candidates = Vec::new();
    for entry in
        std::fs::read_dir(&cache_dir).map_err(|e| format!("Failed to read cache dir: {e}"))?
    {
        let entry = entry.map_err(|e| format!("Failed to read downloaded file: {e}"))?;
        let path = entry.path();
        if path.is_file() && is_supported_media_path(&path) {
            let size = entry
                .metadata()
                .map_err(|e| format!("Failed to inspect downloaded file: {e}"))?
                .len();
            candidates.push((path, size));
        }
    }

    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.1));
    let (path, size) = candidates
        .into_iter()
        .next()
        .ok_or_else(|| "Platform link did not produce a supported audio/video file".to_string())?;

    if size == 0 {
        let _ = std::fs::remove_file(&path);
        return Err("Platform download was empty".into());
    }
    if size > MAX_REMOTE_MEDIA_BYTES {
        let _ = std::fs::remove_file(&path);
        return Err("Remote media is larger than the 2 GB limit".into());
    }

    emit_job_log(
        app_handle,
        client_job_id,
        "info",
        format!(
            "Platform download complete: {} ({:.1} MB)",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("media file"),
            size as f64 / 1024.0 / 1024.0
        ),
    );
    emit_job_progress(
        app_handle,
        client_job_id,
        "download",
        85.0,
        "Platform download complete",
    );
    Ok(path)
}

pub(crate) fn parse_yt_dlp_progress(line: &str) -> Option<f64> {
    let marker = "[download]";
    let text = line.strip_prefix(marker)?.trim_start();
    let pct_end = text.find('%')?;
    let pct_text = text[..pct_end].trim();
    pct_text.parse::<f64>().ok()
}

pub(crate) fn normalize_audio_quality(value: &str) -> &'static str {
    match value {
        "small" => "small",
        "medium" => "medium",
        "best" => "best",
        _ => "auto",
    }
}

pub(crate) fn normalize_audio_format(value: &str) -> &'static str {
    match value {
        "mp3" => "mp3",
        "m4a" => "m4a",
        "wav" => "wav",
        _ => "source",
    }
}

pub(crate) fn yt_dlp_format_selector(audio_quality: &str) -> &'static str {
    match audio_quality {
        "small" => "ba[abr<=64]/ba[filesize<20M]/worstaudio/best",
        "medium" | "auto" => "ba[abr<=128]/ba/bestaudio/best",
        "best" => "ba/bestaudio/best",
        _ => "ba/bestaudio/best",
    }
}

pub(crate) fn yt_dlp_audio_quality_arg(audio_quality: &str) -> &'static str {
    match audio_quality {
        "small" => "64K",
        "medium" | "auto" => "128K",
        "best" => "0",
        _ => "128K",
    }
}

pub(crate) async fn download_url_media(
    app_handle: &tauri::AppHandle,
    client_job_id: &str,
    url: &str,
    audio_quality: &str,
    audio_format: &str,
    skip_start_seconds: f64,
) -> Result<PathBuf, String> {
    match download_remote_media(app_handle, client_job_id, url, skip_start_seconds).await {
        Ok(path) => Ok(path),
        Err(direct_error) => {
            emit_job_log(
                app_handle,
                client_job_id,
                "warn",
                format!(
                    "Direct media download failed, trying platform resolver: {}",
                    direct_error
                ),
            );
            log::info!(
                "Direct media download failed; trying platform resolver: {}",
                direct_error
            );
            download_platform_media(
                app_handle,
                client_job_id,
                url,
                audio_quality,
                audio_format,
                skip_start_seconds,
            ).await.map_err(|platform_error| {
                format!("Direct download failed: {direct_error}; platform download failed: {platform_error}")
            })
        }
    }
}

pub(crate) async fn create_url_preview(
    app_handle: &tauri::AppHandle,
    url: &str,
    preview_seconds: f64,
) -> Result<UrlPreviewResponse, String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("Invalid URL: {e}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => return Err("Only http and https links are supported".into()),
    }

    let cache_dir = app_handle
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("url-previews")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| format!("Failed to create preview dir: {e}"))?;

    match create_platform_preview(app_handle, &cache_dir, url, preview_seconds).await {
        Ok(path) => Ok(UrlPreviewResponse {
            file_path: path.to_string_lossy().into_owned(),
            preview_seconds,
            source: "yt-dlp",
            message: format!(
                "Preview ready: first {} seconds resolved with yt-dlp",
                format_seconds_arg(preview_seconds)
            ),
        }),
        Err(platform_error) => {
            log::warn!("yt-dlp preview failed, trying ffmpeg: {}", platform_error);
            match create_direct_preview(app_handle, &cache_dir, url, preview_seconds).await {
                Ok(path) => Ok(UrlPreviewResponse {
                    file_path: path.to_string_lossy().into_owned(),
                    preview_seconds,
                    source: "ffmpeg",
                    message: format!(
                        "Preview ready: first {} seconds captured from direct media",
                        format_seconds_arg(preview_seconds)
                    ),
                }),
                Err(direct_error) => Err(format!(
                    "Could not create URL preview. yt-dlp failed: {platform_error}; ffmpeg failed: {direct_error}"
                )),
            }
        }
    }
}

pub(crate) async fn create_platform_preview(
    app_handle: &tauri::AppHandle,
    cache_dir: &Path,
    url: &str,
    preview_seconds: f64,
) -> Result<PathBuf, String> {
    let output_template = cache_dir.join("preview.%(ext)s");
    let section = format!("*0-{}", format_seconds_arg(preview_seconds));
    let output = tokio::time::timeout(Duration::from_secs(URL_PREVIEW_TIMEOUT_SECS), {
        let mut command = tokio::process::Command::new(yt_dlp_command_for_app(app_handle));
        apply_no_window_tokio(&mut command);
        apply_yt_dlp_youtube_compat(&mut command);
        command
            .arg("--no-playlist")
            .arg("--newline")
            .arg("--windows-filenames")
            .arg("--download-sections")
            .arg(section)
            .arg("--force-keyframes-at-cuts")
            .arg("-f")
            .arg("ba[abr<=128]/ba/bestaudio/best")
            .arg("-x")
            .arg("--audio-format")
            .arg("m4a")
            .arg("--audio-quality")
            .arg("128K")
            .arg("-o")
            .arg(&output_template)
            .arg(url)
            .env("PYTHONIOENCODING", "utf-8")
            .env("PYTHONUTF8", "1")
            .output()
    })
    .await
    .map_err(|_| "yt-dlp preview timed out".to_string())?
    .map_err(|e| format!("Failed to run yt-dlp: {e}"))?;

    if !output.status.success() {
        return Err(trim_stderr(&output.stderr));
    }

    find_preview_file(cache_dir)
}

pub(crate) async fn create_direct_preview(
    app_handle: &tauri::AppHandle,
    cache_dir: &Path,
    url: &str,
    preview_seconds: f64,
) -> Result<PathBuf, String> {
    let output_path = cache_dir.join("preview.m4a");
    let mut ffmpeg = tokio::process::Command::new(ffmpeg_command_for_app(app_handle));
    apply_no_window_tokio(&mut ffmpeg);
    let output = tokio::time::timeout(
        Duration::from_secs(URL_PREVIEW_TIMEOUT_SECS),
        ffmpeg
            .arg("-y")
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-i")
            .arg(url)
            .arg("-t")
            .arg(format_seconds_arg(preview_seconds))
            .arg("-vn")
            .arg("-c:a")
            .arg("aac")
            .arg("-b:a")
            .arg("128k")
            .arg(&output_path)
            .output(),
    )
    .await
    .map_err(|_| "ffmpeg preview timed out".to_string())?
    .map_err(|e| format!("Failed to run ffmpeg: {e}"))?;

    if !output.status.success() {
        return Err(trim_stderr(&output.stderr));
    }

    ensure_non_empty_preview(output_path)
}

pub(crate) fn find_preview_file(cache_dir: &Path) -> Result<PathBuf, String> {
    let mut candidates = Vec::new();
    for entry in
        std::fs::read_dir(cache_dir).map_err(|e| format!("Failed to read preview dir: {e}"))?
    {
        let entry = entry.map_err(|e| format!("Failed to inspect preview file: {e}"))?;
        let path = entry.path();
        if path.is_file() && is_supported_media_path(&path) {
            let size = entry
                .metadata()
                .map_err(|e| format!("Failed to inspect preview file: {e}"))?
                .len();
            candidates.push((path, size));
        }
    }

    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.1));
    let (path, size) = candidates
        .into_iter()
        .next()
        .ok_or_else(|| "Preview did not produce a supported audio file".to_string())?;
    ensure_non_empty_preview_with_size(path, size)
}

pub(crate) fn ensure_non_empty_preview(path: PathBuf) -> Result<PathBuf, String> {
    let size = std::fs::metadata(&path)
        .map_err(|e| format!("Failed to inspect preview file: {e}"))?
        .len();
    ensure_non_empty_preview_with_size(path, size)
}

pub(crate) fn ensure_non_empty_preview_with_size(path: PathBuf, size: u64) -> Result<PathBuf, String> {
    if size == 0 {
        let _ = std::fs::remove_file(&path);
        return Err("Preview media was empty".into());
    }
    Ok(path)
}
