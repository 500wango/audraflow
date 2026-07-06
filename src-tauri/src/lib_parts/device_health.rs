fn detect_nvidia_device() -> Option<(String, f64, String, String)> {
    let output = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,memory.total,cuda_version,driver_version",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let line = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?
        .to_string();
    let parts = line
        .split(',')
        .map(str::trim)
        .map(str::to_string)
        .collect::<Vec<_>>();
    if parts.len() < 4 {
        return None;
    }
    let vram_gb = parts[1].parse::<f64>().ok()? / 1024.0;
    Some((
        parts[0].clone(),
        vram_gb,
        parts[2].clone(),
        parts[3].clone(),
    ))
}

fn detect_device_diagnostics() -> DeviceDiagnosticsDto {
    let cpu_cores = num_cpus::get() as u32;
    let nvidia = detect_nvidia_device();
    let cuda_available = nvidia.is_some();
    let vram_gb = nvidia.as_ref().map(|(_, vram, _, _)| *vram);
    let device_tier = DeviceTier::classify(cuda_available, vram_gb, cpu_cores);
    let fallback_message = if cuda_available {
        None
    } else {
        Some("CUDA GPU was not detected; transcription will use CPU fallback.".into())
    };

    DeviceDiagnosticsDto {
        cpu_cores,
        cuda_available,
        vram_gb,
        gpu_model: nvidia.as_ref().map(|(name, _, _, _)| name.clone()),
        cuda_version: nvidia.as_ref().map(|(_, _, cuda, _)| cuda.clone()),
        driver_version: nvidia.as_ref().map(|(_, _, _, driver)| driver.clone()),
        device_tier: format!("{:?}", device_tier),
        fallback_message,
    }
}

async fn runtime_health(app_handle: &tauri::AppHandle) -> RuntimeHealthDto {
    let mut items = vec![
        probe_default_whisper_model(app_handle),
        probe_runtime_command(
            "whisperCli",
            "required",
            whisper_cli_command(),
            &["--help"],
            None,
            5,
        )
        .await,
        probe_runtime_command(
            "ffmpeg",
            "required",
            ffmpeg_command(),
            &["-version"],
            None,
            5,
        )
        .await,
        probe_runtime_command(
            "ffprobe",
            "required",
            ffprobe_command(),
            &["-version"],
            None,
            5,
        )
        .await,
        probe_sensevoice_python().await,
        probe_runtime_command(
            "ytDlp",
            "optional",
            yt_dlp_command(),
            &["--version"],
            None,
            5,
        )
        .await,
        probe_demucs().await,
        probe_runtime_command(
            "funasrCli",
            "experimental",
            funasr_cli_command(),
            &["--help"],
            None,
            5,
        )
        .await,
        probe_funasr_models(app_handle),
    ];

    items.sort_by_key(|item| runtime_dependency_sort_key(&item.id));

    let blocking_count = items
        .iter()
        .filter(|item| item.kind == "required" && item.status != "ready")
        .count() as u32;
    let warning_count = items
        .iter()
        .filter(|item| item.kind != "required" && item.status != "ready")
        .count() as u32;

    RuntimeHealthDto {
        generated_at_ms: now_unix_ms(),
        blocking_count,
        warning_count,
        items,
    }
}

fn runtime_dependency_sort_key(id: &str) -> u8 {
    match id {
        "defaultWhisperModel" => 0,
        "whisperCli" => 1,
        "ffmpeg" => 2,
        "ffprobe" => 3,
        "sensevoicePython" => 4,
        "ytDlp" => 5,
        "demucs" => 6,
        "funasrCli" => 7,
        "funasrModels" => 8,
        _ => u8::MAX,
    }
}

fn runtime_dependency_repairable(id: &str) -> bool {
    matches!(
        id,
        "defaultWhisperModel"
            | "whisperCli"
            | "ffmpeg"
            | "ffprobe"
            | "ytDlp"
            | "sensevoicePython"
            | "demucs"
    )
}
