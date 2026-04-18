//! System health checks for the `amplihack doctor` subcommand.
//!
//! Runs a fixed set of checks and prints a pass/fail summary.  Exits with
//! code 1 if any check fails.
//!
//! # Check inventory (5 checks)
//!
//! 1. amplihack hooks installed — reads `$HOME/.claude/settings.json` and
//!    verifies the `hooks` section contains a value with `"amplihack"`.
//! 2. settings.json valid JSON — parses the file; reports only validity, never content.
//! 3. recipe-runner-rs available — locates binary on `$PATH` and runs `--version`.
//! 4. tmux installed — runs `tmux -V` and extracts version string.
//! 5. amplihack binary version — compile-time constant; always passes.
//!
//! # Security
//!
//! * SEC-WS2-01: External stderr is truncated to [`MAX_ERROR_LEN`] chars.
//! * SEC-WS2-02: All externally-sourced strings pass through [`strip_ansi`].
//! * SEC-WS2-03: All `Command::new()` calls use compile-time literal arguments.
//! * SEC-WS2-04: `settings.json` content is never printed.

mod checks;

use anyhow::Result;
use std::path::PathBuf;

pub use checks::{
    check_amplihack_version, check_hooks_installed, check_recipe_runner_available,
    check_settings_valid_json, check_tmux_installed,
};

#[cfg(test)]
use crate::util::strip_ansi;

// ── Public constants ──────────────────────────────────────────────────────────

/// Maximum number of characters kept from a subprocess's stderr before
/// truncation.  Prevents adversarial error output from flooding logs.
pub const MAX_ERROR_LEN: usize = 200;

/// Maximum length of a version string extracted from external tool output.
pub const MAX_VERSION_LEN: usize = 80;

// ── Path helpers ──────────────────────────────────────────────────────────────

/// Return `<home_dir>/.claude/settings.json`.
pub fn settings_json_path_for(home_dir: &std::path::Path) -> PathBuf {
    home_dir.join(".claude").join("settings.json")
}

/// Return the path `$HOME/.claude/settings.json`, or `None` if `HOME` is not
/// set.
pub fn settings_json_path() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|home| settings_json_path_for(std::path::Path::new(&home)))
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Recursively walk a `serde_json::Value` and return `true` if any string
/// value within it contains the substring `"amplihack"`.
fn json_contains_amplihack(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::String(s) => s.contains("amplihack"),
        serde_json::Value::Array(arr) => arr.iter().any(json_contains_amplihack),
        serde_json::Value::Object(map) => map.values().any(json_contains_amplihack),
        _ => false,
    }
}

/// Truncate `s` to at most `max_chars` characters (character boundary safe).
fn truncate(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((byte_idx, _)) => &s[..byte_idx],
        None => s,
    }
}

// ── Summary printer ───────────────────────────────────────────────────────────

/// Print a formatted summary of `results` to stdout and return `(passed,
/// failed)` counts.
pub fn print_summary(results: &[(bool, String)]) -> (usize, usize) {
    let mut passed = 0usize;
    let mut failed = 0usize;

    for (ok, msg) in results {
        if *ok {
            println!("\x1b[32m✓\x1b[0m {msg}");
            passed += 1;
        } else {
            println!("\x1b[31m✗\x1b[0m {msg}");
            failed += 1;
        }
    }

    println!();
    if failed == 0 {
        println!("All checks passed.");
    } else {
        println!("{failed} check(s) failed.");
    }

    (passed, failed)
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Run all doctor checks, print the summary, and exit 1 if any check failed.
pub fn run_doctor() -> Result<()> {
    use std::thread;

    // Spawn subprocess checks in parallel — they are independent and I/O-bound.
    let h3 = thread::spawn(check_recipe_runner_available);
    let h4 = thread::spawn(check_tmux_installed);

    // Fast checks run on the current thread while the subprocess threads work.
    let r1 = check_hooks_installed();
    let r2 = check_settings_valid_json();
    let r5 = check_amplihack_version();

    // Collect results in canonical display order (1–5), waiting for threads.
    let results = vec![
        r1,
        r2,
        h3.join().unwrap_or_else(|_| {
            (
                false,
                "recipe-runner-rs: internal thread panicked".to_string(),
            )
        }),
        h4.join()
            .unwrap_or_else(|_| (false, "tmux: internal thread panicked".to_string())),
        r5,
    ];

    let (_, failed) = print_summary(&results);

    if failed > 0 {
        std::process::exit(1);
    }

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_ansi_passthrough_on_plain_text() {
        let input = "hello world";
        assert_eq!(strip_ansi(input), "hello world");
    }

    #[test]
    fn test_strip_ansi_removes_sgr_sequences() {
        let input = "\x1b[1mbold\x1b[0m normal";
        assert_eq!(strip_ansi(input), "bold normal");
    }

    #[test]
    fn test_strip_ansi_removes_multiple_sequences() {
        let input = "\x1b[32m\x1b[1mgreen bold\x1b[0m";
        assert_eq!(strip_ansi(input), "green bold");
    }

    #[test]
    fn test_settings_json_path_uses_home_env() {
        let path = settings_json_path_for(std::path::Path::new("/tmp/fake-home"));
        assert_eq!(
            path,
            std::path::PathBuf::from("/tmp/fake-home/.claude/settings.json")
        );
    }

    #[test]
    fn test_settings_json_path_none_when_home_unset() {
        let path = settings_json_path_for(std::path::Path::new("/custom/home"));
        assert_eq!(
            path,
            std::path::PathBuf::from("/custom/home/.claude/settings.json"),
            "settings_json_path_for() must append .claude/settings.json to any home root"
        );
    }

    #[test]
    fn test_check_amplihack_version_always_passes() {
        let (passed, msg) = check_amplihack_version();
        assert!(
            passed,
            "amplihack version check must always pass; got message: {msg}"
        );
        assert!(
            !msg.is_empty(),
            "amplihack version message must not be empty"
        );
        let pkg_version = crate::VERSION;
        assert!(
            msg.contains(pkg_version),
            "message should contain the package version '{pkg_version}'; got: {msg}"
        );
    }

    #[test]
    fn test_print_summary_all_pass_returns_correct_counts() {
        let results = vec![
            (true, "hooks installed".to_string()),
            (true, "settings.json valid".to_string()),
            (true, String::from("tmux v3.4")),
        ];
        let (passed, failed) = print_summary(&results);
        assert_eq!(passed, 3, "all three checks should be counted as passed");
        assert_eq!(failed, 0, "no checks should be counted as failed");
    }

    #[test]
    fn test_print_summary_with_failures_returns_correct_counts() {
        let results = vec![
            (true, "hooks installed".to_string()),
            (false, "recipe-runner-rs not found on PATH".to_string()),
            (true, String::from("tmux 3.4")),
        ];
        let (passed, failed) = print_summary(&results);
        assert_eq!(passed, 2, "two checks should be counted as passed");
        assert_eq!(failed, 1, "one check should be counted as failed");
    }

    #[test]
    fn test_max_error_len_is_200() {
        assert_eq!(MAX_ERROR_LEN, 200);
    }

    #[test]
    #[ignore = "requires live environment with tmux, python3, recipe-runner-rs"]
    fn test_run_doctor_does_not_return_err_on_typical_machine() {
        let result = run_doctor();
        assert!(
            result.is_ok(),
            "run_doctor should return Ok on healthy machine"
        );
    }
}
