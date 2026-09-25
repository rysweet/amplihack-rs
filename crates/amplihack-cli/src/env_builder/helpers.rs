use std::collections::HashSet;
use std::env;
use std::path::{Path, PathBuf};

/// Resolves which agent binary identifier the current process is operating
/// under. Delegates to [`amplihack_utils::agent_binary::resolve`], which
/// applies the precedence:
///
/// 1. `AMPLIHACK_AGENT_BINARY` env var (explicit override).
/// 2. A live session marker (`CLAUDE_CODE_SESSION_ID`, `COPILOT_CLI`, ...).
/// 3. `<cwd-or-ancestor>/.claude/runtime/launcher_context.json`.
/// 4. Built-in default `"copilot"`.
///
/// Always returns an allowlisted name (`claude`, `copilot`, `codex`, or
/// `amplifier`). Unknown / dangerous overrides silently fall through to the
/// next layer — they never reach `Command::new`.
pub fn active_agent_binary() -> String {
    active_agent_binary_with_source().0
}

/// [`active_agent_binary`] plus the layer that supplied the answer.
///
/// Callers that export the result to children use this so a guess from the
/// built-in default can be tagged as one (issue #1481).
pub fn active_agent_binary_with_source() -> (String, amplihack_utils::agent_binary::ResolutionSource)
{
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    active_agent_binary_with_source_in(&cwd)
}

/// [`active_agent_binary_with_source`] with the launcher-context walk-up
/// starting at `dir` instead of the process cwd.
///
/// `amplihack recipe run -w <dir>` runs every step in `<dir>`, so that is where
/// a nested `amplihack` would look for `launcher_context.json`. Resolving the
/// run's binary from anywhere else can let one level guess `copilot` while the
/// level below reads a context file the top never saw.
pub fn active_agent_binary_with_source_in(
    dir: &Path,
) -> (String, amplihack_utils::agent_binary::ResolutionSource) {
    match amplihack_utils::agent_binary::resolve_with_source(dir) {
        Ok(resolved) => resolved,
        Err(err) => {
            tracing::warn!(error = %err, "agent binary resolver failed; using built-in default");
            (
                amplihack_utils::agent_binary::DEFAULT_BINARY.to_string(),
                amplihack_utils::agent_binary::ResolutionSource::Default,
            )
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

    // Issue #1274 — one seam. A "choose a file to run" walk: the result is
    // handed to `Command::new`, so an empty `$PATH` element used to make it
    // possible to run an `amplihack-asset-resolver` out of the current
    // directory.
    for dir in amplihack_utils::launch_target::env_path_dirs() {
        let candidate = dir.join("amplihack-asset-resolver");
        if candidate.is_file() {
            return Some(candidate);
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

    // Then existing PATH entries.
    //
    // Issue #1274 — one seam, and this is a call that deliberately differs.
    // `RelativeEntries::Keep`: this REBUILDS `$PATH` for a child process.
    // Dropping the user's own relative entries on the way past would silently
    // edit the environment of the agent and every one of its shell-outs, which
    // is a change amplihack has no business making here. The launch path's
    // protection against cwd-first resolution is that no *candidate binary* is
    // ever chosen from a relative entry — see `launch_target::path_dirs`.
    for entry in amplihack_utils::launch_target::split_path_var(
        std::ffi::OsStr::new(current),
        amplihack_utils::launch_target::RelativeEntries::Keep,
    ) {
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
        if let Ok(n) = reader.read(&mut buf)
            && let Ok(first_line) = std::str::from_utf8(&buf[..n])
            && let Some(line) = first_line.lines().next()
        {
            return line.starts_with("#!") && (line.contains("python") || line.contains("Python"));
        }
    }
    false
}
