//! Individual health check implementations for `amplihack doctor`.

use super::{
    MAX_ERROR_LEN, MAX_VERSION_LEN, json_contains_amplihack, settings_json_path,
    settings_json_path_for,
};
use crate::util::{run_output_with_timeout, strip_ansi, truncate_chars_with_notice};
use std::process::Command;
use std::time::Duration;

const DOCTOR_COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

/// Check 1 — amplihack hooks installed.
///
/// Passes if EITHER the global `$HOME/.claude/settings.json` OR the
/// project-local `<cwd>/.claude/settings.json` has a `hooks` section that
/// contains the substring `"amplihack"`. Project-local installs are valid, so
/// a working install must not be reported as missing (issue #1088).
///
/// File contents are never printed; only presence/validity is reported and
/// error strings are truncated (SEC-WS2-04).
pub fn check_hooks_installed() -> (bool, String) {
    // Global settings take precedence, then the project-local copy. Pass if
    // EITHER location has amplihack hooks. A missing file (`None`) or one
    // without amplihack hooks (`Some(Ok(false))`) falls through to the next
    // candidate. A read/parse error on one candidate must NOT mask a valid
    // install in the other, so errors are remembered and only surfaced when no
    // candidate yields amplihack hooks.
    let candidates = [
        settings_json_path(),
        std::env::current_dir()
            .ok()
            .map(|cwd| settings_json_path_for(&cwd)),
    ];

    let mut first_error: Option<String> = None;
    for path in candidates.into_iter().flatten() {
        match settings_has_amplihack_hooks(&path) {
            Some(Ok(true)) => return (true, "amplihack hooks installed".to_string()),
            Some(Err(msg)) => {
                if first_error.is_none() {
                    first_error = Some(msg);
                }
            }
            Some(Ok(false)) | None => {}
        }
    }

    if let Some(msg) = first_error {
        return (false, msg);
    }

    (
        false,
        "amplihack hooks not found in settings.json (checked global \
         ~/.claude/settings.json and project-local .claude/settings.json)"
            .to_string(),
    )
}

/// Read `path` and report whether its `hooks` section references amplihack.
///
/// Returns `None` when the file is absent (nothing to report for this
/// location), `Some(Ok(bool))` when it parses, and `Some(Err(msg))` for a
/// read/parse error whose message is truncated and never includes file
/// contents.
fn settings_has_amplihack_hooks(path: &std::path::Path) -> Option<Result<bool, String>> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            let msg = e.to_string();
            return Some(Err(format!(
                "hooks: cannot read settings.json: {}",
                truncate_chars_with_notice(&msg, MAX_ERROR_LEN)
            )));
        }
    };

    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return Some(Err("hooks: settings.json is not valid JSON".to_string())),
    };

    let has_hooks = json
        .get("hooks")
        .map(json_contains_amplihack)
        .unwrap_or(false);
    Some(Ok(has_hooks))
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

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (false, "settings.json: file not found".to_string());
        }
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

#[cfg(test)]
mod tests {
    use super::settings_has_amplihack_hooks;

    /// #1123: after collapsing the `exists()` probe into the read, an absent
    /// path must still map to `None` (nothing to report for this location).
    #[test]
    fn settings_has_amplihack_hooks_absent_path_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist/settings.json");
        assert!(settings_has_amplihack_hooks(&missing).is_none());
    }

    /// #1123: a present-but-unreadable path (a directory at the file path) must
    /// map to `Some(Err(_))`, preserving the absent-vs-error distinction.
    #[test]
    fn settings_has_amplihack_hooks_unreadable_present_is_some_err() {
        let dir = tempfile::tempdir().unwrap();
        // The path exists but is a directory, so read_to_string fails with a
        // non-NotFound error.
        let result = settings_has_amplihack_hooks(dir.path());
        assert!(
            matches!(result, Some(Err(_))),
            "present-but-unreadable path must yield Some(Err(_)), got {result:?}"
        );
    }
}
