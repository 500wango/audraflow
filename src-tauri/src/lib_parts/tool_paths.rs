fn whisper_cli_command() -> PathBuf {
    command_env_override("AUDRAFLOW_WHISPER_CLI")
        .or_else(|| command_env_override("FT_WHISPER_CLI"))
        .or_else(|| find_runtime_component_tool("whisper", whisper_cli_binary_name()))
        .or_else(|| find_bundled_command(whisper_cli_binary_name()))
        .or_else(|| find_dev_or_portable_tool(whisper_cli_binary_name()))
        .or_else(|| find_system_command(whisper_cli_binary_name()))
        .unwrap_or_else(|| PathBuf::from(whisper_cli_binary_name()))
}

fn funasr_cli_command() -> PathBuf {
    command_env_override("AUDRAFLOW_FUNASR_CLI")
        .or_else(|| command_env_override("FT_FUNASR_CLI"))
        .or_else(|| find_runtime_component_tool("funasr", funasr_cli_binary_name()))
        .or_else(|| find_bundled_command(funasr_cli_binary_name()))
        .or_else(|| find_dev_or_portable_tool(funasr_cli_binary_name()))
        .or_else(|| find_system_command(funasr_cli_binary_name()))
        .unwrap_or_else(|| PathBuf::from(funasr_cli_binary_name()))
}

fn whisper_cli_binary_name() -> &'static str {
    if cfg!(windows) {
        "whisper-cli.exe"
    } else {
        "whisper-cli"
    }
}

fn funasr_cli_binary_name() -> &'static str {
    if cfg!(windows) {
        "llama-funasr-cli.exe"
    } else {
        "llama-funasr-cli"
    }
}

fn tool_binary_name(name: &'static str) -> &'static str {
    match (cfg!(windows), name) {
        (true, "ffmpeg") => "ffmpeg.exe",
        (true, "ffprobe") => "ffprobe.exe",
        _ => name,
    }
}

fn find_dev_or_portable_tool(name: &str) -> Option<PathBuf> {
    for root in runtime_search_roots() {
        for candidate in [
            root.join(name),
            root.join("bin").join(name),
            root.join("release")
                .join("linux-portable")
                .join("AudraFlow")
                .join("bin")
                .join(name),
            root.join("release")
                .join("windows-portable")
                .join("AudraFlow")
                .join("bin")
                .join(name),
            root.join("external")
                .join("whisper.cpp")
                .join("build-linux")
                .join("bin")
                .join(name),
            root.join("external")
                .join("whisper.cpp")
                .join("build")
                .join("bin")
                .join(name),
            root.join("external")
                .join("Fun-ASR")
                .join("runtime")
                .join("llama.cpp")
                .join("build")
                .join("bin")
                .join(name),
            root.join("external")
                .join("funasr-llamacpp")
                .join("bin")
                .join(name),
        ] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        if let Some(path) = find_staged_binary(&root, name) {
            return Some(path);
        }
    }
    None
}

fn find_staged_binary(root: &Path, name: &str) -> Option<PathBuf> {
    let stem = name.strip_suffix(".exe").unwrap_or(name);
    let prefixed_stem = format!("audraflow-{stem}");
    for dir in [
        root.join("src-tauri").join("binaries"),
        root.join("binaries"),
    ] {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
                    continue;
                };
                if file_name == name
                    || file_name.starts_with(&format!("{stem}-"))
                    || file_name.starts_with(&format!("{prefixed_stem}-"))
                {
                    return Some(path);
                }
            }
        }
    }
    None
}

fn runtime_search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(resource_dir) = std::env::var_os("AUDRAFLOW_RESOURCE_DIR") {
        roots.push(PathBuf::from(resource_dir));
    }
    if let Ok(exe) = std::env::current_exe() {
        roots.extend(exe.ancestors().map(Path::to_path_buf));
    }
    if let Ok(cwd) = std::env::current_dir() {
        roots.extend(cwd.ancestors().map(Path::to_path_buf));
    }
    dedupe_path_list(roots)
}

fn dedupe_path_list(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut deduped = Vec::new();
    for path in paths {
        if !deduped.contains(&path) {
            deduped.push(path);
        }
    }
    deduped
}

fn command_display_path(path: &Path) -> String {
    if path.is_absolute() || path.components().count() > 1 {
        return path.to_string_lossy().into_owned();
    }
    path.file_name()
        .and_then(|value| value.to_str())
        .and_then(find_system_command)
        .unwrap_or_else(|| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn first_output_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(truncate_runtime_text)
}

fn short_output(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(truncate_runtime_text(trimmed))
    }
}

fn truncate_runtime_text(text: &str) -> String {
    const MAX_CHARS: usize = 180;
    if text.chars().count() <= MAX_CHARS {
        text.to_string()
    } else {
        format!("{}...", text.chars().take(MAX_CHARS).collect::<String>())
    }
}

fn directory_size_bytes(path: &Path) -> Result<u64, String> {
    if !path.exists() {
        return Ok(0);
    }
    let mut total = 0u64;
    for entry in std::fs::read_dir(path).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let meta = entry.metadata().map_err(|e| e.to_string())?;
        if meta.is_dir() {
            total += directory_size_bytes(&entry.path())?;
        } else {
            total += meta.len();
        }
    }
    Ok(total)
}

fn count_directory_children(path: &Path) -> Result<u64, String> {
    if !path.exists() {
        return Ok(0);
    }
    Ok(std::fs::read_dir(path)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .count() as u64)
}

fn clear_directory_children(path: &Path) -> Result<u64, String> {
    if !path.exists() {
        return Ok(0);
    }
    let mut removed = 0u64;
    for entry in std::fs::read_dir(path).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let entry_path = entry.path();
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            std::fs::remove_dir_all(&entry_path).map_err(|e| e.to_string())?;
        } else {
            std::fs::remove_file(&entry_path).map_err(|e| e.to_string())?;
        }
        removed += 1;
    }
    Ok(removed)
}
