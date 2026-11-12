//! Individual health check implementations for `amplihack doctor`.

use super::hook_drift::{Source, profile_error_reason, render, report};
use super::{MAX_ERROR_LEN, MAX_VERSION_LEN, settings_json_path, settings_json_path_for};
use crate::commands::install::hook_block::hook_target;
use crate::commands::launch::hook_settings::launch_file_path;
use crate::commands::profile::ProfileStore;
use crate::util::{run_output_with_timeout, strip_ansi, truncate_chars_with_notice};
use std::process::Command;
use std::time::Duration;

const DOCTOR_COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

/// Check 1 — amplihack hooks installed (issue #2279, D9).
///
/// One line per settings file that carries amplihack's hooks block:
/// `$HOME/.claude/settings.json`, each profile's `settings.json` under the
/// configured profile root, and the launch `--settings` file. Fails when the
/// home file is not clean, when a present profile or launch file has
/// drifted, cannot be parsed or is refused, or when the profiles or the
/// launch file cannot be located. An absent home file passes when
/// `<cwd>/.claude/settings.json` is clean, a repo-local install (#1088).
/// Read-only.
///
/// File contents are never printed; lines hold paths, event names and fixed
/// phrases only (SEC-WS2-04).
pub fn check_hooks_installed() -> (bool, String) {
    let Some(home) = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(std::path::PathBuf::from)
    else {
        return (false, "hooks: $HOME not set".to_string());
    };
    let Some(target) = hook_target() else {
        return (
            false,
            "hooks: no usable amplihack-hooks binary; run `amplihack install`".to_string(),
        );
    };
    let root = ProfileStore::from_env()
        .map(|store| store.root().to_path_buf())
        .map_err(|error| profile_error_reason(&error));
    let profiles = match &root {
        Ok(root) => Source::At(root),
        Err(why) => Source::Unknown(why),
    };
    let launch = launch_file_path();
    let launch_source = match &launch {
        Some(path) => Source::At(path),
        None => Source::Unknown("amplihack home unknown or not absolute"),
    };
    let repo_local = std::env::current_dir()
        .ok()
        .map(|cwd| settings_json_path_for(&cwd));
    let reports = report(
        &home,
        repo_local.as_deref(),
        profiles,
        launch_source,
        &target,
    );
    let (ok, lines) = render(&reports);
    let head = if ok {
        "amplihack hooks installed"
    } else {
        "amplihack hooks block drift; run `amplihack install` to repair"
    };
    let mut message = head.to_string();
    for line in lines.lines() {
        message.push_str("\n    ");
        message.push_str(line);
    }
    (ok, message)
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
                    "{}: {}",
                    probe_failure_headline("recipe-runner-rs", &e),
                    truncate_chars_with_notice(&msg, MAX_ERROR_LEN)
                ),
            )
        }
    }
}

/// The headline for a `--version` probe that never produced an exit status.
///
/// Issue #1424. This `Err` arm covers three unrelated events — the file is not
/// there, the probe timed out, and the host would not give amplihack a process
/// to run it in — and it used to print "not found on PATH" over all three.
/// `doctor` exists to tell the user what is wrong; a check that names absence
/// when the machine is merely out of processes sends them to reinstall
/// something that is sitting right there.
fn probe_failure_headline(tool: &str, error: &anyhow::Error) -> String {
    let absent = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<std::io::Error>())
        .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound);
    if absent {
        format!("{tool} not found on PATH")
    } else {
        format!("{tool} could not be run")
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
                    "{}: {}",
                    probe_failure_headline("tmux", &e),
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
    use super::*;

    // ------------------------------------------------------------------
    // Issue #1424: a probe that could not run is not proof of absence.
    // ------------------------------------------------------------------

    #[test]
    fn a_missing_file_is_reported_as_not_on_path() {
        let error = anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::NotFound))
            .context("spawning the probe");
        assert_eq!(
            probe_failure_headline("recipe-runner-rs", &error),
            "recipe-runner-rs not found on PATH"
        );
    }

    #[test]
    fn a_host_out_of_processes_is_not_reported_as_not_on_path() {
        // EAGAIN from fork: the file may be perfectly present. Saying it is
        // not on `$PATH` sends the reader after an install problem that does
        // not exist -- issue #1424.
        let error = anyhow::Error::new(std::io::Error::from_raw_os_error(libc::EAGAIN))
            .context("spawning the probe");
        let headline = probe_failure_headline("recipe-runner-rs", &error);
        assert!(
            !headline.contains("not found"),
            "must not claim absence: {headline}"
        );
        assert_eq!(headline, "recipe-runner-rs could not be run");
    }

    #[test]
    fn a_timeout_is_not_reported_as_not_on_path() {
        let error = anyhow::anyhow!("command timed out after 2s");
        assert!(!probe_failure_headline("tmux", &error).contains("not found"));
    }
}
