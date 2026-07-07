#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_media_folder_filters_supported_files_recursively() {
        let root = std::env::temp_dir().join(format!("audraflow-scan-{}", uuid::Uuid::new_v4()));
        let nested = root.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join("audio.mp3"), b"stub").unwrap();
        std::fs::write(root.join("notes.txt"), b"ignore").unwrap();
        std::fs::write(nested.join("video.mp4"), b"stub").unwrap();

        let files = scan_media_folder(&root).unwrap();

        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|path| path.ends_with("audio.mp3")));
        assert!(files.iter().any(|path| path.ends_with("video.mp4")));
        assert!(!files.iter().any(|path| path.ends_with("notes.txt")));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn inspect_media_file_reports_basic_metadata() {
        let root = std::env::temp_dir().join(format!("audraflow-inspect-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("sample.mp3");
        std::fs::write(&path, b"stub").unwrap();

        let info = inspect_media_file(&path).unwrap();

        assert_eq!(info.file_name, "sample.mp3");
        assert_eq!(info.format, "MP3");
        assert_eq!(info.size_bytes, 4);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_component_ids_normalize_dependency_names() {
        assert_eq!(normalize_runtime_component_id("whisperCli"), Some("whisper"));
        assert_eq!(normalize_runtime_component_id("whisper-cli"), Some("whisper"));
        assert_eq!(normalize_runtime_component_id("ffprobe"), Some("ffmpeg"));
        assert_eq!(
            normalize_runtime_component_id("vcRedist"),
            Some("vc-redist")
        );
        assert_eq!(normalize_runtime_component_id("funasrCli"), Some("funasr"));
        assert_eq!(
            normalize_runtime_component_id("llama-funasr-cli"),
            Some("funasr")
        );
        assert_eq!(normalize_runtime_component_id("ytDlp"), Some("yt-dlp"));
        assert_eq!(normalize_runtime_component_id("unknown"), None);
    }

    #[test]
    fn repair_validation_accepts_ready_target() {
        let health = RuntimeHealthDto {
            generated_at_ms: 0,
            blocking_count: 0,
            warning_count: 0,
            items: vec![RuntimeDependencyDto {
                id: "ffmpeg".into(),
                status: "ready".into(),
                kind: "required".into(),
                path: Some("/tmp/ffmpeg".into()),
                version: Some("ffmpeg version test".into()),
                detail: None,
                repairable: true,
            }, RuntimeDependencyDto {
                id: "ffprobe".into(),
                status: "ready".into(),
                kind: "required".into(),
                path: Some("/tmp/ffprobe".into()),
                version: Some("ffprobe version test".into()),
                detail: None,
                repairable: true,
            }],
        };

        assert!(ensure_repair_succeeded(&health, "ffmpeg").is_ok());
    }

    #[test]
    fn repair_validation_rejects_unready_target() {
        let health = RuntimeHealthDto {
            generated_at_ms: 0,
            blocking_count: 1,
            warning_count: 0,
            items: vec![RuntimeDependencyDto {
                id: "whisperCli".into(),
                status: "missing".into(),
                kind: "required".into(),
                path: None,
                version: None,
                detail: Some("whisper.dll was not found".into()),
                repairable: true,
            }],
        };

        let error = ensure_repair_succeeded(&health, "whisperCli").unwrap_err();

        assert!(error.contains("Whisper CLI"));
        assert!(error.contains("missing"));
        assert!(error.contains("whisper.dll was not found"));
    }

    #[test]
    fn funasr_runtime_component_uses_official_release_when_supported() {
        let download = funasr_official_download();
        if cfg!(any(
            all(windows, target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "aarch64"),
            all(target_os = "macos", target_arch = "aarch64")
        )) {
            let url = download.expect("Fun-ASR download should be available").url;
            assert!(url.contains("github.com/modelscope/FunASR/releases/download/"));
            assert!(url.contains(FUNASR_LLAMA_CPP_RELEASE_TAG));
            assert!(!url.contains("500wango/audraflow"));
        } else {
            assert!(download.is_none());
        }
    }

    #[test]
    fn tar_gz_runtime_archive_extracts_required_file_by_basename() {
        let root = std::env::temp_dir().join(format!(
            "audraflow-runtime-tar-{}",
            uuid::Uuid::new_v4()
        ));
        let archive_path = root.join("funasr.tar.gz");
        let destination = root.join("bin");
        std::fs::create_dir_all(&destination).unwrap();

        let archive_file = std::fs::File::create(&archive_path).unwrap();
        let encoder =
            flate2::write::GzEncoder::new(archive_file, flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let payload = b"#!/bin/sh\n";
        let mut header = tar::Header::new_gnu();
        header.set_path("funasr-llamacpp/llama-funasr-cli").unwrap();
        header.set_size(payload.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder.append(&header, &payload[..]).unwrap();
        let encoder = builder.into_inner().unwrap();
        encoder.finish().unwrap();

        extract_required_files_from_archive(&archive_path, &destination, &["llama-funasr-cli"])
            .unwrap();

        assert_eq!(
            std::fs::read(destination.join("llama-funasr-cli")).unwrap(),
            payload
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn funasr_usage_output_is_accepted_as_probe_success() {
        let usage = "usage: /usr/bin/audraflow-llama-funasr-cli --enc enc.gguf -m llm.gguf -a audio.wav [-n npred] [--json]";
        assert!(text_looks_like_funasr_usage(usage));
    }

    #[test]
    fn funasr_glibc_loader_error_is_not_probe_success() {
        let error = "/lib/x86_64-linux-gnu/libc.so.6: version `GLIBC_2.38' not found";
        assert!(!text_looks_like_funasr_usage(error));
    }

    #[test]
    fn zip_entry_basename_handles_nested_and_windows_paths() {
        assert_eq!(zip_entry_basename("bin/ffmpeg.exe").as_deref(), Some("ffmpeg.exe"));
        assert_eq!(
            zip_entry_basename("nested\\bin\\whisper.dll").as_deref(),
            Some("whisper.dll")
        );
        assert_eq!(zip_entry_basename("/").as_deref(), None);
    }

    #[test]
    fn telemetry_record_hashes_user_identifiers() {
        let record = telemetry_request_to_record(TelemetryEventRequest {
            event_type: "playback_seek".into(),
            job_id: Some("job-secret".into()),
            segment_id: Some("segment-secret".into()),
            audio_hours: None,
            transcript_chars: None,
            active_seconds: None,
            inactive_seconds: None,
            completed_ratio: None,
            op_type: None,
            chars_before: None,
            chars_after: None,
            source: None,
            from_ms: Some(1200),
            to_ms: Some(3000),
            trigger: Some("click_segment".into()),
            mark_ms: None,
            label_type: None,
            format: None,
            include_timestamps: None,
            include_speakers: None,
            include_marks: None,
        })
        .unwrap();

        let json = serde_json::to_string(&record).unwrap();

        assert!(!json.contains("job-secret"));
        assert!(!json.contains("segment-secret"));
        assert!(json.contains("jobIdHash"));
        assert!(json.contains("segmentIdHash"));
        assert!(json.contains("playback_seek"));
    }

    #[test]
    fn telemetry_rejects_unknown_event_type() {
        let error = telemetry_request_to_record(TelemetryEventRequest {
            event_type: "raw_transcript_upload".into(),
            job_id: None,
            segment_id: None,
            audio_hours: None,
            transcript_chars: None,
            active_seconds: None,
            inactive_seconds: None,
            completed_ratio: None,
            op_type: None,
            chars_before: None,
            chars_after: None,
            source: None,
            from_ms: None,
            to_ms: None,
            trigger: None,
            mark_ms: None,
            label_type: None,
            format: None,
            include_timestamps: None,
            include_speakers: None,
            include_marks: None,
        })
        .unwrap_err();

        assert!(error.contains("Unsupported telemetry event type"));
    }

    #[test]
    fn telemetry_consent_defaults_to_disabled_and_undecided() {
        let path = std::env::temp_dir().join(format!(
            "audraflow-consent-missing-{}.json",
            uuid::Uuid::new_v4()
        ));

        let state = read_telemetry_consent_from_path(&path);

        assert!(!state.enabled);
        assert!(!state.decided);
        assert!(state.updated_at_ms.is_none());
    }

    #[test]
    fn telemetry_consent_roundtrips_enabled_state() {
        let root = std::env::temp_dir().join(format!("audraflow-consent-{}", uuid::Uuid::new_v4()));
        let path = root.join("privacy").join("telemetry-consent.json");

        let written = write_telemetry_consent_to_path(&path, true).unwrap();
        let loaded = read_telemetry_consent_from_path(&path);

        assert!(written.enabled);
        assert!(written.decided);
        assert!(loaded.enabled);
        assert!(loaded.decided);
        assert!(loaded.updated_at_ms.is_some());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn direct_media_urls_are_not_treated_as_platform_links() {
        let direct = reqwest::Url::parse("https://cdn.example.com/audio/file.mp3").unwrap();
        let youtube_direct =
            reqwest::Url::parse("https://youtube.com/downloads/archive.wav").unwrap();

        assert!(is_probable_direct_media_url(&direct));
        assert!(!is_probable_platform_url(&direct));
        assert!(is_probable_direct_media_url(&youtube_direct));
        assert!(!is_probable_platform_url(&youtube_direct));
    }

    #[test]
    fn known_platform_urls_require_platform_resolution() {
        let youtube = reqwest::Url::parse("https://www.youtube.com/watch?v=test").unwrap();
        let bilibili = reqwest::Url::parse("https://www.bilibili.com/video/BV123").unwrap();

        assert!(is_probable_platform_url(&youtube));
        assert!(is_probable_platform_url(&bilibili));
    }

    #[test]
    fn runtime_command_available_rejects_missing_absolute_path() {
        let missing = std::env::temp_dir().join(format!(
            "audraflow-missing-command-{}",
            uuid::Uuid::new_v4()
        ));

        assert!(!is_runtime_command_available(&missing));
    }

    #[test]
    fn directory_size_and_clear_children_work_recursively() {
        let root = std::env::temp_dir().join(format!("audraflow-cache-{}", uuid::Uuid::new_v4()));
        let nested = root.join("model-v1");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("model.bin"), [1u8; 7]).unwrap();
        std::fs::write(root.join("manifest.json"), [2u8; 3]).unwrap();

        let size = directory_size_bytes(&root).unwrap();
        let removed = clear_directory_children(&root).unwrap();

        assert_eq!(size, 10);
        assert_eq!(removed, 2);
        assert_eq!(count_directory_children(&root).unwrap(), 0);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn model_component_normalization_keeps_safe_path_parts() {
        assert_eq!(
            normalize_model_component(" ../ggml-base.bin ", "fallback"),
            "..ggml-base.bin"
        );
        assert_eq!(normalize_model_component("   ", "fallback"), "fallback");
    }

    #[test]
    fn default_transcription_language_uses_auto_for_multilingual_models() {
        assert_eq!(default_transcription_language("auto"), "auto");
        assert_eq!(default_transcription_language(" multilingual "), "auto");
        assert_eq!(default_transcription_language(""), "auto");
    }

    #[test]
    fn default_transcription_language_preserves_explicit_languages() {
        assert_eq!(default_transcription_language("en"), "en");
        assert_eq!(default_transcription_language(" ZH "), "zh");
    }

    #[test]
    fn normalize_transcription_language_accepts_explicit_audio_language() {
        assert_eq!(
            normalize_transcription_language(Some("english"), None),
            "en"
        );
        assert_eq!(normalize_transcription_language(Some("中文"), None), "zh");
        assert_eq!(normalize_transcription_language(Some("zh"), None), "zh");
        assert_eq!(normalize_transcription_language(Some("auto"), None), "auto");
        assert_eq!(normalize_transcription_language(None, None), "auto");
    }

    #[test]
    fn normalize_asr_engine_defaults_to_auto() {
        assert_eq!(normalize_asr_engine(None), "auto");
        assert_eq!(normalize_asr_engine(Some("auto")), "auto");
        assert_eq!(normalize_asr_engine(Some("funasr")), "funasr");
        assert_eq!(normalize_asr_engine(Some("fun-asr-nano")), "funasr");
        assert_eq!(normalize_asr_engine(Some("whisper.cpp")), "whisper");
    }

    #[test]
    fn resolve_asr_engine_prefers_whisper_when_available() {
        assert_eq!(resolve_asr_engine("auto", "music", true), "whisper");
        assert_eq!(resolve_asr_engine("auto", "music", false), "sensevoice");
        assert_eq!(resolve_asr_engine("auto", "speech", true), "whisper");
        assert_eq!(resolve_asr_engine("auto", "speech", false), "sensevoice");
        assert_eq!(
            resolve_asr_engine("sensevoice", "music", true),
            "sensevoice"
        );
        assert_eq!(resolve_asr_engine("funasr", "music", true), "funasr");
        assert_eq!(resolve_asr_engine("whisper", "speech", false), "whisper");
    }

    fn installed_whisper_model(name: &str) -> audraflow_model_manager::InstalledModel {
        audraflow_model_manager::InstalledModel {
            info: audraflow_model_manager::ModelInfo {
                name: name.into(),
                version: "test".into(),
                language: "auto".into(),
                size_bytes: 1,
                sha256: "hash".into(),
                download_url: "https://example.com/model.bin".into(),
                model_type: audraflow_model_manager::ModelType::WhisperCpp,
            },
            path: PathBuf::from(format!("/models/{name}.bin")),
            installed_at_ms: 1,
        }
    }

    #[test]
    fn preferred_lyrics_whisper_model_respects_selected_music_model() {
        let base = installed_whisper_model("base");
        let small = installed_whisper_model("small");
        let large = installed_whisper_model("large-v3-turbo-q8_0");
        let installed = vec![base, small.clone(), large.clone()];

        let preferred = preferred_lyrics_whisper_model(&installed, Some(&large), false).unwrap();

        assert_eq!(preferred.info.name, "large-v3-turbo-q8_0");
        assert_eq!(preferred.path, large.path);
    }

    #[test]
    fn preferred_lyrics_whisper_model_prefers_large_for_extreme_music_without_selection() {
        let base = installed_whisper_model("base");
        let small = installed_whisper_model("small");
        let large = installed_whisper_model("large-v3-turbo-q8_0");
        let installed = vec![base, small.clone(), large.clone()];

        let preferred = preferred_lyrics_whisper_model(&installed, None, true).unwrap();

        assert_eq!(preferred.info.name, "large-v3-turbo-q8_0");
        assert_eq!(preferred.path, large.path);
    }

    #[test]
    fn preferred_lyrics_whisper_model_prefers_small_for_balanced_music_without_selection() {
        let base = installed_whisper_model("base");
        let small = installed_whisper_model("small");
        let large = installed_whisper_model("large-v3-turbo-q8_0");
        let installed = vec![base, small.clone(), large.clone()];

        let preferred = preferred_lyrics_whisper_model(&installed, None, false).unwrap();

        assert_eq!(preferred.info.name, "small");
    }

    #[test]
    fn resolve_whisper_model_for_music_respects_selected_model_when_extreme() {
        let base = installed_whisper_model("base");
        let small = installed_whisper_model("small");
        let large = installed_whisper_model("large-v3-turbo-q8_0");
        let installed = vec![base, small.clone(), large.clone()];

        let resolved =
            resolve_whisper_model_for_job("whisper", "music", Some(small), &installed, true)
                .unwrap();

        assert_eq!(resolved.info.name, "small");
    }

    #[test]
    fn resolve_whisper_model_for_music_uses_large_when_extreme_without_selection() {
        let base = installed_whisper_model("base");
        let small = installed_whisper_model("small");
        let large = installed_whisper_model("large-v3-turbo-q8_0");
        let installed = vec![base, small, large.clone()];

        let resolved =
            resolve_whisper_model_for_job("whisper", "music", None, &installed, true).unwrap();

        assert_eq!(resolved.info.name, "large-v3-turbo-q8_0");
    }

    #[test]
    fn normalize_audio_mode_preserves_music_only() {
        assert_eq!(normalize_audio_mode(Some("music")), "music");
        assert_eq!(normalize_audio_mode(Some("lyrics")), "music");
        assert_eq!(normalize_audio_mode(Some("speech")), "speech");
        assert_eq!(normalize_audio_mode(None), "speech");
    }

    #[test]
    fn normalize_vocal_separation_requires_music_mode() {
        assert_eq!(
            normalize_vocal_separation(Some("demucs"), "music"),
            "demucs"
        );
        assert_eq!(normalize_vocal_separation(Some("on"), "music"), "demucs");
        assert_eq!(normalize_vocal_separation(Some("demucs"), "speech"), "off");
        assert_eq!(normalize_vocal_separation(None, "music"), "off");
    }

    #[test]
    fn infer_model_name_strips_common_ggml_prefix() {
        let path = PathBuf::from("C:\\models\\ggml-tiny.bin");

        assert_eq!(infer_model_name(&path, None), "tiny");
        assert_eq!(
            infer_model_name(&path, Some("custom/model".into())),
            "custommodel"
        );
    }

    #[test]
    fn infer_model_name_from_url_strips_common_ggml_prefix() {
        assert_eq!(
            infer_model_name_from_url("https://example.com/models/ggml-base.bin", None),
            "base"
        );
        assert_eq!(
            infer_model_name_from_url("https://example.com/model.bin", Some("custom/model".into())),
            "custommodel"
        );
    }

    #[test]
    fn device_diagnostics_are_always_reportable() {
        let diagnostics = detect_device_diagnostics();

        assert!(diagnostics.cpu_cores > 0);
        assert!(!diagnostics.device_tier.is_empty());
        if !diagnostics.cuda_available {
            assert!(diagnostics.fallback_message.is_some());
        }
    }

    #[test]
    fn export_conversion_preserves_json_metadata_and_docx_bytes() {
        let storage = audraflow_storage::Storage::open_in_memory().unwrap();
        storage
            .create_job("job-export", "C:\\media\\meeting.wav", "hash", false)
            .unwrap();
        storage
            .insert_segments(
                "job-export",
                &[Segment {
                    segment_id: "seg-1".into(),
                    start_ms: 1_000,
                    end_ms: 4_000,
                    speaker_id: Some("Speaker A".into()),
                    text: "corrected text".into(),
                    raw_text: "raw text".into(),
                    confidence: 0.62,
                    low_confidence_reasons: vec!["low_snr".into()],
                    corrections: vec![],
                    marks: vec![],
                }],
            )
            .unwrap();
        storage
            .record_correction(
                "seg-1",
                "text",
                "raw text",
                "corrected text",
                &CorrectionSource::User,
                false,
            )
            .unwrap();
        storage
            .add_mark("seg-1", 2_500, Some("review"), Some("check this"))
            .unwrap();

        let rows = storage.get_segments("job-export").unwrap();
        let json = render_export(
            &storage,
            "job-export",
            &rows,
            "json",
            SpeakerExportFilter::All,
            true,
            true,
        )
        .unwrap();

        assert!(json.contains("\"lowConfidenceReasons\""));
        assert!(json.contains("\"low_snr\""));
        assert!(json.contains("\"corrections\""));
        assert!(json.contains("\"oldValue\": \"raw text\""));
        assert!(json.contains("\"marks\""));
        assert!(json.contains("\"markMs\": 2500"));

        let obsidian = render_export(
            &storage,
            "job-export",
            &rows,
            "obsidian",
            SpeakerExportFilter::All,
            true,
            true,
        )
        .unwrap();
        assert!(obsidian.contains("> [!quote]"));
        assert!(obsidian.contains("Speaker A"));

        let notion = render_export(
            &storage,
            "job-export",
            &rows,
            "notion",
            SpeakerExportFilter::All,
            true,
            true,
        )
        .unwrap();
        assert!(notion.contains("<details>"));
        assert!(notion.contains("<summary>Speaker A"));

        let export_segments = storage_segments_to_ipc_segments(&storage, &rows, true).unwrap();
        let docx = audraflow_export::export_docx(
            &export_segments,
            "meeting",
            &audraflow_export::ExportOptions {
                include_timestamps: true,
                include_speakers: true,
                include_marks: true,
                speaker_filter: audraflow_export::SpeakerFilter::All,
            },
        )
        .unwrap();

        assert_eq!(&docx[0..2], b"PK");
    }

    #[test]
    fn speaker_filter_controls_exported_speaker_labels() {
        let named = SegmentRow {
            segment_id: "named".into(),
            start_ms: 0,
            end_ms: 1_000,
            speaker_id: Some("Alice".into()),
            text: "named speaker".into(),
            raw_text: "named speaker".into(),
            confidence: 0.9,
        };
        let placeholder = SegmentRow {
            segment_id: "placeholder".into(),
            start_ms: 1_000,
            end_ms: 2_000,
            speaker_id: Some("Speaker A".into()),
            text: "placeholder speaker".into(),
            raw_text: "placeholder speaker".into(),
            confidence: 0.9,
        };

        let all = [named.clone(), placeholder.clone()]
            .iter()
            .map(|segment| render_plain_segment(segment, SpeakerExportFilter::All, false))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains("Alice: named speaker"));
        assert!(all.contains("Speaker A: placeholder speaker"));

        let named_only = [named, placeholder]
            .iter()
            .map(|segment| render_plain_segment(segment, SpeakerExportFilter::NamedOnly, false))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(named_only.contains("Alice: named speaker"));
        assert!(named_only.contains("placeholder speaker"));
        assert!(!named_only.contains("Speaker A:"));

        assert_eq!(
            render_segment_text(
                &SegmentRow {
                    segment_id: "hidden".into(),
                    start_ms: 0,
                    end_ms: 1_000,
                    speaker_id: Some("Alice".into()),
                    text: "speaker hidden".into(),
                    raw_text: "speaker hidden".into(),
                    confidence: 0.9,
                },
                SpeakerExportFilter::Hidden,
            ),
            "speaker hidden"
        );
    }
}
