use crate::*;
#[tauri::command]
pub(crate) async fn cmd_record_telemetry_event(
    app_handle: tauri::AppHandle,
    request: TelemetryEventRequest,
) -> Result<(), String> {
    record_local_telemetry(&app_handle, request)
}

#[tauri::command]
pub(crate) async fn cmd_get_telemetry_consent(
    app_handle: tauri::AppHandle,
) -> Result<TelemetryConsentState, String> {
    read_telemetry_consent(&app_handle)
}

#[tauri::command]
pub(crate) async fn cmd_set_telemetry_consent(
    app_handle: tauri::AppHandle,
    request: SetTelemetryConsentRequest,
) -> Result<TelemetryConsentState, String> {
    write_telemetry_consent(&app_handle, request.enabled)
}

#[tauri::command]
pub(crate) async fn cmd_clear_local_history(
    app_handle: tauri::AppHandle,
) -> Result<PrivacyActionResult, String> {
    let db_path = storage_db_path()?;
    let telemetry_path = telemetry_events_path(&app_handle)?;
    let before = file_size_or_zero(&db_path) + file_size_or_zero(&telemetry_path);
    let storage = audraflow_storage::Storage::open(&db_path).map_err(|e| e.to_string())?;
    storage.clear_job_history().map_err(|e| e.to_string())?;
    if telemetry_path.exists() {
        std::fs::remove_file(&telemetry_path).map_err(|e| e.to_string())?;
    }
    let after = file_size_or_zero(&db_path);

    Ok(PrivacyActionResult {
        message: "Local job history and telemetry events were cleared.".into(),
        bytes_freed: before.saturating_sub(after),
        items_affected: 1,
    })
}

#[tauri::command]
pub(crate) async fn cmd_delete_model_cache(
    app_handle: tauri::AppHandle,
) -> Result<PrivacyActionResult, String> {
    let model_dir = model_cache_dir(&app_handle)?;
    let before = directory_size_bytes(&model_dir)?;
    let removed = clear_directory_children(&model_dir)?;

    Ok(PrivacyActionResult {
        message: "Model cache was cleared.".into(),
        bytes_freed: before,
        items_affected: removed,
    })
}

#[tauri::command]
pub(crate) async fn cmd_get_model_settings(app_handle: tauri::AppHandle) -> Result<ModelSettingsDto, String> {
    model_settings(&app_handle)
}

#[tauri::command]
pub(crate) async fn cmd_get_model_catalog(
    app_handle: tauri::AppHandle,
) -> Result<Vec<ModelCatalogEntryDto>, String> {
    builtin_model_catalog(&app_handle)
}

#[tauri::command]
pub(crate) async fn cmd_import_local_model(
    app_handle: tauri::AppHandle,
    request: ImportLocalModelRequest,
) -> Result<ModelSettingsDto, String> {
    let app_handle_clone = app_handle.clone();
    tokio::task::spawn_blocking(move || import_local_model(&app_handle_clone, request))
        .await
        .map_err(|e| format!("Import task panicked: {e}"))?
}

#[tauri::command]
pub(crate) async fn cmd_download_model(
    app_handle: tauri::AppHandle,
    request: DownloadModelRequest,
) -> Result<ModelActionResult, String> {
    download_model(app_handle, request).await
}

#[tauri::command]
pub(crate) async fn cmd_select_model(
    app_handle: tauri::AppHandle,
    request: SelectModelRequest,
) -> Result<ModelSettingsDto, String> {
    let manager = model_manager(&app_handle)?;
    manager
        .select_model(&request.name, &request.version)
        .map_err(|e| e.to_string())?;
    model_settings(&app_handle)
}

#[tauri::command]
pub(crate) async fn cmd_delete_model(
    app_handle: tauri::AppHandle,
    request: DeleteModelRequest,
) -> Result<ModelActionResult, String> {
    let manager = model_manager(&app_handle)?;
    let installed = manager
        .list_installed_models()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|model| model.info.name == request.name && model.info.version == request.version)
        .ok_or_else(|| {
            format!(
                "Model is not installed: {} v{}",
                request.name, request.version
            )
        })?;
    let bytes_freed = directory_size_bytes(installed.path.parent().unwrap_or(&installed.path))
        .unwrap_or_else(|_| file_size_or_zero(&installed.path));

    manager
        .remove_model(&request.name, &request.version)
        .map_err(|e| e.to_string())?;

    Ok(ModelActionResult {
        message: format!("Deleted {} v{}.", request.name, request.version),
        bytes_freed,
        items_affected: 1,
        settings: model_settings(&app_handle)?,
    })
}

#[tauri::command]
pub(crate) async fn cmd_clear_unused_models(
    app_handle: tauri::AppHandle,
) -> Result<ModelActionResult, String> {
    let manager = model_manager(&app_handle)?;
    let selected = manager.selected_model().map_err(|e| e.to_string())?;
    let installed = manager.list_installed_models().map_err(|e| e.to_string())?;
    let mut bytes_freed = 0u64;
    let mut removed = 0u64;

    for model in installed {
        let is_selected = selected.as_ref().is_some_and(|selected| {
            selected.info.name == model.info.name && selected.info.version == model.info.version
        });
        if is_selected {
            continue;
        }

        bytes_freed = bytes_freed.saturating_add(
            directory_size_bytes(model.path.parent().unwrap_or(&model.path))
                .unwrap_or_else(|_| file_size_or_zero(&model.path)),
        );
        manager
            .remove_model(&model.info.name, &model.info.version)
            .map_err(|e| e.to_string())?;
        removed += 1;
    }

    Ok(ModelActionResult {
        message: if removed == 0 {
            "No unused models to clear.".into()
        } else {
            format!("Cleared {removed} unused model(s).")
        },
        bytes_freed,
        items_affected: removed,
        settings: model_settings(&app_handle)?,
    })
}

#[tauri::command]
pub(crate) async fn cmd_get_diagnostics_preview(
    app_handle: tauri::AppHandle,
) -> Result<DiagnosticsPreview, String> {
    diagnostics_preview(&app_handle)
}

#[tauri::command]
pub(crate) async fn cmd_get_device_diagnostics() -> Result<DeviceDiagnosticsDto, String> {
    Ok(detect_device_diagnostics())
}

#[tauri::command]
pub(crate) async fn cmd_get_runtime_health(app_handle: tauri::AppHandle) -> Result<RuntimeHealthDto, String> {
    Ok(runtime_health(&app_handle).await)
}

#[tauri::command]
pub(crate) async fn cmd_get_runtime_components(
    app_handle: tauri::AppHandle,
) -> Result<Vec<RuntimeComponentDto>, String> {
    Ok(runtime_components(&app_handle))
}

#[tauri::command]
pub(crate) async fn cmd_download_runtime_component(
    app_handle: tauri::AppHandle,
    id: String,
) -> Result<RuntimeComponentActionResultDto, String> {
    let normalized = normalize_runtime_component_id(&id)
        .ok_or_else(|| format!("Unknown runtime component: {id}"))?;
    let message = install_runtime_component_by_id(&app_handle, normalized).await?;
    Ok(RuntimeComponentActionResultDto {
        id: normalized.into(),
        message,
        components: runtime_components(&app_handle),
        health: runtime_health(&app_handle).await,
    })
}

#[tauri::command]
pub(crate) async fn cmd_delete_runtime_component(
    app_handle: tauri::AppHandle,
    id: String,
) -> Result<RuntimeComponentActionResultDto, String> {
    let normalized = normalize_runtime_component_id(&id)
        .ok_or_else(|| format!("Unknown runtime component: {id}"))?;
    let message = delete_runtime_component_by_id(&app_handle, normalized).await?;
    Ok(RuntimeComponentActionResultDto {
        id: normalized.into(),
        message,
        components: runtime_components(&app_handle),
        health: runtime_health(&app_handle).await,
    })
}

#[tauri::command]
pub(crate) async fn cmd_repair_runtime_dependency(
    app_handle: tauri::AppHandle,
    id: String,
) -> Result<RuntimeRepairResultDto, String> {
    repair_runtime_dependency(app_handle, &id).await
}

#[tauri::command]
pub(crate) async fn cmd_export_diagnostics_package(app_handle: tauri::AppHandle) -> Result<String, String> {
    let preview = diagnostics_preview(&app_handle)?;
    let payload = serde_json::json!({
        "app_version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "telemetry_enabled": preview.telemetry_enabled,
        "local_history_bytes": preview.local_history_bytes,
        "telemetry_events_bytes": preview.telemetry_events_bytes,
        "model_cache_bytes": preview.model_cache_bytes,
        "model_cache_items": preview.model_cache_items,
        "fields": preview.fields,
        "generated_at_ms": now_unix_ms(),
    });
    let output_dir = app_handle
        .path()
        .download_dir()
        .or_else(|_| app_handle.path().app_data_dir())
        .map_err(|e| e.to_string())?
        .join("AudraFlow");
    std::fs::create_dir_all(&output_dir).map_err(|e| e.to_string())?;
    let output_path = output_dir.join(format!("diagnostics-{}.json", now_unix_ms()));
    let json = serde_json::to_vec_pretty(&payload).map_err(|e| e.to_string())?;
    std::fs::write(&output_path, json).map_err(|e| e.to_string())?;
    Ok(output_path.to_string_lossy().into_owned())
}
