//! AudraFlow CLI
//!
//! Command-line interface for batch transcription and export.
//! PRD FR-015: supports `transcribe`, `export`, and `batch` subcommands.
//!
//! All output can be directed to stdout for pipeline integration.

use anyhow::Context;
use audraflow_export::*;
use audraflow_ipc::Segment;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        print_usage();
        return Ok(());
    }

    match args[1].as_str() {
        "transcribe" => cmd_transcribe(&args[2..])?,
        "export" => cmd_export(&args[2..])?,
        "batch" => cmd_batch(&args[2..])?,
        "version" => println!("AudraFlow CLI v0.1.0"),
        "help" | "--help" | "-h" => print_usage(),
        _ => {
            eprintln!("Unknown command: {}", args[1]);
            print_usage();
        }
    }

    Ok(())
}

fn print_usage() {
    println!(
        r#"AudraFlow CLI v0.1.0

USAGE:
  audraflow <command> [options]

COMMANDS:
  transcribe <file>    Transcribe an audio file (requires whisper.cpp)
  export <file>        Export a transcript JSON to various formats
  batch <dir>          Batch export a directory of transcript JSON files
  version              Print version and exit

TRANSCRIBE OPTIONS:
  --model <path>       Required whisper.cpp model file, e.g. ggml-base.bin
  --whisper-cli <path> whisper-cli executable or command name (default: whisper-cli)
  --language <code>    Language passed to whisper-cli (default: zh)
  --format <fmt>       Output format: txt, md, srt, vtt, json, docx, obsidian, notion
  --output <path>      Write to file (default: stdout; required for docx)
  --timestamps         Include timestamps in text/Markdown output
  --speakers           Include speaker labels in output
  --speaker-filter <f> Speaker labels: all, namedOnly, hidden
  --marks              Include timestamp marks where the format supports them
  --title <text>       Document title (for Markdown/DOCX)

EXPORT OPTIONS:
  --format <fmt>       Output format: txt, md, srt, vtt, json, docx, obsidian, notion
                       stdout-json and stdout-markdown are accepted aliases
  --output <path>      Write to file (default: stdout)
  --timestamps         Include timestamps in output
  --speakers           Include speaker labels in output
  --speaker-filter <f> Speaker labels: all, namedOnly, hidden
  --marks              Include timestamp marks where the format supports them
  --title <text>       Document title (for Markdown/DOCX)

EXAMPLES:
  audraflow transcribe audio.mp3 --model models/ggml-base.bin --format md --output out.md
  audraflow export transcript.json --format srt --output out.srt
  audraflow export transcript.json --format md --timestamps --speakers
  audraflow batch ./transcripts/ --format txt --output-dir ./text/
"#
    );
}

// ── Subcommands ────────────────────────────────────────────────────────────

struct TranscribeConfig {
    input_path: PathBuf,
    model_path: PathBuf,
    whisper_cli: PathBuf,
    language: String,
    output_format: String,
    output_path: Option<String>,
    title: String,
    export_options: ExportOptions,
}

fn cmd_transcribe(args: &[String]) -> anyhow::Result<()> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_usage();
        return Ok(());
    }

    let config = parse_transcribe_args(args)?;
    ensure_stdout_supported(&config.output_format, config.output_path.as_deref())?;
    ensure_existing_file(&config.input_path, "input audio file")?;
    ensure_existing_file(&config.model_path, "whisper model file")?;
    ensure_whisper_cli_hint_is_valid(&config.whisper_cli)?;

    eprintln!("Transcribing: {}", config.input_path.display());
    eprintln!("Model: {}", config.model_path.display());

    let segments = run_whisper_cli(&config)?;
    let source_hash = hash_file_sha256(&config.input_path)?;
    let transcript_id = config
        .input_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("transcript");
    let rendered = render_cli_export_with_metadata(
        &segments,
        &config.output_format,
        transcript_id,
        &source_hash,
        &config.title,
        &config.export_options,
    )?;

    write_rendered_output(
        rendered,
        &config.output_format,
        config.output_path.as_deref(),
    )?;
    Ok(())
}

fn parse_transcribe_args(args: &[String]) -> anyhow::Result<TranscribeConfig> {
    let mut input: Option<String> = None;
    let mut model: Option<String> = None;
    let mut whisper_cli = "whisper-cli".to_string();
    let mut language = "zh".to_string();
    let mut format = "json".to_string();
    let mut output: Option<String> = None;
    let mut timestamps = false;
    let mut speakers = false;
    let mut marks = false;
    let mut speaker_filter = SpeakerFilter::All;
    let mut title: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--model" | "-m" => model = Some(take_flag_value(args, &mut i, "--model")?),
            "--whisper-cli" => {
                whisper_cli = take_flag_value(args, &mut i, "--whisper-cli")?;
            }
            "--language" | "-l" => {
                language = take_flag_value(args, &mut i, "--language")?;
            }
            "--format" | "-f" => {
                format = take_flag_value(args, &mut i, "--format")?;
            }
            "--output" | "-o" => {
                output = Some(take_flag_value(args, &mut i, "--output")?);
            }
            "--timestamps" => timestamps = true,
            "--no-timestamps" => timestamps = false,
            "--speakers" => speakers = true,
            "--no-speakers" => speakers = false,
            "--marks" => marks = true,
            "--speaker-filter" => {
                let value = take_flag_value(args, &mut i, "--speaker-filter")?;
                speaker_filter = parse_speaker_filter(&value)?;
                speakers = !matches!(speaker_filter, SpeakerFilter::Hidden);
            }
            "--title" => {
                title = Some(take_flag_value(args, &mut i, "--title")?);
            }
            arg if arg.starts_with('-') => {
                anyhow::bail!("Unknown transcribe option: {arg}");
            }
            arg if input.is_none() => {
                input = Some(arg.to_string());
            }
            arg => {
                anyhow::bail!("Unexpected transcribe argument: {arg}");
            }
        }
        i += 1;
    }

    let input_path = PathBuf::from(input.context(
        "Missing input audio file. Usage: audraflow transcribe <file> --model <model.bin>",
    )?);
    let model_path = PathBuf::from(
        model.context("Missing --model <path>. Example: --model models/ggml-base.bin")?,
    );
    let output_format = normalize_format(&format)?;
    let title = title.unwrap_or_else(|| {
        input_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("Transcript")
            .to_string()
    });

    Ok(TranscribeConfig {
        input_path,
        model_path,
        whisper_cli: PathBuf::from(whisper_cli),
        language,
        output_format,
        output_path: output,
        title,
        export_options: ExportOptions {
            include_timestamps: timestamps,
            include_speakers: speakers,
            include_marks: marks,
            speaker_filter,
        },
    })
}

fn take_flag_value(args: &[String], index: &mut usize, flag: &str) -> anyhow::Result<String> {
    *index += 1;
    let value = args
        .get(*index)
        .with_context(|| format!("Missing value for {flag}"))?;
    if value.starts_with('-') {
        anyhow::bail!("Missing value for {flag}");
    }
    Ok(value.clone())
}

fn run_whisper_cli(config: &TranscribeConfig) -> anyhow::Result<Vec<Segment>> {
    let output_prefix = temp_output_prefix();
    let json_path = output_prefix.with_extension("json");

    eprintln!(
        "Running {} -m {} -f {} -oj -of {} -l {}",
        display_command(&config.whisper_cli),
        config.model_path.display(),
        config.input_path.display(),
        output_prefix.display(),
        config.language
    );

    let status = Command::new(&config.whisper_cli)
        .arg("-m")
        .arg(&config.model_path)
        .arg("-f")
        .arg(&config.input_path)
        .arg("-oj")
        .arg("-of")
        .arg(&output_prefix)
        .arg("-l")
        .arg(&config.language)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "whisper-cli was not found or could not be started: {}. Pass --whisper-cli <path> or add it to PATH.",
                display_command(&config.whisper_cli)
            )
        })?;

    let (status, stdout, stderr) = wait_with_forwarded_output(status)?;
    if !status.success() {
        let _ = std::fs::remove_file(&json_path);
        anyhow::bail!(
            "whisper-cli failed with status {status}. stdout: {} stderr: {}",
            preview_output(&stdout),
            preview_output(&stderr)
        );
    }

    let json_str = std::fs::read_to_string(&json_path).with_context(|| {
        format!(
            "whisper-cli completed but did not produce JSON output: {}",
            json_path.display()
        )
    })?;
    let segments = parse_whisper_json(&json_str);
    let _ = std::fs::remove_file(&json_path);
    segments
}

fn wait_with_forwarded_output(
    mut child: std::process::Child,
) -> anyhow::Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>)> {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let stdout_handle = stdout.map(|stream| std::thread::spawn(move || read_child_stream(stream)));
    let stderr_handle = stderr.map(|stream| std::thread::spawn(move || read_child_stream(stream)));

    let status = child.wait().context("Failed to wait for whisper-cli")?;
    let stdout = join_output_thread(stdout_handle)?;
    let stderr = join_output_thread(stderr_handle)?;
    Ok((status, stdout, stderr))
}

fn read_child_stream(mut stream: impl std::io::Read) -> Vec<u8> {
    let mut captured = Vec::new();
    let mut buf = [0_u8; 4096];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                captured.extend_from_slice(&buf[..n]);
                eprint!("{}", String::from_utf8_lossy(&buf[..n]));
            }
            Err(_) => break,
        }
    }
    captured
}

fn join_output_thread(handle: Option<std::thread::JoinHandle<Vec<u8>>>) -> anyhow::Result<Vec<u8>> {
    match handle {
        Some(handle) => handle
            .join()
            .map_err(|_| anyhow::anyhow!("Failed to collect whisper-cli output")),
        None => Ok(Vec::new()),
    }
}

fn parse_whisper_json(json: &str) -> anyhow::Result<Vec<Segment>> {
    let value: Value = serde_json::from_str(json.trim_start_matches('\u{feff}'))
        .context("Invalid whisper-cli JSON output")?;
    let transcriptions = value
        .get("transcription")
        .or_else(|| value.get("transcriptions"))
        .and_then(Value::as_array)
        .context("Missing 'transcription' array in whisper-cli JSON output")?;

    let segments = transcriptions
        .iter()
        .enumerate()
        .map(|(i, segment)| {
            let start_ms = read_whisper_segment_ms(segment, "from");
            let end_ms = read_whisper_segment_ms(segment, "to");
            let text = segment
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();

            Segment {
                segment_id: format!("seg-{i}"),
                start_ms,
                end_ms,
                speaker_id: None,
                raw_text: text.clone(),
                text,
                confidence: 0.9,
                low_confidence_reasons: vec![],
                corrections: vec![],
                marks: vec![],
            }
        })
        .collect();

    Ok(segments)
}

fn read_whisper_segment_ms(segment: &Value, field: &str) -> i64 {
    segment
        .get("offsets")
        .and_then(|offsets| offsets.get(field))
        .and_then(value_as_milliseconds)
        .or_else(|| {
            segment
                .get("timestamps")
                .and_then(|timestamps| timestamps.get(field))
                .and_then(value_as_milliseconds)
        })
        .unwrap_or(0)
}

fn value_as_milliseconds(value: &Value) -> Option<i64> {
    if let Some(ms) = value.as_i64() {
        return Some(ms);
    }
    if let Some(number) = value.as_f64() {
        return Some(number.round() as i64);
    }
    value.as_str().and_then(|text| {
        text.parse::<i64>()
            .ok()
            .or_else(|| parse_timestamp_ms(text))
    })
}

fn parse_timestamp_ms(value: &str) -> Option<i64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    let parts: Vec<&str> = trimmed.split(':').collect();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [seconds] => (0, 0, *seconds),
        [minutes, seconds] => (0, minutes.parse::<i64>().ok()?, *seconds),
        [hours, minutes, seconds] => (
            hours.parse::<i64>().ok()?,
            minutes.parse::<i64>().ok()?,
            *seconds,
        ),
        _ => return None,
    };

    let (seconds, millis) = parse_seconds_and_millis(seconds)?;
    Some(((hours * 3600 + minutes * 60 + seconds) * 1000) + millis)
}

fn parse_seconds_and_millis(value: &str) -> Option<(i64, i64)> {
    let normalized = value.replace(',', ".");
    let mut parts = normalized.splitn(2, '.');
    let seconds = parts.next()?.parse::<i64>().ok()?;
    let millis = parts
        .next()
        .map(|fraction| {
            let digits: String = fraction.chars().take(3).collect();
            format!("{digits:0<3}").parse::<i64>().ok()
        })
        .unwrap_or(Some(0))?;
    Some((seconds, millis))
}

fn cmd_export(args: &[String]) -> anyhow::Result<()> {
    let mut input: Option<String> = None;
    let mut format = "txt".to_string();
    let mut output: Option<String> = None;
    let mut timestamps = false;
    let mut speakers = false;
    let mut marks = false;
    let mut speaker_filter = SpeakerFilter::All;
    let mut title = "Transcript".to_string();

    // Parse flags
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--format" | "-f" => {
                i += 1;
                if i < args.len() {
                    format = args[i].clone();
                }
            }
            "--output" | "-o" => {
                i += 1;
                if i < args.len() {
                    output = Some(args[i].clone());
                }
            }
            "--timestamps" => timestamps = true,
            "--speakers" => speakers = true,
            "--marks" => marks = true,
            "--speaker-filter" => {
                i += 1;
                if i < args.len() {
                    speaker_filter = parse_speaker_filter(&args[i])?;
                    speakers = !matches!(speaker_filter, SpeakerFilter::Hidden);
                }
            }
            "--title" => {
                i += 1;
                if i < args.len() {
                    title = args[i].clone();
                }
            }
            arg if !arg.starts_with('-') && input.is_none() => {
                input = Some(arg.to_string());
            }
            _ => {}
        }
        i += 1;
    }

    let input_path =
        input.context("Missing input file. Usage: audraflow export <file.json> [options]")?;

    let input_transcript = read_transcript_from_json(&input_path)?;

    let options = ExportOptions {
        include_timestamps: timestamps,
        include_speakers: speakers,
        include_marks: marks,
        speaker_filter,
    };

    let normalized_format = normalize_format(&format)?;
    ensure_stdout_supported(&normalized_format, output.as_deref())?;
    let title = if title == "Transcript" && !input_transcript.title.is_empty() {
        input_transcript.title.as_str()
    } else {
        &title
    };
    let rendered = render_cli_export_with_metadata(
        &input_transcript.segments,
        &normalized_format,
        &input_transcript.transcript_id,
        &input_transcript.source_hash,
        title,
        &options,
    )?;
    write_rendered_output(rendered, &normalized_format, output.as_deref())?;

    Ok(())
}

fn cmd_batch(args: &[String]) -> anyhow::Result<()> {
    let mut input_dir: Option<String> = None;
    let mut format = "txt".to_string();
    let mut output_dir: Option<String> = None;
    let mut speaker_filter = SpeakerFilter::All;
    let mut include_speakers = true;
    let mut include_timestamps = true;
    let mut include_marks = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--format" | "-f" => {
                i += 1;
                if i < args.len() {
                    format = args[i].clone();
                }
            }
            "--output-dir" | "-o" => {
                i += 1;
                if i < args.len() {
                    output_dir = Some(args[i].clone());
                }
            }
            "--timestamps" => include_timestamps = true,
            "--no-timestamps" => include_timestamps = false,
            "--speakers" => include_speakers = true,
            "--no-speakers" => include_speakers = false,
            "--marks" => include_marks = true,
            "--speaker-filter" => {
                i += 1;
                if i < args.len() {
                    speaker_filter = parse_speaker_filter(&args[i])?;
                    include_speakers = !matches!(speaker_filter, SpeakerFilter::Hidden);
                }
            }
            arg if !arg.starts_with('-') && input_dir.is_none() => {
                input_dir = Some(arg.to_string());
            }
            _ => {}
        }
        i += 1;
    }

    let dir = input_dir.context("Missing input directory")?;
    let out_dir = output_dir.unwrap_or_else(|| ".".to_string());
    std::fs::create_dir_all(&out_dir)?;
    let normalized_format = normalize_format(&format)?;

    // Scan for JSON transcript files
    let mut count = 0;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                let transcript = match read_transcript_from_json(&path) {
                    Ok(transcript) => transcript,
                    Err(_) => continue,
                };

                let options = ExportOptions {
                    include_timestamps,
                    include_speakers,
                    include_marks,
                    speaker_filter,
                };

                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("output");
                let ext = format_ext(&normalized_format);
                let out_path = PathBuf::from(&out_dir).join(format!("{}.{}", stem, ext));

                std::fs::write(
                    &out_path,
                    render_cli_export_with_metadata(
                        &transcript.segments,
                        &normalized_format,
                        &transcript.transcript_id,
                        &transcript.source_hash,
                        if transcript.title.is_empty() {
                            stem
                        } else {
                            &transcript.title
                        },
                        &options,
                    )?,
                )?;
                eprintln!("  {}", out_path.display());
                count += 1;
            }
        }
    }

    eprintln!("Batch export complete: {} files", count);
    Ok(())
}

fn ensure_stdout_supported(format: &str, output_path: Option<&str>) -> anyhow::Result<()> {
    if format == "docx" && output_path.is_none() {
        anyhow::bail!("DOCX format requires --output <path> (binary format)");
    }
    Ok(())
}

fn ensure_existing_file(path: &Path, label: &str) -> anyhow::Result<()> {
    if path.is_file() {
        return Ok(());
    }
    anyhow::bail!("{label} not found: {}", path.display());
}

fn hash_file_sha256(path: &Path) -> anyhow::Result<String> {
    use std::io::Read;

    let mut file =
        std::fs::File::open(path).with_context(|| format!("Cannot hash {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn ensure_whisper_cli_hint_is_valid(path: &Path) -> anyhow::Result<()> {
    let explicit_path = path.is_absolute()
        || path
            .parent()
            .is_some_and(|parent| !parent.as_os_str().is_empty());
    if explicit_path && !path.is_file() {
        anyhow::bail!(
            "whisper-cli executable not found: {}. Pass --whisper-cli <path> or add whisper-cli to PATH.",
            path.display()
        );
    }
    Ok(())
}

fn write_rendered_output(
    rendered: Vec<u8>,
    format: &str,
    output_path: Option<&str>,
) -> anyhow::Result<()> {
    if let Some(out_path) = output_path {
        std::fs::write(out_path, rendered)?;
        eprintln!("Exported to: {out_path}");
        return Ok(());
    }

    ensure_stdout_supported(format, None)?;
    let text = String::from_utf8(rendered).context("Rendered export is not valid UTF-8")?;
    print!("{text}");
    Ok(())
}

fn temp_output_prefix() -> PathBuf {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("audraflow-whisper-{}-{millis}", std::process::id()))
}

fn display_command(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn preview_output(output: &[u8]) -> String {
    const MAX_CHARS: usize = 2000;
    let text = String::from_utf8_lossy(output);
    let trimmed = text.trim();
    if trimmed.chars().count() <= MAX_CHARS {
        return trimmed.to_string();
    }
    let preview: String = trimmed.chars().take(MAX_CHARS).collect();
    format!("{preview}...")
}

fn format_ext(format: &str) -> &str {
    match format {
        "md" | "markdown" => "md",
        "docx" => "docx",
        "srt" => "srt",
        "vtt" => "vtt",
        "json" => "json",
        "obsidian" | "notion" | "stdout-json" | "stdout-markdown" => "md",
        _ => "txt",
    }
}

struct TranscriptJsonInput {
    transcript_id: String,
    source_hash: String,
    title: String,
    segments: Vec<Segment>,
}

fn read_transcript_from_json(
    path: impl AsRef<std::path::Path>,
) -> anyhow::Result<TranscriptJsonInput> {
    let path = path.as_ref();
    let json_str = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read input file: {}", path.display()))?;
    parse_transcript_json(&json_str).or_else(|_| {
        let segments = parse_segments_json(&json_str)?;
        let title = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("Transcript")
            .to_string();
        Ok(TranscriptJsonInput {
            transcript_id: title.clone(),
            source_hash: String::new(),
            title,
            segments,
        })
    })
}

fn parse_transcript_json(json_str: &str) -> anyhow::Result<TranscriptJsonInput> {
    let json_str = json_str.trim_start_matches('\u{feff}');
    let export: TranscriptExport =
        serde_json::from_str(json_str).context("Invalid full transcript JSON format")?;
    Ok(TranscriptJsonInput {
        transcript_id: export.transcript_id,
        source_hash: export.source_hash,
        title: export.title,
        segments: export.segments,
    })
}

fn parse_segments_json(json_str: &str) -> anyhow::Result<Vec<Segment>> {
    let json_str = json_str.trim_start_matches('\u{feff}');
    serde_json::from_str(json_str).context("Invalid transcript JSON format")
}

fn parse_speaker_filter(value: &str) -> anyhow::Result<SpeakerFilter> {
    match value.trim().to_ascii_lowercase().as_str() {
        "all" => Ok(SpeakerFilter::All),
        "namedonly" | "named_only" | "named-only" => Ok(SpeakerFilter::NamedOnly),
        "hidden" | "hide" | "none" => Ok(SpeakerFilter::Hidden),
        other => anyhow::bail!("Unsupported speaker filter: {other}. Use all, namedOnly, hidden"),
    }
}

fn normalize_format(format: &str) -> anyhow::Result<String> {
    match format.trim().to_ascii_lowercase().as_str() {
        "txt" | "text" | "plain" => Ok("txt".into()),
        "md" | "markdown" | "stdout-markdown" | "stdoutmarkdown" => Ok("md".into()),
        "json" | "stdout-json" | "stdoutjson" => Ok("json".into()),
        "srt" => Ok("srt".into()),
        "vtt" => Ok("vtt".into()),
        "docx" => Ok("docx".into()),
        "obsidian" | "clipboard-obsidian" | "clipboardobsidian" => Ok("obsidian".into()),
        "notion" | "clipboard-notion" | "clipboardnotion" => Ok("notion".into()),
        other => anyhow::bail!(
            "Unsupported format: {other}. Use: txt, md, srt, vtt, json, docx, obsidian, notion"
        ),
    }
}

#[cfg(test)]
fn render_cli_export(
    segments: &[Segment],
    format: &str,
    title: &str,
    options: &ExportOptions,
) -> anyhow::Result<Vec<u8>> {
    render_cli_export_with_metadata(segments, format, title, "", title, options)
}

fn render_cli_export_with_metadata(
    segments: &[Segment],
    format: &str,
    transcript_id: &str,
    source_hash: &str,
    title: &str,
    options: &ExportOptions,
) -> anyhow::Result<Vec<u8>> {
    let bytes = match format {
        "txt" => export_txt(segments, options).into_bytes(),
        "md" => export_markdown(segments, title, options).into_bytes(),
        "srt" => export_srt_with_options(segments, options).into_bytes(),
        "vtt" => export_vtt_with_options(segments, options).into_bytes(),
        "json" => export_transcript_json(&TranscriptExport::new(
            transcript_id,
            source_hash,
            title,
            segments.to_vec(),
            options.clone(),
        ))
        .into_bytes(),
        "docx" => export_docx(segments, title, options)?,
        "obsidian" => {
            export_obsidian_callout(&filter_clipboard_speakers(segments, options)).into_bytes()
        }
        "notion" => {
            export_notion_toggle(&filter_clipboard_speakers(segments, options)).into_bytes()
        }
        _ => anyhow::bail!("Unsupported format: {format}"),
    };
    Ok(bytes)
}

fn filter_clipboard_speakers(segments: &[Segment], options: &ExportOptions) -> Vec<Segment> {
    segments
        .iter()
        .cloned()
        .map(|mut segment| {
            if !should_keep_speaker(segment.speaker_id.as_deref(), options) {
                segment.speaker_id = None;
            }
            segment
        })
        .collect()
}

fn should_keep_speaker(speaker: Option<&str>, options: &ExportOptions) -> bool {
    if !options.include_speakers {
        return false;
    }
    match options.speaker_filter {
        SpeakerFilter::Hidden => false,
        SpeakerFilter::All => speaker.is_some_and(|value| !value.trim().is_empty()),
        SpeakerFilter::NamedOnly => speaker.is_some_and(is_named_speaker),
    }
}

fn is_named_speaker(speaker: &str) -> bool {
    let normalized = speaker.trim().to_ascii_lowercase();
    !normalized.is_empty()
        && normalized != "speaker"
        && !normalized.strip_prefix("speaker ").is_some_and(|suffix| {
            suffix.len() == 1 && suffix.chars().all(|ch| ch.is_ascii_alphabetic())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_segment(id: &str, text: &str, speaker: Option<&str>) -> Segment {
        Segment {
            segment_id: id.into(),
            start_ms: 0,
            end_ms: 1_000,
            speaker_id: speaker.map(ToOwned::to_owned),
            text: text.into(),
            raw_text: text.into(),
            confidence: 0.9,
            low_confidence_reasons: vec![],
            corrections: vec![],
            marks: vec![],
        }
    }

    #[test]
    fn normalize_format_accepts_stdout_aliases() {
        assert_eq!(normalize_format("stdout-json").unwrap(), "json");
        assert_eq!(normalize_format("stdoutMarkdown").unwrap(), "md");
        assert_eq!(normalize_format("clipboard-obsidian").unwrap(), "obsidian");
    }

    #[test]
    fn parse_speaker_filter_accepts_frozen_variants() {
        assert!(matches!(
            parse_speaker_filter("all").unwrap(),
            SpeakerFilter::All
        ));
        assert!(matches!(
            parse_speaker_filter("namedOnly").unwrap(),
            SpeakerFilter::NamedOnly
        ));
        assert!(matches!(
            parse_speaker_filter("hidden").unwrap(),
            SpeakerFilter::Hidden
        ));
    }

    #[test]
    fn parse_segments_json_accepts_utf8_bom() {
        let json = "\u{feff}[{\"segmentId\":\"s1\",\"startMs\":0,\"endMs\":1000,\"speakerId\":null,\"text\":\"hello\",\"rawText\":\"hello\",\"confidence\":0.9,\"lowConfidenceReasons\":[],\"corrections\":[],\"marks\":[]}]";

        let segments = parse_segments_json(json).unwrap();

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "hello");
    }

    #[test]
    fn parse_transcript_json_accepts_full_schema() {
        let options = ExportOptions {
            include_timestamps: true,
            include_speakers: true,
            include_marks: true,
            speaker_filter: SpeakerFilter::All,
        };
        let json = export_transcript_json(&TranscriptExport::new(
            "job-42",
            "hash-42",
            "Board Meeting",
            vec![make_segment("s1", "hello", Some("Alice"))],
            options,
        ));

        let transcript = parse_transcript_json(&json).unwrap();

        assert_eq!(transcript.transcript_id, "job-42");
        assert_eq!(transcript.source_hash, "hash-42");
        assert_eq!(transcript.title, "Board Meeting");
        assert_eq!(transcript.segments.len(), 1);
        assert_eq!(transcript.segments[0].speaker_id.as_deref(), Some("Alice"));
    }

    #[test]
    fn read_transcript_from_json_accepts_legacy_segment_array() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy-transcript.json");
        let legacy_json = serde_json::to_string(&vec![make_segment("s1", "legacy", None)]).unwrap();
        std::fs::write(&path, legacy_json).unwrap();

        let transcript = read_transcript_from_json(&path).unwrap();

        assert_eq!(transcript.transcript_id, "legacy-transcript");
        assert_eq!(transcript.title, "legacy-transcript");
        assert_eq!(transcript.segments.len(), 1);
        assert_eq!(transcript.segments[0].text, "legacy");
    }

    #[test]
    fn render_cli_export_outputs_full_json_schema() {
        let options = ExportOptions {
            include_timestamps: true,
            include_speakers: true,
            include_marks: true,
            speaker_filter: SpeakerFilter::All,
        };

        let json = String::from_utf8(
            render_cli_export_with_metadata(
                &[make_segment("s1", "hello", Some("Alice"))],
                "json",
                "job-42",
                "hash-42",
                "Board Meeting",
                &options,
            )
            .unwrap(),
        )
        .unwrap();

        assert!(json.contains("\"transcriptId\": \"job-42\""));
        assert!(json.contains("\"sourceHash\": \"hash-42\""));
        assert!(json.contains("\"title\": \"Board Meeting\""));
        assert!(json.contains("\"segments\""));
        assert!(json.contains("\"exportOptions\""));
    }

    #[test]
    fn parse_transcribe_args_requires_model() {
        let args = vec!["audio.mp3".to_string()];

        let error = match parse_transcribe_args(&args) {
            Ok(_) => panic!("parse_transcribe_args should require --model"),
            Err(error) => error.to_string(),
        };

        assert!(error.contains("Missing --model"));
    }

    #[test]
    fn parse_transcribe_args_accepts_export_options() {
        let args = vec![
            "audio.mp3".to_string(),
            "--model".to_string(),
            "models/ggml-base.bin".to_string(),
            "--whisper-cli".to_string(),
            "C:/tools/whisper-cli.exe".to_string(),
            "--language".to_string(),
            "en".to_string(),
            "--format".to_string(),
            "stdout-markdown".to_string(),
            "--output".to_string(),
            "out.md".to_string(),
            "--timestamps".to_string(),
            "--speaker-filter".to_string(),
            "hidden".to_string(),
            "--title".to_string(),
            "Meeting".to_string(),
        ];

        let config = parse_transcribe_args(&args).unwrap();

        assert_eq!(config.input_path, PathBuf::from("audio.mp3"));
        assert_eq!(config.model_path, PathBuf::from("models/ggml-base.bin"));
        assert_eq!(
            config.whisper_cli,
            PathBuf::from("C:/tools/whisper-cli.exe")
        );
        assert_eq!(config.language, "en");
        assert_eq!(config.output_format, "md");
        assert_eq!(config.output_path.as_deref(), Some("out.md"));
        assert_eq!(config.title, "Meeting");
        assert!(config.export_options.include_timestamps);
        assert!(!config.export_options.include_speakers);
        assert!(matches!(
            config.export_options.speaker_filter,
            SpeakerFilter::Hidden
        ));
    }

    #[test]
    fn parse_whisper_json_accepts_transcription_offsets() {
        let json = r#"{
            "transcription": [
                {"offsets": {"from": 1200, "to": 3400}, "text": "hello"}
            ]
        }"#;

        let segments = parse_whisper_json(json).unwrap();

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].segment_id, "seg-0");
        assert_eq!(segments[0].start_ms, 1200);
        assert_eq!(segments[0].end_ms, 3400);
        assert_eq!(segments[0].text, "hello");
        assert_eq!(segments[0].raw_text, "hello");
    }

    #[test]
    fn parse_whisper_json_accepts_transcriptions_timestamps() {
        let json = r#"{
            "transcriptions": [
                {"timestamps": {"from": "00:01:02.345", "to": "00:01:03,500"}, "text": "world"}
            ]
        }"#;

        let segments = parse_whisper_json(json).unwrap();

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].start_ms, 62_345);
        assert_eq!(segments[0].end_ms, 63_500);
        assert_eq!(segments[0].text, "world");
    }

    #[test]
    fn render_cli_export_applies_speaker_filter_to_subtitles() {
        let segments = vec![
            make_segment("s1", "Named", Some("Alice")),
            make_segment("s2", "Placeholder", Some("Speaker A")),
        ];
        let options = ExportOptions {
            include_timestamps: true,
            include_speakers: true,
            include_marks: false,
            speaker_filter: SpeakerFilter::NamedOnly,
        };

        let srt = String::from_utf8(render_cli_export(&segments, "srt", "Test", &options).unwrap())
            .unwrap();

        assert!(srt.contains("[Alice] Named"));
        assert!(srt.contains("Placeholder"));
        assert!(!srt.contains("[Speaker A]"));
    }

    #[test]
    fn render_cli_export_filters_clipboard_speakers() {
        let segments = vec![make_segment("s1", "Hidden speaker", Some("Alice"))];
        let options = ExportOptions {
            include_timestamps: true,
            include_speakers: true,
            include_marks: false,
            speaker_filter: SpeakerFilter::Hidden,
        };

        let obsidian =
            String::from_utf8(render_cli_export(&segments, "obsidian", "Test", &options).unwrap())
                .unwrap();

        assert!(obsidian.contains("Hidden speaker"));
        assert!(!obsidian.contains("Alice"));
    }
}
