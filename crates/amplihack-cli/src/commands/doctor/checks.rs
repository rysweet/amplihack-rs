//! Individual health check implementations for `amplihack doctor`.

use crate::util::strip_ansi;
use super::{MAX_ERROR_LEN, MAX_VERSION_LEN, settings_json_path, truncate, json_contains_amplihack};

/// Check 1 — amplihack hooks installed.
///
/// Reads `$HOME/.claude/settings.json` and verifies that the `hooks` section
/// contains at least one entry whose value contains the substring
/// `"amplihack"`.
pub fn check_hooks_installed() -> (bool, String) {
    let path = match settings_json_path() {
        None => return (false, "hooks: $HOME not set".to_string()),
        Some(p) => p,
    };

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            let msg = e.to_string();
            return (
                false,
                format!(
                    "hooks: cannot read settings.json: {}",
                    truncate(&msg, MAX_ERROR_LEN)
                ),
            );
        }
    };

    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return (false, "hooks: settings.json is not valid JSON".to_string()),
    };

    if let Some(hooks) = json.get("hooks")
        && json_contains_amplihack(hooks)
    {
        return (true, "amplihack hooks installed".to_string());
    }

    (
        false,
        "amplihack hooks not found in settings.json".to_string(),
    )
}

/// Check 2 — settings.json valid JSON.
///
/// Reads `$HOME/.claude/settings.json` (if present) and attempts to parse it
/// with `serde_json`.  Only existence, validity, and the presence of the
/// `"amplihack"` string are reported — content is never printed.  See
/// SEC-WS2-04.
pub fn check_settings_valid_json() -> (bool, String) {
    let path = match settings_json_path() {
        None => return (false, "settings.json: $HOME not set".to_string()),
        Some(p) => p,
    };

    if !path.exists() {
        return (false, "settings.json: file not found".to_string());
    }

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            let msg = e.to_string();
            return (
                false,
                format!(
                    "settings.json: cannot read: {}",
                    truncate(&msg, MAX_ERROR_LEN)
                ),
            );
        }
    };

    match serde_json::from_str::<serde_json::Value>(&content) {
        Ok(_) => (true, "settings.json is valid JSON".to_string()),
        Err(_) => (false, "settings.json: invalid JSON".to_string()),
    }
}

/// Check 3 — recipe-runner-rs available and responsive.
///
/// Locates `recipe-runner-rs` on `$PATH` and runs `recipe-runner-rs
/// --version`, reporting the version string on success.
///
/// SAFETY: `"recipe-runner-rs"` and `"--version"` are compile-time literals;
/// no user input is passed to the subprocess.
pub fn check_recipe_runner_available() -> (bool, String) {
    // SAFETY: all arguments are compile-time literals — no user input.
    let output = std::process::Command::new("recipe-runner-rs")
        .arg("--version")
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let raw = String::from_utf8_lossy(&out.stdout);
            let first_line = raw.lines().next().unwrap_or("").trim();
            let stripped = strip_ansi(first_line);
            let version = truncate(&stripped, MAX_VERSION_LEN).to_string();
            (true, format!("recipe-runner-rs {version}"))
        }
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            let first_line = err.lines().next().unwrap_or("").trim().to_string();
            let msg = strip_ansi(&first_line);
            (
                false,
                format!("recipe-runner-rs: {}", truncate(&msg, MAX_ERROR_LEN)),
            )
        }
        Err(e) => {
            let msg = e.to_string();
            (
                false,
                format!(
                    "recipe-runner-rs not found on PATH: {}",
                    truncate(&msg, MAX_ERROR_LEN)
                ),
            )
        }
    }
}

/// Check 4 — tmux installed.
///
/// Runs `tmux -V` and extracts the version string from the first line of
/// stdout.  `strip_ansi()` is applied before display.  See SEC-WS2-02.
///
/// SAFETY: `"tmux"` and `"-V"` are compile-time literals.
pub fn check_tmux_installed() -> (bool, String) {
    // SAFETY: all arguments are compile-time literals — no user input.
    let output = std::process::Command::new("tmux").arg("-V").output();

    match output {
        Ok(out) if out.status.success() => {
            let raw = String::from_utf8_lossy(&out.stdout);
            let first_line = raw.lines().next().unwrap_or("").trim();
            let stripped = strip_ansi(first_line);
            let version = truncate(&stripped, MAX_VERSION_LEN).to_string();
            (true, version)
        }
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            let first_line = err.lines().next().unwrap_or("").trim().to_string();
            let msg = strip_ansi(&first_line);
            (false, format!("tmux: {}", truncate(&msg, MAX_ERROR_LEN)))
        }
        Err(e) => {
            let msg = e.to_string();
            (
                false,
                format!("tmux not found: {}", truncate(&msg, MAX_ERROR_LEN)),
            )
        }
    }
}

/// Check 6 — amplihack binary version (compile-time constant).
///
/// Returns the version baked in at compile time via `env!("CARGO_PKG_VERSION")`.
/// This check always passes on a valid install and cannot fail at runtime.
pub fn check_amplihack_version() -> (bool, String) {
    let version = env!("CARGO_PKG_VERSION");
    (true, format!("amplihack v{version}"))
}
