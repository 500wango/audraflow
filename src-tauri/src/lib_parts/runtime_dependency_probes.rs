use crate::*;
pub(crate) fn probe_default_whisper_model(app_handle: &tauri::AppHandle) -> RuntimeDependencyDto {
    let id = "defaultWhisperModel";
    let repairable = runtime_dependency_repairable(id);
    let manager = match model_manager(app_handle) {
        Ok(manager) => manager,
        Err(error) => {
            return RuntimeDependencyDto {
                id: id.into(),
                status: "missing".into(),
                kind: "required".into(),
                path: None,
                version: None,
                detail: Some(error),
                repairable,
            };
        }
    };

    if let Err(error) = ensure_bundled_default_model(app_handle, &manager) {
        return RuntimeDependencyDto {
            id: id.into(),
            status: "warning".into(),
            kind: "required".into(),
            path: None,
            version: None,
            detail: Some(error),
            repairable,
        };
    }

    match manager.selected_model() {
        Ok(Some(model)) if is_whisper_cpp_model(&model) && model.path.is_file() => {
            RuntimeDependencyDto {
                id: id.into(),
                status: "ready".into(),
                kind: "required".into(),
                path: Some(model.path.to_string_lossy().into_owned()),
                version: Some(format!("{} {}", model.info.name, model.info.version)),
                detail: None,
                repairable,
            }
        }
        Ok(_) => RuntimeDependencyDto {
            id: id.into(),
            status: "missing".into(),
            kind: "required".into(),
            path: None,
            version: None,
            detail: Some("No selected local Whisper model was found.".into()),
            repairable,
        },
        Err(error) => RuntimeDependencyDto {
            id: id.into(),
            status: "warning".into(),
            kind: "required".into(),
            path: None,
            version: None,
            detail: Some(error.to_string()),
            repairable,
        },
    }
}

#[cfg(windows)]
pub(crate) fn probe_vc_redist() -> RuntimeDependencyDto {
    let id = "vcRedist";
    let missing = vc_redist_missing_files();
    if missing.is_empty() {
        RuntimeDependencyDto {
            id: id.into(),
            status: "ready".into(),
            kind: "required".into(),
            path: vc_redist_install_dir().map(|path| path.to_string_lossy().into_owned()),
            version: None,
            detail: Some("Microsoft Visual C++ Runtime x64 is available.".into()),
            repairable: runtime_dependency_repairable(id),
        }
    } else {
        RuntimeDependencyDto {
            id: id.into(),
            status: "missing".into(),
            kind: "required".into(),
            path: vc_redist_install_dir().map(|path| path.to_string_lossy().into_owned()),
            version: None,
            detail: Some(format!("Missing DLL(s): {}", missing.join(", "))),
            repairable: runtime_dependency_repairable(id),
        }
    }
}

pub(crate) async fn probe_runtime_command(
    id: &str,
    kind: &str,
    program: PathBuf,
    args: &[&str],
    display_path: Option<String>,
    timeout_secs: u64,
) -> RuntimeDependencyDto {
    let display_path = display_path.unwrap_or_else(|| command_display_path(&program));
    // Relative names without a directory only work if they are on PATH. Prefer an
    // absolute path when the binary exists, so probes report accurate missing vs
    // broken states after managed-component installs.
    if program.components().count() == 1 && !program.exists() {
        return RuntimeDependencyDto {
            id: id.into(),
            status: "missing".into(),
            kind: kind.into(),
            path: None,
            version: None,
            detail: Some(format!(
                "{} was not found. Install it from Runtime Components, or place it on PATH.",
                program.display()
            )),
            repairable: runtime_dependency_repairable(id),
        };
    }
    if program.is_absolute() && !program.is_file() {
        return RuntimeDependencyDto {
            id: id.into(),
            status: "missing".into(),
            kind: kind.into(),
            path: Some(display_path),
            version: None,
            detail: Some(format!("Binary not found at {}", program.display())),
            repairable: runtime_dependency_repairable(id),
        };
    }

    unblock_windows_file(&program);
    let mut command = tokio::process::Command::new(&program);
    command.args(args);
    apply_no_window_tokio(&mut command);

    match tokio::time::timeout(Duration::from_secs(timeout_secs), command.output()).await {
        Ok(Ok(output)) if output.status.success() => RuntimeDependencyDto {
            id: id.into(),
            status: "ready".into(),
            kind: kind.into(),
            path: Some(display_path),
            version: first_output_line(&output.stdout)
                .or_else(|| first_output_line(&output.stderr)),
            detail: None,
            repairable: runtime_dependency_repairable(id),
        },
        Ok(Ok(output)) => RuntimeDependencyDto {
            id: id.into(),
            status: "warning".into(),
            kind: kind.into(),
            path: Some(display_path),
            version: first_output_line(&output.stdout)
                .or_else(|| first_output_line(&output.stderr)),
            detail: Some(format!(
                "Probe exited with {}. {}",
                output.status,
                short_output(&output.stderr)
                    .or_else(|| short_output(&output.stdout))
                    .unwrap_or_else(|| "No output.".into())
            )),
            repairable: runtime_dependency_repairable(id),
        },
        Ok(Err(error)) => RuntimeDependencyDto {
            id: id.into(),
            status: "missing".into(),
            kind: kind.into(),
            path: Some(display_path),
            version: None,
            detail: Some(format!(
                "Failed to start {}: {error}",
                program.display()
            )),
            repairable: runtime_dependency_repairable(id),
        },
        Err(_) => RuntimeDependencyDto {
            id: id.into(),
            status: "warning".into(),
            kind: kind.into(),
            path: Some(display_path),
            version: None,
            detail: Some(format!("Probe timed out after {timeout_secs}s.")),
            repairable: runtime_dependency_repairable(id),
        },
    }
}

pub(crate) async fn probe_funasr_cli(app_handle: &tauri::AppHandle) -> RuntimeDependencyDto {
    let id = "funasrCli";
    let program = funasr_cli_command_for_app(app_handle);
    let display_path = command_display_path(&program);
    if program.components().count() == 1 && !program.exists() {
        return RuntimeDependencyDto {
            id: id.into(),
            status: "missing".into(),
            kind: "experimental".into(),
            path: None,
            version: None,
            detail: Some(
                "Fun-ASR CLI was not found. Install it from Runtime Components.".into(),
            ),
            repairable: runtime_dependency_repairable(id),
        };
    }
    if program.is_absolute() && !program.is_file() {
        return RuntimeDependencyDto {
            id: id.into(),
            status: "missing".into(),
            kind: "experimental".into(),
            path: Some(display_path),
            version: None,
            detail: Some(format!("Binary not found at {}", program.display())),
            repairable: runtime_dependency_repairable(id),
        };
    }
    unblock_windows_file(&program);
    let mut command = tokio::process::Command::new(&program);
    command.arg("--help");
    apply_no_window_tokio(&mut command);

    match tokio::time::timeout(Duration::from_secs(5), command.output()).await {
        Ok(Ok(output)) if output.status.success() || output_looks_like_funasr_usage(&output) => {
            RuntimeDependencyDto {
                id: id.into(),
                status: "ready".into(),
                kind: "experimental".into(),
                path: Some(display_path),
                version: first_output_line(&output.stdout)
                    .or_else(|| first_output_line(&output.stderr)),
                detail: None,
                repairable: runtime_dependency_repairable(id),
            }
        }
        Ok(Ok(output)) => RuntimeDependencyDto {
            id: id.into(),
            status: "warning".into(),
            kind: "experimental".into(),
            path: Some(display_path),
            version: first_output_line(&output.stdout)
                .or_else(|| first_output_line(&output.stderr)),
            detail: Some(format!(
                "Probe exited with {}. {}",
                output.status,
                short_output(&output.stderr)
                    .or_else(|| short_output(&output.stdout))
                    .unwrap_or_else(|| "No output.".into())
            )),
            repairable: runtime_dependency_repairable(id),
        },
        Ok(Err(error)) => RuntimeDependencyDto {
            id: id.into(),
            status: "missing".into(),
            kind: "experimental".into(),
            path: None,
            version: None,
            detail: Some(error.to_string()),
            repairable: runtime_dependency_repairable(id),
        },
        Err(_) => RuntimeDependencyDto {
            id: id.into(),
            status: "warning".into(),
            kind: "experimental".into(),
            path: Some(display_path),
            version: None,
            detail: Some("Probe timed out after 5s.".into()),
            repairable: runtime_dependency_repairable(id),
        },
    }
}

pub(crate) async fn funasr_cli_probe_succeeds(program: &Path) -> bool {
    if !program.is_file() {
        return false;
    }
    unblock_windows_file(program);
    let mut command = tokio::process::Command::new(program);
    command.arg("--help");
    apply_no_window_tokio(&mut command);
    let Ok(result) = tokio::time::timeout(Duration::from_secs(5), command.output()).await else {
        return false;
    };
    let Ok(output) = result else {
        return false;
    };
    output.status.success() || output_looks_like_funasr_usage(&output)
}

pub(crate) fn output_looks_like_funasr_usage(output: &std::process::Output) -> bool {
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    text_looks_like_funasr_usage(&text)
}

pub(crate) fn text_looks_like_funasr_usage(text: &str) -> bool {
    let normalized = text.to_ascii_lowercase();
    normalized.contains("llama-funasr-cli")
        && normalized.contains("--enc")
        && normalized.contains("-a audio")
}

pub(crate) async fn probe_sensevoice_python() -> RuntimeDependencyDto {
    let Some(invocation) = resolve_python_invocation() else {
        return RuntimeDependencyDto {
            id: "sensevoicePython".into(),
            status: "missing".into(),
            kind: "optional".into(),
            path: None,
            version: None,
            detail: Some("Python was not found.".into()),
            repairable: runtime_dependency_repairable("sensevoicePython"),
        };
    };

    let script = "import funasr, modelscope; print('funasr/modelscope ready')";
    let mut args = invocation.base_args.clone();
    args.push("-c".into());
    args.push(script.into());
    let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();

    probe_runtime_command(
        "sensevoicePython",
        "optional",
        invocation.program,
        &arg_refs,
        Some(invocation.display),
        15,
    )
    .await
}

pub(crate) async fn probe_demucs() -> RuntimeDependencyDto {
    let Some(invocation) = resolve_demucs_invocation_for_health() else {
        return RuntimeDependencyDto {
            id: "demucs".into(),
            status: "missing".into(),
            kind: "optional".into(),
            path: None,
            version: None,
            detail: Some("Demucs was not found.".into()),
            repairable: runtime_dependency_repairable("demucs"),
        };
    };

    let mut args = invocation.base_args;
    args.push("--help".into());
    let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();

    probe_runtime_command(
        "demucs",
        "optional",
        invocation.program,
        &arg_refs,
        Some(invocation.display),
        8,
    )
    .await
}

pub(crate) fn probe_funasr_models(app_handle: &tauri::AppHandle) -> RuntimeDependencyDto {
    match resolve_funasr_model_paths(app_handle) {
        Ok(paths) => RuntimeDependencyDto {
            id: "funasrModels".into(),
            status: "ready".into(),
            kind: "experimental".into(),
            path: paths
                .model_dir
                .map(|path| path.to_string_lossy().into_owned()),
            version: None,
            detail: Some(format!(
                "encoder={}, llm={}, vad={}",
                paths.encoder_path.display(),
                paths.llm_path.display(),
                paths
                    .vad_path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "not found".into())
            )),
            repairable: runtime_dependency_repairable("funasrModels"),
        },
        Err(error) => RuntimeDependencyDto {
            id: "funasrModels".into(),
            status: "missing".into(),
            kind: "experimental".into(),
            path: None,
            version: None,
            detail: Some(error),
            repairable: runtime_dependency_repairable("funasrModels"),
        },
    }
}

pub(crate) async fn repair_runtime_dependency(
    app_handle: tauri::AppHandle,
    id: &str,
) -> Result<RuntimeRepairResultDto, String> {
    let normalized = id.trim();
    let message = match normalized {
        "defaultWhisperModel" => repair_default_whisper_model(&app_handle).await?,
        "vcRedist" => install_runtime_component_by_id(&app_handle, "vc-redist").await?,
        "whisperCli" => {
            let mut messages = Vec::new();
            if let Some(message) = repair_vc_redist_if_missing(&app_handle).await? {
                messages.push(message);
            }
            messages.push(install_runtime_component_by_id(&app_handle, "whisper").await?);
            messages.join(" ")
        }
        "ffmpeg" | "ffprobe" => install_runtime_component_by_id(&app_handle, "ffmpeg").await?,
        "funasrCli" => {
            let vc_redist_message = repair_vc_redist_if_missing(&app_handle).await?;
            let command = funasr_cli_command_for_app(&app_handle);
            if funasr_cli_probe_succeeds(&command).await {
                let ready_message = format!("Fun-ASR CLI is already usable: {}", command.display());
                if let Some(message) = vc_redist_message {
                    format!("{message} {ready_message}")
                } else {
                    ready_message
                }
            } else {
                let install_message = install_runtime_component_by_id(&app_handle, "funasr").await?;
                if let Some(message) = vc_redist_message {
                    format!("{message} {install_message}")
                } else {
                    install_message
                }
            }
        }
        "ytDlp" => install_runtime_component_by_id(&app_handle, "yt-dlp").await?,
        "sensevoicePython" => {
            repair_python_packages(
                "SenseVoice Python packages",
                &["funasr", "modelscope"],
                Duration::from_secs(30 * 60),
            )
            .await?
        }
        "demucs" => {
            repair_python_packages(
                "Demucs",
                &["demucs", "torchcodec"],
                Duration::from_secs(30 * 60),
            )
            .await?
        }
        _ => {
            return Err(format!(
                "Runtime dependency cannot be repaired automatically: {id}"
            ))
        }
    };

    let health = runtime_health(&app_handle).await;
    ensure_repair_succeeded(&health, normalized)?;

    Ok(RuntimeRepairResultDto {
        id: normalized.into(),
        message,
        components: runtime_components(&app_handle),
        health,
    })
}

pub(crate) async fn repair_vc_redist_if_missing(app_handle: &tauri::AppHandle) -> Result<Option<String>, String> {
    if vc_redist_missing_files().is_empty() {
        Ok(None)
    } else {
        install_runtime_component_by_id(app_handle, "vc-redist")
            .await
            .map(Some)
    }
}

pub(crate) fn ensure_repair_succeeded(health: &RuntimeHealthDto, id: &str) -> Result<(), String> {
    let targets = repair_validation_targets(id);
    for target in targets {
        let Some(item) = health.items.iter().find(|item| item.id == target) else {
            return Err(format!("Repair did not return a health item for {target}."));
        };
        if item.status != "ready" {
            return Err(format!(
                "Repair completed, but {} is still {}. {}",
                runtime_dependency_label_for_error(&item.id),
                item.status,
                item.detail
                    .as_deref()
                    .unwrap_or("Refresh runtime diagnostics for details.")
            ));
        }
    }
    Ok(())
}

pub(crate) fn repair_validation_targets(id: &str) -> Vec<&'static str> {
    match id {
        "ffmpeg" | "ffprobe" => vec!["ffmpeg", "ffprobe"],
        "vcRedist" => vec!["vcRedist"],
        "whisperCli" => vec!["whisperCli"],
        "ytDlp" => vec!["ytDlp"],
        "funasrCli" => vec!["funasrCli"],
        "sensevoicePython" => vec!["sensevoicePython"],
        "demucs" => vec!["demucs"],
        "defaultWhisperModel" => vec!["defaultWhisperModel"],
        _ => vec![],
    }
}

pub(crate) fn runtime_dependency_label_for_error(id: &str) -> &str {
    match id {
        "defaultWhisperModel" => "Whisper model",
        "vcRedist" => "Microsoft VC++ Runtime",
        "whisperCli" => "Whisper CLI",
        "ffmpeg" => "FFmpeg",
        "ffprobe" => "FFprobe",
        "sensevoicePython" => "SenseVoice Python packages",
        "ytDlp" => "yt-dlp",
        "demucs" => "Demucs",
        "funasrCli" => "Fun-ASR CLI",
        "funasrModels" => "Fun-ASR GGUF models",
        _ => "Runtime dependency",
    }
}

pub(crate) async fn repair_default_whisper_model(app_handle: &tauri::AppHandle) -> Result<String, String> {
    let manager = model_manager(app_handle)?;
    if let Some(installed) = ensure_bundled_default_model(app_handle, &manager)? {
        match manager.selected_model() {
            Ok(Some(model)) if is_whisper_cpp_model(&model) && model.path.is_file() => {
                return Ok(format!("Whisper model is ready: {}", model.path.display()));
            }
            Ok(_) => {
                manager
                    .select_model(&installed.info.name, &installed.info.version)
                    .map_err(|e| e.to_string())?;
                return Ok(format!(
                    "Default Whisper model was selected: {}",
                    installed.path.display()
                ));
            }
            Err(error) => {
                manager
                    .select_model(&installed.info.name, &installed.info.version)
                    .map_err(|e| e.to_string())?;
                return Ok(format!(
                    "Default Whisper model was selected after repairing the previous selection ({error}): {}",
                    installed.path.display(),
                ));
            }
        }
    }

    let mut info = bundled_default_model_info();
    info.download_url = format!(
        "{WHISPER_CPP_MODEL_BASE_URL}/{WHISPER_CPP_MODEL_COMMIT}/ggml-{DEFAULT_WHISPER_MODEL_NAME}.bin"
    );
    let download_id = "defaultWhisperModel";
    emit_model_download_progress(
        app_handle,
        download_id,
        0,
        info.size_bytes,
        "Downloading default Whisper model",
    );

    let app_for_progress = app_handle.clone();
    let manager_for_download = manager;
    let info_for_download = info.clone();
    tokio::task::spawn_blocking(move || {
        manager_for_download.download(&info_for_download, |downloaded, total| {
            emit_model_download_progress(
                &app_for_progress,
                download_id,
                downloaded,
                total,
                format!(
                    "Downloaded {} / {}",
                    format_file_size(downloaded),
                    format_file_size(total)
                ),
            );
        })?;
        manager_for_download.select_model(&info_for_download.name, &info_for_download.version)?;
        anyhow::Ok(())
    })
    .await
    .map_err(|e| format!("Default model repair task failed: {e}"))?
    .map_err(|e| e.to_string())?;

    Ok("Default Whisper model was downloaded and selected.".into())
}

pub(crate) fn looks_like_html(bytes: &[u8]) -> bool {
    let sample_len = bytes.len().min(512);
    let sample = String::from_utf8_lossy(&bytes[..sample_len])
        .trim_start()
        .to_ascii_lowercase();
    sample.starts_with("<!doctype html") || sample.starts_with("<html")
}

pub(crate) fn mark_executable(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path)
            .map_err(|e| format!("Failed to inspect downloaded executable: {e}"))?
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions)
            .map_err(|e| format!("Failed to mark downloaded executable: {e}"))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

pub(crate) async fn repair_python_packages(
    label: &str,
    packages: &[&str],
    timeout: Duration,
) -> Result<String, String> {
    let invocation = ensure_managed_python_venv(label).await?;
    let mut args = invocation.base_args.clone();
    args.extend([
        "-m".into(),
        "pip".into(),
        "install".into(),
        "-U".into(),
    ]);
    args.extend(packages.iter().map(|package| (*package).to_string()));

    let output = run_runtime_invocation_with_timeout(&invocation, &args, timeout, label).await?;

    if !output.status.success() {
        return Err(format!(
            "{label} repair failed: {}",
            short_output(&output.stderr)
                .or_else(|| short_output(&output.stdout))
                .unwrap_or_else(|| "No output.".into())
        ));
    }

    Ok(format!(
        "{label} installed or updated in AudraFlow's isolated Python environment."
    ))
}

pub(crate) async fn ensure_managed_python_venv(label: &str) -> Result<RuntimeInvocation, String> {
    if let Some(invocation) = find_managed_python_invocation() {
        return Ok(invocation);
    }

    let base = resolve_system_python_invocation().ok_or_else(|| {
        format!(
            "{label} repair requires Python 3. Install Python 3 first or set AUDRAFLOW_PYTHON_BIN."
        )
    })?;
    let venv_dir = runtime_component_dir("python-venv");
    if let Some(parent) = venv_dir.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create Python runtime directory: {e}"))?;
    }

    let mut create_args = base.base_args.clone();
    create_args.extend([
        "-m".into(),
        "venv".into(),
        venv_dir.to_string_lossy().into_owned(),
    ]);
    let output = run_runtime_invocation_with_timeout(
        &base,
        &create_args,
        Duration::from_secs(5 * 60),
        "Python venv creation",
    )
    .await?;
    if !output.status.success() {
        return Err(format!(
            "Failed to create AudraFlow Python environment: {}",
            short_output(&output.stderr)
                .or_else(|| short_output(&output.stdout))
                .unwrap_or_else(|| "No output.".into())
        ));
    }

    let invocation = find_managed_python_invocation().ok_or_else(|| {
        format!(
            "Python venv was created but the interpreter was not found at {}",
            managed_python_bin().display()
        )
    })?;
    let bootstrap_args = vec![
        "-m".into(),
        "pip".into(),
        "install".into(),
        "-U".into(),
        "pip".into(),
        "setuptools".into(),
        "wheel".into(),
    ];
    let output = run_runtime_invocation_with_timeout(
        &invocation,
        &bootstrap_args,
        Duration::from_secs(10 * 60),
        "Python package bootstrap",
    )
    .await?;
    if !output.status.success() {
        return Err(format!(
            "Failed to bootstrap AudraFlow Python environment: {}",
            short_output(&output.stderr)
                .or_else(|| short_output(&output.stdout))
                .unwrap_or_else(|| "No output.".into())
        ));
    }

    Ok(invocation)
}
