//! Process metadata helpers used when recording workstream launches.

use std::fs;
use std::path::{Path, PathBuf};

pub fn process_start_metadata(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        stat.rsplit_once(") ")
            .and_then(|(_, rest)| rest.split_whitespace().nth(19))
            .map(str::to_string)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

pub fn normalize_path(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|_| PathBuf::from(path))
        .to_string_lossy()
        .to_string()
}
