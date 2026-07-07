use crate::*;
/// Estimate processing time using the adaptive scheduler.
/// Called when user toggles "极致准确" to show estimated duration change.
///
/// Input: audio duration (seconds), extreme accuracy flag.
/// Output: scheduler plan with estimated seconds.
#[tauri::command]
pub(crate) async fn cmd_estimate_job(
    audio_duration_s: f64,
    extreme_accuracy: bool,
) -> Result<JobPlan, String> {
    let diagnostics = detect_device_diagnostics();
    let device_tier = DeviceTier::classify(
        diagnostics.cuda_available,
        diagnostics.vram_gb,
        diagnostics.cpu_cores,
    );
    let input = SchedulerInput {
        duration_seconds: audio_duration_s,
        snr_db: None,
        speech_density: None,
        estimated_speaker_count: 1,
        is_high_noise: false,
        device_tier,
        cuda_available: diagnostics.cuda_available,
        vram_gb: diagnostics.vram_gb,
        cpu_cores: diagnostics.cpu_cores,
        extreme_accuracy,
        model_cached: true,
        cold_start_seconds: None,
    };

    let plan = Scheduler::plan(&input);
    log::info!(
        "Estimate: extreme={}, duration={:.0}s → est={:.0}s, model={:?}",
        extreme_accuracy,
        audio_duration_s,
        plan.estimated_duration_seconds,
        plan.model_size,
    );

    Ok(JobPlan {
        job_id: String::new(), // No job created yet
        plan_id: plan.plan_id,
        model_size: format!("{:?}", plan.model_size),
        estimated_seconds: plan.estimated_duration_seconds,
        explanation: plan.explanation,
        fallback_reason: plan.fallback_reason,
    })
}

/// Activate a license key.
#[tauri::command]
pub(crate) async fn cmd_activate_license(
    app_handle: tauri::AppHandle,
    license_key: String,
) -> Result<String, String> {
    let app_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?;

    let mut manager = LicenseManager::new(app_dir).map_err(|e| e.to_string())?;
    manager.activate(&license_key).map_err(|e| e.to_string())?;

    Ok("License activated successfully".into())
}

/// Get current license state (trial days remaining, activation status).
#[tauri::command]
pub(crate) async fn cmd_get_license_state(app_handle: tauri::AppHandle) -> Result<serde_json::Value, String> {
    use LicenseState::*;

    let app_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?;

    let manager = LicenseManager::new(app_dir).map_err(|e| e.to_string())?;
    let state = manager.state();

    let json = match state {
        NotActivated => serde_json::json!({
            "state": "not_activated",
            "is_usable": false,
            "trial_days_remaining": 0
        }),
        Trial {
            days_remaining,
            expires_at,
            ..
        } => serde_json::json!({
            "state": "trial",
            "is_usable": true,
            "trial_days_remaining": days_remaining,
            "expires_at": expires_at
        }),
        Activated {
            model_updates_until,
            ..
        } => serde_json::json!({
            "state": "activated",
            "is_usable": true,
            "trial_days_remaining": 0,
            "model_updates_until": model_updates_until
        }),
        TrialExpired { .. } => serde_json::json!({
            "state": "trial_expired",
            "is_usable": false,
            "trial_days_remaining": 0
        }),
        Invalid(reason) => serde_json::json!({
            "state": "invalid",
            "is_usable": false,
            "trial_days_remaining": 0,
            "reason": reason
        }),
    };

    Ok(json)
}
