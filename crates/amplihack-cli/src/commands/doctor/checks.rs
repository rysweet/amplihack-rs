//! Individual health check implementations for `amplihack doctor`.

use super::{MAX_ERROR_LEN, MAX_VERSION_LEN, json_contains_amplihack, settings_json_path};
use crate::util::{run_output_with_timeout, strip_ansi, truncate_chars_with_notice};
use std::process::Command;
use std::time::Duration;

const DOCTOR_COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

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
                    truncate_chars_with_notice(&msg, MAX_ERROR_LEN)
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
                    truncate_chars_with_notice(&msg, MAX_ERROR_LEN)
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
    let mut command = Command::new("recipe-runner-rs");
    command.arg("--version");
    let output = run_output_with_timeout(command, DOCTOR_COMMAND_TIMEOUT);

    match output {
        Ok(out) if out.status.success() => {
            let stripped = sanitized_single_line(&out.stdout);
            let version = truncate_chars_with_notice(&stripped, MAX_VERSION_LEN);
            (true, format!("recipe-runner-rs {version}"))
        }
        Ok(out) => {
            let msg = sanitized_single_line(&out.stderr);
            (
                false,
                format!(
                    "recipe-runner-rs: {}",
                    truncate_chars_with_notice(&msg, MAX_ERROR_LEN)
                ),
            )
        }
        Err(e) => {
            let msg = e.to_string();
            (
                false,
                format!(
                    "recipe-runner-rs not found on PATH: {}",
                    truncate_chars_with_notice(&msg, MAX_ERROR_LEN)
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
    let mut command = Command::new("tmux");
    command.arg("-V");
    let output = run_output_with_timeout(command, DOCTOR_COMMAND_TIMEOUT);

    match output {
        Ok(out) if out.status.success() => {
            let stripped = sanitized_single_line(&out.stdout);
            let version = truncate_chars_with_notice(&stripped, MAX_VERSION_LEN);
            (true, version)
        }
        Ok(out) => {
            let msg = sanitized_single_line(&out.stderr);
            (
                false,
                format!("tmux: {}", truncate_chars_with_notice(&msg, MAX_ERROR_LEN)),
            )
        }
        Err(e) => {
            let msg = e.to_string();
            (
                false,
                format!(
                    "tmux not found: {}",
                    truncate_chars_with_notice(&msg, MAX_ERROR_LEN)
                ),
            )
        }
    }
}

fn sanitized_single_line(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes).replace(['\r', '\n'], " ");
    strip_ansi(text.trim())
}

/// Check 6 — amplihack binary version (compile-time constant).
///
/// Returns the version baked in at compile time. Prefers the
/// `AMPLIHACK_RELEASE_VERSION` env override set by the release workflow and
/// falls back to `CARGO_PKG_VERSION` for dev builds. See `amplihack_cli::VERSION`.
/// This check always passes on a valid install and cannot fail at runtime.
pub fn check_amplihack_version() -> (bool, String) {
    let version = crate::VERSION;
    (true, format!("amplihack v{version}"))
}
