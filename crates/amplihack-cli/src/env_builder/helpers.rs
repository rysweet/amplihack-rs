use std::collections::HashSet;
use std::env;
use std::path::PathBuf;

pub fn active_agent_binary() -> String {
    match env::var("AMPLIHACK_AGENT_BINARY") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            tracing::warn!(
                "AMPLIHACK_AGENT_BINARY not set; defaulting to 'claude'. This usually means a subprocess was launched outside the amplihack CLI dispatcher."
            );
            "claude".to_string()
        }
    }
}

pub(super) fn find_asset_resolver_binary() -> Option<PathBuf> {
    if let Ok(exe) = env::current_exe()
        && let Some(parent) = exe.parent()
    {
        let sibling = parent.join("amplihack-asset-resolver");
        if sibling.is_file() {
            return Some(sibling);
        }
    }

    if let Ok(path) = env::var("PATH") {
        for dir in env::split_paths(&path) {
            let candidate = dir.join("amplihack-asset-resolver");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    if let Ok(home) = env::var("HOME") {
        for suffix in [".local/bin", ".cargo/bin"] {
            let candidate = PathBuf::from(&home)
                .join(suffix)
                .join("amplihack-asset-resolver");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

/// Build a PATH string by prepending directories and deduplicating.
pub(super) fn build_path(prepend: &[PathBuf], current: &str) -> String {
    let mut seen = HashSet::new();
    let mut parts = Vec::new();

    // Prepend entries first (higher priority)
    for dir in prepend {
        let s = dir.to_string_lossy().to_string();
        if seen.insert(s.clone()) {
            parts.push(s);
        }
    }

    // Then existing PATH entries
    for entry in env::split_paths(current) {
        let s = entry.to_string_lossy().to_string();
        if seen.insert(s.clone()) {
            parts.push(s);
        }
    }

    env::join_paths(parts.iter().map(|s| s.as_str()))
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Generate a simple session ID (timestamp + PID).
pub(super) fn generate_session_id() -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("rs-{}-{}", ts, std::process::id())
}

pub(super) fn session_tree_context_present() -> bool {
    [
        "AMPLIHACK_TREE_ID",
        "AMPLIHACK_SESSION_DEPTH",
        "AMPLIHACK_MAX_DEPTH",
        "AMPLIHACK_MAX_SESSIONS",
    ]
    .iter()
    .any(|key| env::var_os(key).is_some())
}

pub(super) fn resolve_session_tree_id() -> String {
    env::var("AMPLIHACK_TREE_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            generate_session_id()
                .chars()
                .filter(|ch| *ch != '-')
                .take(8)
                .collect()
        })
}

pub(super) fn resolve_session_tree_depth(increment: bool) -> String {
    let base = match env::var("AMPLIHACK_SESSION_DEPTH") {
        Ok(raw) if !raw.trim().is_empty() => match raw.parse::<u32>() {
            Ok(parsed) => parsed,
            Err(error) => {
                tracing::warn!(
                    value = raw,
                    "invalid AMPLIHACK_SESSION_DEPTH, defaulting to 0: {error}"
                );
                0
            }
        },
        _ => 0,
    };
    let depth = if increment {
        base.saturating_add(1)
    } else {
        base
    };
    depth.to_string()
}

/// Check whether a PATH/PYTHONPATH entry looks like it references the Python
/// amplihack package (not amplihack-rs).
pub(super) fn is_python_amplihack_path(entry: &str) -> bool {
    if entry.is_empty() {
        return false;
    }
    // Match paths containing amplihack that aren't amplihack-rs
    let lower = entry.to_lowercase();
    (lower.contains("amplihack") && !lower.contains("amplihack-rs") && !lower.contains("amplihack_rs"))
        // Also catch pip/site-packages installs
        || (lower.contains("site-packages") && lower.contains("amplihack"))
}

/// Check whether a file is a Python script by reading its shebang or checking extension.
pub(super) fn is_file_python_script(path: &std::path::Path) -> bool {
    if path.extension().and_then(|e| e.to_str()) == Some("py") {
        return true;
    }
    // Read just enough bytes to check for a Python shebang
    if let Ok(file) = std::fs::File::open(path) {
        use std::io::Read;
        let mut buf = [0u8; 128];
        let mut reader = std::io::BufReader::new(file);
        if let Ok(n) = reader.read(&mut buf) {
            if let Ok(first_line) = std::str::from_utf8(&buf[..n]) {
                if let Some(line) = first_line.lines().next() {
                    return line.starts_with("#!")
                        && (line.contains("python") || line.contains("Python"));
                }
            }
        }
    }
    false
}
