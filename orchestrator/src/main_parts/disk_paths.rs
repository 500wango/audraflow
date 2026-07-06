fn low_disk_message(status: &DiskSpaceStatus) -> String {
    format!(
        "Paused: free disk space is below {:.0} MB (available {:.0} MB). Free disk space and resume the job.",
        bytes_to_mb(status.min_free_bytes),
        bytes_to_mb(status.available_bytes)
    )
}

fn bytes_to_mb(bytes: u64) -> f64 {
    bytes as f64 / 1024.0 / 1024.0
}

fn disk_check_target(path: &Path) -> PathBuf {
    if path.is_dir() {
        return path.to_path_buf();
    }

    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(target_os = "windows")]
fn available_disk_bytes(target: &Path) -> anyhow::Result<u64> {
    use anyhow::Context as _;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let wide: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut available_bytes = 0u64;
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available_bytes,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };

    if ok == 0 {
        Err(std::io::Error::last_os_error())
            .with_context(|| format!("failed to read free disk space for {}", target.display()))
    } else {
        Ok(available_bytes)
    }
}

#[cfg(unix)]
fn available_disk_bytes(target: &Path) -> anyhow::Result<u64> {
    use anyhow::Context as _;
    use std::ffi::CString;
    use std::mem::MaybeUninit;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(target.as_os_str().as_bytes())
        .with_context(|| format!("disk check path contains a NUL byte: {}", target.display()))?;
    let mut stat = MaybeUninit::<libc::statvfs>::uninit();
    let rc = unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) };

    if rc != 0 {
        Err(std::io::Error::last_os_error())
            .with_context(|| format!("failed to read free disk space for {}", target.display()))
    } else {
        let stat = unsafe { stat.assume_init() };
        let available = (stat.f_bavail as u128).saturating_mul(stat.f_frsize as u128);
        Ok(available.min(u64::MAX as u128) as u64)
    }
}

#[cfg(all(not(target_os = "windows"), not(unix)))]
fn available_disk_bytes(_target: &Path) -> anyhow::Result<u64> {
    Ok(u64::MAX)
}

fn get_db_path() -> anyhow::Result<PathBuf> {
    let dir = app_data_dir();
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("audraflow.db"))
}

fn app_data_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("AudraFlow");
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
