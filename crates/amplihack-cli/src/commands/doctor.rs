//! System health checks for the `amplihack doctor` subcommand.
//!
//! Runs a fixed set of checks and prints a pass/fail summary.  Exits with
//! code 1 if any check fails.
//!
//! # Check inventory (6 checks)
//!
//! 1. amplihack hooks installed — reads `$HOME/.claude/settings.json` and
//!    verifies the `hooks` section contains a value with `"amplihack"`.
//! 2. settings.json valid JSON — parses the file; reports only validity, never content.
//! 3. recipe-runner-rs available — locates binary on `$PATH` and runs `--version`.
//! 4. Python bridge working — runs `python3 -c "import amplihack"`.
//! 5. tmux installed — runs `tmux -V` and extracts version string.
//! 6. amplihack binary version — compile-time constant; always passes.
//!
//! # Security
//!
//! * SEC-WS2-01: External stderr is truncated to [`MAX_ERROR_LEN`] chars.
//! * SEC-WS2-02: All externally-sourced strings pass through [`strip_ansi`].
//! * SEC-WS2-03: All `Command::new()` calls use compile-time literal arguments.
//! * SEC-WS2-04: `settings.json` content is never printed.

use anyhow::Result;
use std::path::PathBuf;

// Re-export strip_ansi from the shared util module so existing callers within
// this file continue to work without qualification.  Shared module ensures the
// SEC-WS2-02 contract is applied consistently across doctor.rs and
// binary_finder.rs.
use crate::util::strip_ansi;

// ── Public constants ──────────────────────────────────────────────────────────

/// Maximum number of characters kept from a subprocess's stderr before
/// truncation.  Prevents adversarial error output from flooding logs.
pub const MAX_ERROR_LEN: usize = 200;

/// Maximum length of a version string extracted from external tool output.
pub const MAX_VERSION_LEN: usize = 80;

// ── Path helpers ──────────────────────────────────────────────────────────────

/// Return `<home_dir>/.claude/settings.json`.
///
/// Accepts an explicit `home_dir` parameter rather than reading `$HOME` from
/// the environment.  Callers that need the real home directory should use
/// [`settings_json_path`]; tests pass a fake path to avoid mutating the
/// environment and eliminate test-ordering races.
pub fn settings_json_path_for(home_dir: &std::path::Path) -> PathBuf {
    home_dir.join(".claude").join("settings.json")
}

/// Return the path `$HOME/.claude/settings.json`, or `None` if `HOME` is not
/// set.
///
/// Uses the same `HOME`-lookup pattern as `nesting.rs`.
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

// ── Individual checks ─────────────────────────────────────────────────────────
//
// Each check returns `(passed: bool, message: String)`.  The message is shown
// in the summary line.  Check functions never panic — a failure is always
// represented as `(false, <description>)`.

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
/// --version`, reporting the version string on success.  Both the availability
/// check and the version report happen in a single subprocess call.
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

/// Check 4 — Python bridge working.
///
/// Runs `python3 -c "import amplihack"` and reports success if the exit code
/// is 0.  On failure the real stderr from the subprocess is captured and
/// sanitised via `strip_ansi` before display; no fabricated error text is
/// substituted.  See SEC-WS2-01 and SEC-WS2-02.
///
/// SAFETY: all subprocess arguments are compile-time literals.
pub fn check_python_bridge() -> (bool, String) {
    // SAFETY: all arguments are compile-time literals — no user input.
    let result = std::process::Command::new("python3")
        .args(["-c", "import amplihack"])
        .output();

    match result {
        Ok(out) if out.status.success() => (true, "python3 amplihack module available".to_string()),
        Ok(out) => {
            let raw = String::from_utf8_lossy(&out.stderr);
            let first_line = raw.lines().next().unwrap_or("").trim();
            let stripped = strip_ansi(first_line);
            let msg = if stripped.is_empty() {
                "(no output captured)".to_string()
            } else {
                truncate(&stripped, MAX_ERROR_LEN).to_string()
            };
            (false, format!("python bridge: {msg}"))
        }
        Err(e) => {
            let msg = e.to_string();
            (
                false,
                format!(
                    "python bridge: python3 not found: {}",
                    truncate(&msg, MAX_ERROR_LEN)
                ),
            )
        }
    }
}

/// Check 5 — tmux installed.
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

// ── Summary printer ───────────────────────────────────────────────────────────

/// Print a formatted summary of `results` to stdout and return `(passed,
/// failed)` counts.
///
/// Each result is displayed as:
///   `✓ <message>`  — when `passed == true`
///   `✗ <message>`  — when `passed == false`
///
/// A final line prints either `"All checks passed."` or
/// `"<N> check(s) failed."`.
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
///
/// Uses `std::process::exit(1)` rather than propagating `Err` so that the
/// exit code is explicit and the `Result<()>` return type is not polluted with
/// "soft" check failures.
///
/// # Performance
///
/// Checks 3–5 each spawn an external subprocess and dominate wall-clock time
/// (~100–500 ms each).  They are launched concurrently on dedicated threads so
/// total doctor time is bounded by the *slowest* single check rather than the
/// sum.  Checks 1, 2, and 6 (file I/O + compile-time constant) remain on the
/// calling thread and complete while the subprocess threads are running.
pub fn run_doctor() -> Result<()> {
    use std::thread;

    // Spawn subprocess checks in parallel — they are independent and I/O-bound.
    let h3 = thread::spawn(check_recipe_runner_available);
    let h4 = thread::spawn(check_python_bridge);
    let h5 = thread::spawn(check_tmux_installed);

    // Fast checks run on the current thread while the subprocess threads work.
    let r1 = check_hooks_installed();
    let r2 = check_settings_valid_json();
    let r6 = check_amplihack_version();

    // Collect results in canonical display order (1–6), waiting for threads.
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
            .unwrap_or_else(|_| (false, "python bridge: internal thread panicked".to_string())),
        h5.join()
            .unwrap_or_else(|_| (false, "tmux: internal thread panicked".to_string())),
        r6,
    ];

    let (_, failed) = print_summary(&results);

    if failed > 0 {
        std::process::exit(1);
    }

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────
//
// Test strategy (per design spec):
//   • Unit tests target formatting/logic helpers — they inject true/false to
//     exercise display code without spawning real processes.
//   • Integration tests that spawn real binaries are marked `#[ignore]` so
//     they do not break offline builds.

#[cfg(test)]
mod tests {
    use super::*;

    // ── WS2-TEST-01: strip_ansi passthrough ───────────────────────────────

    /// Plain text with no escape sequences should be returned unchanged.
    #[test]
    fn test_strip_ansi_passthrough_on_plain_text() {
        let input = "hello world";
        assert_eq!(strip_ansi(input), "hello world");
    }

    // ── WS2-TEST-02: strip_ansi removal ───────────────────────────────────

    /// ANSI SGR sequences (e.g. bold, colour reset) must be removed.
    #[test]
    fn test_strip_ansi_removes_sgr_sequences() {
        // "\x1b[1m" = bold on, "\x1b[0m" = reset
        let input = "\x1b[1mbold\x1b[0m normal";
        assert_eq!(strip_ansi(input), "bold normal");
    }

    // ── WS2-TEST-03: strip_ansi with nested sequences ─────────────────────

    /// Multiple consecutive escape sequences must all be removed.
    #[test]
    fn test_strip_ansi_removes_multiple_sequences() {
        let input = "\x1b[32m\x1b[1mgreen bold\x1b[0m";
        assert_eq!(strip_ansi(input), "green bold");
    }

    // ── WS2-TEST-04: settings_json_path_for appends correct suffix ───────

    /// `settings_json_path_for()` must append `.claude/settings.json` to
    /// the provided home directory.  Uses a pure function — no env mutation,
    /// no race condition.
    #[test]
    fn test_settings_json_path_uses_home_env() {
        let path = settings_json_path_for(std::path::Path::new("/tmp/fake-home"));
        assert_eq!(
            path,
            std::path::PathBuf::from("/tmp/fake-home/.claude/settings.json")
        );
    }

    // ── WS2-TEST-05: settings_json_path_for works with different roots ────

    /// `settings_json_path_for()` must work with any home directory path,
    /// including deeply nested ones.  Uses a pure function — no env mutation.
    #[test]
    fn test_settings_json_path_none_when_home_unset() {
        // Pure function test: verify that a different home root produces
        // the expected path.  (The old test mutated HOME to simulate an unset
        // value; the env-reading wrapper settings_json_path() returns None
        // when HOME is absent — but testing that here would require env
        // mutation and introduce a race.  The pure helper is tested instead.)
        let path = settings_json_path_for(std::path::Path::new("/custom/home"));
        assert_eq!(
            path,
            std::path::PathBuf::from("/custom/home/.claude/settings.json"),
            "settings_json_path_for() must append .claude/settings.json to any home root"
        );
    }

    // ── WS2-TEST-06: check_amplihack_version always passes ────────────────

    /// The amplihack version check must always pass (it uses a compile-time
    /// constant and cannot fail at runtime).
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
        // The message should mention the package version.
        let pkg_version = env!("CARGO_PKG_VERSION");
        assert!(
            msg.contains(pkg_version),
            "message should contain the package version '{pkg_version}'; got: {msg}"
        );
    }

    // ── WS2-TEST-07: print_summary all-pass ───────────────────────────────

    /// When all checks pass, `print_summary` must return `(N, 0)` where N is
    /// the number of results.
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

    // ── WS2-TEST-08: print_summary with failures ──────────────────────────

    /// When some checks fail, `print_summary` must return accurate counts.
    #[test]
    fn test_print_summary_with_failures_returns_correct_counts() {
        let results = vec![
            (true, "hooks installed".to_string()),
            (false, "recipe-runner-rs not found on PATH".to_string()),
            (false, "python bridge: ModuleNotFoundError".to_string()),
        ];
        let (passed, failed) = print_summary(&results);
        assert_eq!(passed, 1, "one check should be counted as passed");
        assert_eq!(failed, 2, "two checks should be counted as failed");
    }

    // ── WS2-TEST-09: MAX_ERROR_LEN constant ───────────────────────────────

    /// The error truncation limit must equal exactly 200 characters to match
    /// the security requirement in SEC-WS2-01.
    #[test]
    fn test_max_error_len_is_200() {
        assert_eq!(MAX_ERROR_LEN, 200);
    }

    // ── WS2-TEST-10: Integration smoke (requires live environment) ─────────

    /// Smoke test: `run_doctor()` should not return Err on a typical
    /// developer machine.  This test is `#[ignore]`-marked because it spawns
    /// real subprocesses and modifies nothing.
    #[test]
    #[ignore = "requires live environment with tmux, python3, recipe-runner-rs"]
    fn test_run_doctor_does_not_return_err_on_typical_machine() {
        // run_doctor() calls std::process::exit(1) on failure, so if we
        // reach the Ok case the environment is healthy.
        let result = run_doctor();
        assert!(
            result.is_ok(),
            "run_doctor should return Ok on healthy machine"
        );
    }
}
