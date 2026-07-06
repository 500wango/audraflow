fn print_usage() {
    eprintln!(
        "Usage: audraflow-asr-runtime transcribe <audio> [--engine whisper|sensevoice|funasr] [--model <model.bin>] [--whisper-cli <path>] [--language auto|zh|en] [--file-hash <sha256>] [--extreme-accuracy] [--audio-mode speech|music] [--vocal-separation off|demucs]"
    );
}

fn cmd_transcribe(args: &[String]) -> anyhow::Result<()> {
    let config = parse_runtime_transcribe_args(args)?;
    let device_info = whisper_engine::detect_device();
    let pipeline = audio_pipeline::AudioPipeline::new()?;
    let result = match config.asr_engine {
        AsrEngine::FunAsr => transcribe_file_with_funasr_sync(
            &pipeline,
            &config.input_path,
            &config.file_hash,
            &config.language,
            config.extreme_accuracy,
            config.audio_mode,
            config.vocal_separation,
        )?,
        AsrEngine::SenseVoice => {
            match transcribe_file_with_sensevoice_sync(
                &pipeline,
                &config.input_path,
                &config.file_hash,
                &config.language,
                config.extreme_accuracy,
                config.audio_mode,
                config.vocal_separation,
            ) {
                Ok(result) => result,
                Err(error) if config.model_path.is_some() => {
                    log::warn!("SenseVoice failed; falling back to Whisper: {error}");
                    let mut result =
                        transcribe_file_with_whisper_sync(&device_info, &pipeline, &config)?;
                    result.preprocess_messages.insert(
                        0,
                        format!(
                            "SenseVoice failed; fell back to Whisper: {}",
                            truncate_message(&error.to_string(), 180)
                        ),
                    );
                    result
                }
                Err(error) => return Err(error),
            }
        }
        AsrEngine::Whisper => transcribe_file_with_whisper_sync(&device_info, &pipeline, &config)?,
    };
    let output = RuntimeTranscribeOutput {
        segments: result.segments,
        audio_duration_s: result.audio_info.duration_seconds,
        rtf: result.rtf,
        ttfv_s: result.ttfv_s,
        chunk_count: result.chunk_count,
        preprocess_messages: result.preprocess_messages,
    };
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

fn transcribe_file_with_whisper_sync(
    device_info: &whisper_engine::DeviceInfo,
    pipeline: &audio_pipeline::AudioPipeline,
    config: &RuntimeTranscribeConfig,
) -> anyhow::Result<TranscriptionResult> {
    let model_path = config
        .model_path
        .clone()
        .context("Missing --model <path> for Whisper engine")?;
    let engine = whisper_engine::WhisperEngine::new(device_info)?
        .with_model(model_path)
        .with_whisper_cli(config.whisper_cli.clone())
        .with_language(config.language.clone())
        .with_lyrics_mode(config.audio_mode == AudioMode::Music);
    transcribe_file_pipeline_sync(
        &engine,
        pipeline,
        &config.input_path,
        &config.file_hash,
        config.extreme_accuracy,
        config.audio_mode,
        config.vocal_separation,
    )
}

fn parse_runtime_transcribe_args(args: &[String]) -> anyhow::Result<RuntimeTranscribeConfig> {
    let mut input: Option<String> = None;
    let mut model: Option<String> = None;
    let mut whisper_cli: Option<String> = None;
    let mut language = "auto".to_string();
    let mut file_hash = String::new();
    let mut asr_engine = AsrEngine::SenseVoice;
    let mut extreme_accuracy = false;
    let mut audio_mode = AudioMode::Speech;
    let mut vocal_separation = VocalSeparationMode::Off;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--engine" | "--asr-engine" => {
                asr_engine = parse_asr_engine(&take_arg_value(args, &mut i, "--engine")?)?;
            }
            "--model" | "-m" => model = Some(take_arg_value(args, &mut i, "--model")?),
            "--whisper-cli" => {
                whisper_cli = Some(take_arg_value(args, &mut i, "--whisper-cli")?);
            }
            "--language" | "-l" => {
                language = take_arg_value(args, &mut i, "--language")?;
            }
            "--file-hash" => {
                file_hash = take_arg_value(args, &mut i, "--file-hash")?;
            }
            "--extreme-accuracy" => extreme_accuracy = true,
            "--audio-mode" => {
                audio_mode = parse_audio_mode(&take_arg_value(args, &mut i, "--audio-mode")?)?;
            }
            "--vocal-separation" => {
                vocal_separation = parse_vocal_separation_mode(&take_arg_value(
                    args,
                    &mut i,
                    "--vocal-separation",
                )?)?;
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            arg if arg.starts_with('-') => anyhow::bail!("Unknown transcribe option: {arg}"),
            arg if input.is_none() => input = Some(arg.to_string()),
            arg => anyhow::bail!("Unexpected transcribe argument: {arg}"),
        }
        i += 1;
    }

    let input_path = PathBuf::from(input.context("Missing input audio path")?);
    ensure_file(&input_path, "input audio")?;
    let model_path = model.map(PathBuf::from);
    if asr_engine == AsrEngine::Whisper {
        let model_path = model_path
            .as_ref()
            .context("Missing --model <path> for Whisper engine")?;
        ensure_file(model_path, "ASR model")?;
    } else if let Some(model_path) = model_path.as_ref() {
        ensure_file(model_path, "fallback ASR model")?;
    }

    Ok(RuntimeTranscribeConfig {
        input_path,
        model_path,
        whisper_cli: whisper_engine::resolve_whisper_cli(whisper_cli.map(PathBuf::from)),
        language,
        file_hash,
        asr_engine,
        extreme_accuracy,
        audio_mode,
        vocal_separation,
    })
}

fn take_arg_value(args: &[String], index: &mut usize, flag: &str) -> anyhow::Result<String> {
    *index += 1;
    let value = args
        .get(*index)
        .with_context(|| format!("Missing value for {flag}"))?;
    if value.starts_with('-') {
        anyhow::bail!("Missing value for {flag}");
    }
    Ok(value.clone())
}

fn parse_audio_mode(value: &str) -> anyhow::Result<AudioMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "speech" | "default" => Ok(AudioMode::Speech),
        "music" | "lyrics" | "lyric" => Ok(AudioMode::Music),
        other => anyhow::bail!("Unsupported audio mode: {other}"),
    }
}

fn parse_asr_engine(value: &str) -> anyhow::Result<AsrEngine> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "sensevoice" | "sense_voice" => Ok(AsrEngine::SenseVoice),
        "whisper" | "whispercpp" | "whisper.cpp" => Ok(AsrEngine::Whisper),
        "funasr" | "fun-asr" | "fun_asr" | "funasr-nano" | "fun-asr-nano" => Ok(AsrEngine::FunAsr),
        other => anyhow::bail!("Unsupported ASR engine: {other}"),
    }
}

fn parse_vocal_separation_mode(value: &str) -> anyhow::Result<VocalSeparationMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "off" | "none" | "false" | "disabled" => Ok(VocalSeparationMode::Off),
        "demucs" | "vocal" | "vocals" | "on" | "true" => Ok(VocalSeparationMode::Demucs),
        other => anyhow::bail!("Unsupported vocal separation mode: {other}"),
    }
}

fn sensevoice_chunking_plan(
    audio_mode: AudioMode,
    extreme_accuracy: bool,
    vocals_isolated: bool,
) -> SenseVoiceChunkingPlan {
    let max_chunk_ms = if extreme_accuracy { 20_000 } else { 30_000 };
    match audio_mode {
        AudioMode::Music => SenseVoiceChunkingPlan {
            max_chunk_ms,
            overlap_ms: 2_000,
            internal_vad: !vocals_isolated,
        },
        AudioMode::Speech => SenseVoiceChunkingPlan {
            max_chunk_ms,
            overlap_ms: 0,
            internal_vad: true,
        },
    }
}

fn music_chunking_plan(_extreme_accuracy: bool) -> (i64, i64) {
    (90_000, 0)
}

fn ensure_file(path: &Path, label: &str) -> anyhow::Result<()> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("{label} file not found: {}", path.display()))?;
    if !metadata.is_file() {
        anyhow::bail!("{label} path is not a file: {}", path.display());
    }
    if metadata.len() == 0 {
        anyhow::bail!("{label} file is empty: {}", path.display());
    }
    Ok(())
}
