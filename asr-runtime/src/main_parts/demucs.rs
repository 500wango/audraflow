#[derive(Debug, Clone)]
struct DemucsInvocation {
    program: OsString,
    base_args: Vec<OsString>,
    envs: Vec<(OsString, OsString)>,
    label: String,
}

fn run_demucs_vocal_separation(input_path: &Path, temp_dir: &Path) -> anyhow::Result<PathBuf> {
    let demucs = resolve_demucs_invocation()
        .context("Demucs runtime was not found; reinstall AudraFlow or set AUDRAFLOW_DEMUCS_BIN")?;
    let output_dir = temp_dir.join("demucs");
    std::fs::create_dir_all(&output_dir)?;

    log::info!(
        "Running Demucs vocal separation with {}: {}",
        demucs.label,
        input_path.display()
    );

    let mut command = Command::new(&demucs.program);
    command
        .args(&demucs.base_args)
        .arg("--two-stems")
        .arg("vocals")
        .arg("--out")
        .arg(&output_dir)
        .arg(input_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in &demucs.envs {
        command.env(name, value);
    }
    let output = command
        .output()
        .with_context(|| format!("failed to start {}", demucs.label))?;

    if !output.status.success() {
        anyhow::bail!(
            "{} exited with {}. stderr: {} stdout: {}",
            demucs.label,
            output.status,
            preview_text(&output.stderr),
            preview_text(&output.stdout)
        );
    }

    find_vocals_output(&output_dir).with_context(|| {
        format!(
            "demucs did not produce vocals.wav under {}",
            output_dir.display()
        )
    })
}

fn resolve_demucs_invocation() -> Option<DemucsInvocation> {
    if let Some(program) = demucs_env_override() {
        let invocation = DemucsInvocation {
            label: program.to_string_lossy().into_owned(),
            program,
            base_args: Vec::new(),
            envs: Vec::new(),
        };
        if probe_demucs(&invocation) {
            return Some(invocation);
        }
        log::warn!("AUDRAFLOW_DEMUCS_BIN is set but is not runnable as demucs");
    }

    if let Some(invocation) = managed_python_demucs_invocation() {
        if probe_demucs(&invocation) {
            return Some(invocation);
        }
        log::warn!("Managed Python runtime was found but could not run demucs");
    }

    if let Some(invocation) = bundled_python_demucs_invocation() {
        if probe_demucs(&invocation) {
            return Some(invocation);
        }
        log::warn!("Bundled Python runtime was found but could not run demucs");
    }

    let candidates = [
        DemucsInvocation {
            program: OsString::from("demucs"),
            base_args: Vec::new(),
            envs: Vec::new(),
            label: "demucs".into(),
        },
        DemucsInvocation {
            program: OsString::from("python3"),
            base_args: vec![OsString::from("-m"), OsString::from("demucs")],
            envs: Vec::new(),
            label: "python3 -m demucs".into(),
        },
        DemucsInvocation {
            program: OsString::from("python"),
            base_args: vec![OsString::from("-m"), OsString::from("demucs")],
            envs: Vec::new(),
            label: "python -m demucs".into(),
        },
        DemucsInvocation {
            program: OsString::from("py"),
            base_args: vec![
                OsString::from("-3"),
                OsString::from("-m"),
                OsString::from("demucs"),
            ],
            envs: Vec::new(),
            label: "py -3 -m demucs".into(),
        },
    ];

    candidates.into_iter().find(probe_demucs)
}

fn managed_python_demucs_invocation() -> Option<DemucsInvocation> {
    let python = managed_python_bin();
    if !python.is_file() {
        return None;
    }
    let python_dir = python.parent().unwrap_or_else(|| Path::new(""));
    let envs = bundled_python_envs(python_dir);
    Some(DemucsInvocation {
        label: format!("{} -m demucs", python.display()),
        program: python.into_os_string(),
        base_args: vec![OsString::from("-m"), OsString::from("demucs")],
        envs,
    })
}

fn managed_python_bin() -> PathBuf {
    let venv_dir = app_data_dir()
        .join("runtime")
        .join("components")
        .join("python-venv");
    if cfg!(windows) {
        venv_dir.join("Scripts").join("python.exe")
    } else {
        venv_dir.join("bin").join("python")
    }
}

fn bundled_python_demucs_invocation() -> Option<DemucsInvocation> {
    for root in runtime_search_roots() {
        for python in [
            root.join("bin").join("python").join("python.exe"),
            root.join("resources")
                .join("bin")
                .join("python")
                .join("python.exe"),
            root.join("python").join("python.exe"),
        ] {
            if !python.is_file() {
                continue;
            }
            let python_dir = python.parent().unwrap_or_else(|| Path::new(""));
            let envs = bundled_python_envs(python_dir);
            return Some(DemucsInvocation {
                label: format!("{} -m demucs", python.display()),
                program: python.into_os_string(),
                base_args: vec![OsString::from("-m"), OsString::from("demucs")],
                envs,
            });
        }
    }
    None
}

fn bundled_python_envs(python_dir: &Path) -> Vec<(OsString, OsString)> {
    vec![
        (OsString::from("PYTHONUTF8"), OsString::from("1")),
        (OsString::from("PYTHONNOUSERSITE"), OsString::from("1")),
        (
            OsString::from("TORCH_HOME"),
            python_dir.join("torch-cache").into_os_string(),
        ),
        (
            OsString::from("HF_HOME"),
            python_dir.join("hf-cache").into_os_string(),
        ),
        (
            OsString::from("MODELSCOPE_CACHE"),
            python_dir.join("modelscope-cache").into_os_string(),
        ),
    ]
}

fn app_data_dir() -> PathBuf {
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

fn runtime_search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        roots.extend(exe.ancestors().map(Path::to_path_buf));
    }
    if let Ok(cwd) = std::env::current_dir() {
        roots.extend(cwd.ancestors().map(Path::to_path_buf));
    }

    let mut deduped = Vec::new();
    for root in roots {
        if !deduped.contains(&root) {
            deduped.push(root);
        }
    }
    deduped
}

fn demucs_env_override() -> Option<OsString> {
    std::env::var_os("AUDRAFLOW_DEMUCS_BIN")
        .or_else(|| std::env::var_os("DEMUCS_BIN"))
        .filter(|value| !value.is_empty())
}

fn probe_demucs(invocation: &DemucsInvocation) -> bool {
    let mut command = Command::new(&invocation.program);
    command
        .args(&invocation.base_args)
        .arg("--help")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (name, value) in &invocation.envs {
        command.env(name, value);
    }
    command
        .status()
        .is_ok_and(|status| status.success())
}

fn find_vocals_output(output_dir: &Path) -> anyhow::Result<PathBuf> {
    let mut candidates = Vec::new();
    collect_vocals_outputs(output_dir, &mut candidates)?;
    candidates.sort_by_key(|path| {
        std::fs::metadata(path)
            .map(|metadata| std::cmp::Reverse(metadata.len()))
            .unwrap_or(std::cmp::Reverse(0))
    });
    candidates
        .into_iter()
        .next()
        .context("vocals.wav was not found")
}

fn collect_vocals_outputs(dir: &Path, candidates: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    if !dir.exists() {
        return Ok(());
    }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_vocals_outputs(&path, candidates)?;
            continue;
        }

        let file_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        if file_name == "vocals.wav" || (stem == "vocals" && extension == "wav") {
            candidates.push(path);
        }
    }

    Ok(())
}

fn preview_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    if trimmed.chars().count() <= 500 {
        trimmed.to_string()
    } else {
        format!("{}...", trimmed.chars().take(500).collect::<String>())
    }
}

fn truncate_message(message: &str, max_chars: usize) -> String {
    if message.chars().count() <= max_chars {
        message.to_string()
    } else {
        format!("{}...", message.chars().take(max_chars).collect::<String>())
    }
}
