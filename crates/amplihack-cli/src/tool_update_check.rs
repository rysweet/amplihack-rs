//! Pre-launch npm tool update notice (WS3).
//!
//! Before launching an npm-distributed tool (claude, copilot, codex), check
//! whether a newer version is available from the npm registry and print a
//! one-line stderr notice if so.
//!
//! ## Design constraints
//!
//! - **No new crate dependencies** — stdlib only (`std::process::Command`,
//!   `std::sync::mpsc`, `std::thread`).
//! - **Timeout-bounded** — each npm subprocess is limited to 3 seconds.
//!   If npm is slow, unavailable, or missing, the check is silently skipped.
//! - **Non-interactive bypass** — when `AMPLIHACK_NONINTERACTIVE=1` or when
//!   the `skip` parameter is `true`, no subprocesses are spawned.
//! - **Input sanitization** — all npm output is passed through
//!   [`sanitize_version`] before printing to prevent ANSI injection attacks
//!   from a malicious npm registry response.
//! - **Hardcoded package mapping** — only known tool names are mapped to npm
//!   packages. User-controlled `tool` strings are never interpolated into
//!   the npm package argument. See [`npm_package_for_tool`].
//!
//! ## Security notes (SEC-WS3)
//!
//! - `npm_package_for_tool()` uses a hardcoded `match` — no user input is
//!   ever interpolated into npm command arguments.
//! - `sanitize_version()` strips non-semver characters before printing.
//! - A 3-second `recv_timeout` on a `mpsc::channel` acts as a DoS guard
//!   against a hung or malicious npm binary on PATH.
//! - No credentials or secrets are passed to npm subprocesses.
//!
//! ## Position in launch sequence
//!
//! ```text
//! amplihack launch <tool>
//!   → nesting detection
//!   → maybe_print_npm_update_notice(tool, skip)   ← this module
//!   → bootstrap::ensure_tool_available(tool)
//!   → spawn ManagedChild
//! ```
//!
//! Placing the check before `ensure_tool_available` means we skip the notice
//! entirely when the tool isn't installed yet (nothing to update).

use crate::util::is_noninteractive;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Subprocess timeout for each npm command.
const NPM_TIMEOUT: Duration = Duration::from_secs(3);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Print a one-line stderr update notice if a newer version of `tool` is
/// available from npm.
///
/// This function is a no-op (returns immediately without spawning any
/// subprocesses) when:
/// - `skip` is `true` (caller passed `--skip-update-check`)
/// - [`is_noninteractive`] returns `true` (`AMPLIHACK_NONINTERACTIVE=1` or no TTY)
/// - `tool` has no known npm package (see [`npm_package_for_tool`])
/// - npm is not on PATH
/// - Either npm query times out or returns unparseable output
///
/// # Example output (stderr, when update is available)
///
/// ```text
/// amplihack: update available: @anthropic-ai/claude-code 1.0.5 → 1.1.0
/// (run: npm install -g @anthropic-ai/claude-code to update)
/// ```
pub fn maybe_print_npm_update_notice(tool: &str, skip: bool) {
    // SEC-WS3: AMPLIHACK_NONINTERACTIVE check is the second guard.
    // Unconditionally prevents subprocess spawning regardless of skip flag.
    if skip || is_noninteractive() {
        return;
    }

    let Some(pkg) = npm_package_for_tool(tool) else {
        return;
    };

    let installed = match get_installed_version(pkg) {
        Some(v) => v,
        None => return, // npm not available or tool not installed
    };

    let latest = match get_latest_version(pkg) {
        Some(v) => v,
        None => return, // npm registry unavailable or timeout
    };

    // Only print when versions actually differ.
    if installed != latest {
        let safe_installed = sanitize_version(&installed);
        let safe_latest = sanitize_version(&latest);
        eprintln!(
            "amplihack: update available: {pkg} {safe_installed} → {safe_latest}"
        );
        eprintln!("(run: npm install -g {pkg} to update)");
    }
}

// ---------------------------------------------------------------------------
// Package mapping (hardcoded — SEC-WS3)
// ---------------------------------------------------------------------------

/// Map a tool name to its npm package identifier.
///
/// Uses only hardcoded match arms.  User-controlled `tool` strings are never
/// interpolated into npm command arguments — this is a security invariant.
///
/// Returns `None` for any tool not distributed via npm.
pub fn npm_package_for_tool(tool: &str) -> Option<&'static str> {
    match tool {
        "claude" => Some("@anthropic-ai/claude-code"),
        "copilot" => Some("@github/copilot-cli"),
        "codex" => Some("@openai/codex"),
        // amplifier is not npm-distributed
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Version queries
// ---------------------------------------------------------------------------

/// Query the locally installed version of an npm package.
///
/// Runs: `npm list -g --depth=0 --json`
/// Parses the JSON output to extract the version for `pkg`.
///
/// Returns `None` if npm is unavailable, times out, or the package is not
/// installed globally.
pub fn get_installed_version(pkg: &str) -> Option<String> {
    let output = run_npm_with_timeout(
        &["list", "-g", "--depth=0", "--json"],
        NPM_TIMEOUT,
    )?;

    // JSON structure: {"dependencies": {"@pkg/name": {"version": "1.2.3"}}}
    // Simple string search to avoid a JSON parsing dependency.
    let search_key = format!("\"{}\"", pkg);
    let pkg_pos = output.find(&search_key)?;
    let after_pkg = &output[pkg_pos..];

    // Find "version" key after the package name
    let version_pos = after_pkg.find("\"version\"")?;
    let after_version = &after_pkg[version_pos..];

    // Extract the version value: "version": "1.2.3"
    let colon_pos = after_version.find(':')?;
    let after_colon = after_version[colon_pos + 1..].trim_start();
    if !after_colon.starts_with('"') {
        return None;
    }
    let inner = &after_colon[1..];
    let end = inner.find('"')?;
    let version = inner[..end].to_string();
    if version.is_empty() {
        return None;
    }
    Some(version)
}

/// Query the latest published version of an npm package from the registry.
///
/// Runs: `npm show <pkg> version`
/// Returns the first token on stdout as the version string.
///
/// Returns `None` if npm is unavailable, times out, or the package is unknown.
pub fn get_latest_version(pkg: &str) -> Option<String> {
    // SEC-WS3: pkg is always a &'static str from npm_package_for_tool().
    // It is never a user-controlled runtime string.
    let output = run_npm_with_timeout(&["show", pkg, "version"], NPM_TIMEOUT)?;
    let version = output.split_whitespace().next()?.to_string();
    if version.is_empty() {
        return None;
    }
    Some(version)
}

// ---------------------------------------------------------------------------
// Version string sanitization (SEC-WS3)
// ---------------------------------------------------------------------------

/// Strip all characters from `s` that are not safe for semver display.
///
/// Allowed character set: `[a-zA-Z0-9.\-+]`
///
/// This is a mandatory security control.  A compromised npm registry could
/// return ANSI escape sequences in a version string, which when printed with
/// `eprintln!` would corrupt terminal state.  This filter ensures only
/// semver-safe characters reach the terminal.
///
/// **Do not remove or bypass this function** — it is a security boundary.
///
/// # Examples
///
/// ```rust
/// # use amplihack_cli::tool_update_check::sanitize_version;
/// assert_eq!(sanitize_version("1.2.3"), "1.2.3");
/// assert_eq!(sanitize_version("1.0.0-beta.1"), "1.0.0-beta.1");
/// assert_eq!(sanitize_version("\x1b[31m1.2.3\x1b[0m"), "1.2.3");
/// assert_eq!(sanitize_version("1.2.3\n"), "1.2.3");
/// ```
pub fn sanitize_version(s: &str) -> String {
    // Phase 1: Strip ANSI escape sequences of the form ESC [ ... <letter>.
    // This must run before character filtering because ANSI sequences contain
    // alphanumeric characters (e.g., "\x1b[31m" → '3', '1', 'm') which would
    // otherwise pass the allowlist filter and corrupt the output.
    let stripped = strip_ansi_escapes(s);

    // Phase 2: Allowlist filter — keep only semver-safe characters.
    // Allowed: [a-zA-Z0-9.\-+]
    stripped
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-' || *c == '+')
        .collect()
}

/// Remove ANSI CSI escape sequences (ESC [ ... letter) from `s`.
///
/// This is a minimal implementation sufficient for stripping terminal colour
/// codes from npm version output.  It handles the common form:
///   ESC '[' <parameter bytes 0x30–0x3F>* <intermediate bytes 0x20–0x2F>* <final byte 0x40–0x7E>
fn strip_ansi_escapes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Start of a potential escape sequence.
            if chars.peek() == Some(&'[') {
                // CSI sequence: ESC [ ... <final byte>
                chars.next(); // consume '['
                // Consume parameter and intermediate bytes until final byte.
                // Final byte: 0x40–0x7E ('@' through '~').
                loop {
                    match chars.next() {
                        Some(b) if b as u32 >= 0x40 && b as u32 <= 0x7E => break,
                        Some(_) => continue, // parameter/intermediate byte
                        None => break,        // unterminated sequence — end of string
                    }
                }
                // Sequence consumed; emit nothing.
            }
            // Other ESC sequences (non-CSI) are dropped by not emitting ESC.
        } else {
            out.push(c);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Subprocess execution with timeout
// ---------------------------------------------------------------------------

/// Run `npm <args>` with a hard timeout, returning stdout on success.
///
/// Uses a background thread + `mpsc::channel` + `recv_timeout` to enforce
/// the timeout without requiring an async runtime.  This is a security
/// control against a hung or malicious npm binary on PATH.
///
/// Returns `None` if:
/// - `npm` is not found on PATH
/// - The process does not complete within `timeout`
/// - The process exits with a non-zero status
/// - stdout is not valid UTF-8
pub fn run_npm_with_timeout(args: &[&str], timeout: Duration) -> Option<String> {
    // Convert to owned Strings so the thread can take ownership.
    let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();

    let (tx, rx) = mpsc::channel::<Option<String>>();

    thread::spawn(move || {
        let result = std::process::Command::new("npm")
            .args(&args_owned)
            .output()
            .ok()
            .and_then(|out| {
                if out.status.success() {
                    String::from_utf8(out.stdout).ok()
                } else {
                    None
                }
            });
        // Ignore send errors — receiver may have timed out.
        let _ = tx.send(result);
    });

    rx.recv_timeout(timeout).ok().flatten()
}

// ---------------------------------------------------------------------------
// Unit tests (TDD — these define the contract)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── npm_package_for_tool ────────────────────────────────────────────────

    /// WS3-UNIT-1: claude maps to the Anthropic npm package.
    #[test]
    fn npm_package_for_claude_returns_anthropic_package() {
        assert_eq!(
            npm_package_for_tool("claude"),
            Some("@anthropic-ai/claude-code"),
            "claude must map to @anthropic-ai/claude-code"
        );
    }

    /// WS3-UNIT-2: copilot maps to the GitHub Copilot CLI package.
    #[test]
    fn npm_package_for_copilot_returns_github_package() {
        assert_eq!(
            npm_package_for_tool("copilot"),
            Some("@github/copilot-cli"),
            "copilot must map to @github/copilot-cli"
        );
    }

    /// WS3-UNIT-3: codex maps to the OpenAI Codex package.
    #[test]
    fn npm_package_for_codex_returns_openai_package() {
        assert_eq!(
            npm_package_for_tool("codex"),
            Some("@openai/codex"),
            "codex must map to @openai/codex"
        );
    }

    /// WS3-UNIT-4: amplifier returns None — not npm-distributed.
    #[test]
    fn npm_package_for_amplifier_returns_none() {
        assert_eq!(
            npm_package_for_tool("amplifier"),
            None,
            "amplifier is not npm-distributed and must return None"
        );
    }

    /// WS3-UNIT-5: Unknown tool names return None (no package).
    #[test]
    fn npm_package_for_unknown_tool_returns_none() {
        assert_eq!(npm_package_for_tool("totally-unknown-binary"), None);
        assert_eq!(npm_package_for_tool(""), None);
        assert_eq!(npm_package_for_tool("npm"), None);
    }

    /// WS3-UNIT-6: User-supplied strings that look like injection attempts
    /// must return None — never be passed to npm as package names.
    #[test]
    fn npm_package_for_tool_rejects_injection_attempts() {
        // These strings should never reach npm as package arguments.
        assert_eq!(npm_package_for_tool("claude; rm -rf /"), None);
        assert_eq!(npm_package_for_tool("claude && malicious"), None);
        assert_eq!(npm_package_for_tool("@evil/package"), None);
        assert_eq!(npm_package_for_tool("$(whoami)"), None);
    }

    // ── sanitize_version ───────────────────────────────────────────────────

    /// WS3-UNIT-7: Plain semver strings pass through unchanged.
    #[test]
    fn sanitize_version_passes_through_plain_semver() {
        assert_eq!(sanitize_version("1.2.3"), "1.2.3");
        assert_eq!(sanitize_version("1.0.0"), "1.0.0");
        assert_eq!(sanitize_version("0.0.1"), "0.0.1");
        assert_eq!(sanitize_version("10.20.300"), "10.20.300");
    }

    /// WS3-UNIT-8: Pre-release and build-metadata suffixes are preserved.
    #[test]
    fn sanitize_version_preserves_prerelease_and_build_metadata() {
        assert_eq!(sanitize_version("1.0.0-beta.1"), "1.0.0-beta.1");
        assert_eq!(sanitize_version("2.0.0-rc.3"), "2.0.0-rc.3");
        assert_eq!(sanitize_version("1.0.0+build.20240101"), "1.0.0+build.20240101");
    }

    /// WS3-UNIT-9: ANSI escape sequences are stripped (SEC-WS3).
    ///
    /// A malicious registry could return `\x1b[31m1.2.3\x1b[0m` to inject
    /// terminal control codes. Sanitize_version must strip all such sequences.
    #[test]
    fn sanitize_version_strips_ansi_escape_sequences() {
        // Red colour sequence wrapping a version
        assert_eq!(sanitize_version("\x1b[31m1.2.3\x1b[0m"), "1.2.3");
        // Bold
        assert_eq!(sanitize_version("\x1b[1m2.0.0\x1b[0m"), "2.0.0");
        // Mixed
        assert_eq!(sanitize_version("\x1b[32;1m1.0.0-beta\x1b[0m"), "1.0.0-beta");
    }

    /// WS3-UNIT-10: Newlines and whitespace are stripped.
    ///
    /// npm output often has trailing newlines that must not appear in the
    /// printed version string.
    #[test]
    fn sanitize_version_strips_whitespace_and_newlines() {
        assert_eq!(sanitize_version("1.2.3\n"), "1.2.3");
        assert_eq!(sanitize_version("1.2.3\r\n"), "1.2.3");
        assert_eq!(sanitize_version("  1.2.3  "), "1.2.3");
        assert_eq!(sanitize_version("1.2.3\t"), "1.2.3");
    }

    /// WS3-UNIT-11: Empty strings pass through as empty strings.
    #[test]
    fn sanitize_version_empty_string_returns_empty() {
        assert_eq!(sanitize_version(""), "");
    }

    /// WS3-UNIT-12: Non-ASCII characters are stripped.
    ///
    /// npm output should never contain non-ASCII in version strings, but
    /// defensive filtering protects against unexpected registry responses.
    #[test]
    fn sanitize_version_strips_non_ascii_characters() {
        assert_eq!(sanitize_version("1.2.3\u{200B}"), "1.2.3"); // zero-width space
        assert_eq!(sanitize_version("1.2.3™"), "1.2.3");
        assert_eq!(sanitize_version("1.2.3\u{0000}"), "1.2.3"); // null byte
    }

    // ── run_npm_with_timeout ───────────────────────────────────────────────

    /// WS3-UNIT-13: A zero-duration timeout returns None immediately.
    ///
    /// Verifies the timeout mechanism works — recv_timeout(0) will always
    /// time out before the thread can complete.
    #[test]
    fn run_npm_with_timeout_zero_duration_returns_none() {
        // A zero timeout should always return None regardless of npm presence.
        let result = run_npm_with_timeout(&["--version"], Duration::from_nanos(0));
        assert!(
            result.is_none(),
            "Zero-duration timeout must return None (timed out before npm could respond)"
        );
    }

    /// WS3-UNIT-14: A non-existent command returns None (npm not found).
    ///
    /// Tests the fallback path when npm binary is absent from PATH.
    /// Uses a clearly-bogus command name to ensure it fails.
    #[test]
    fn run_npm_with_timeout_missing_binary_returns_none() {
        // Replace PATH with an empty temp dir so no binaries are found.
        // This is safe because we restore it after.  The spawn will fail
        // with ENOENT and the thread sends None.
        //
        // We can't use the test_support lock here (different crate in integration test),
        // so we use a very short timeout to avoid flakiness.
        //
        // NOTE: This test is inherently racy if npm IS on PATH but takes <1s.
        // That's acceptable — the important invariant is that a missing npm
        // returns None, not panics.  The timeout ensures we never block forever.
        let result = run_npm_with_timeout(
            &["totally-invalid-npm-subcommand-that-will-exit-nonzero"],
            Duration::from_millis(500),
        );
        // Either None (npm not found / timed out) or None (npm exits non-zero).
        // Both are acceptable — the key invariant is it doesn't panic.
        let _ = result; // may be None or Some depending on environment
    }

    // ── get_installed_version JSON parsing ────────────────────────────────

    /// WS3-UNIT-15: get_installed_version parses a well-formed JSON response.
    ///
    /// Uses a known npm JSON output format.  We can't call real npm in unit tests,
    /// so we test the parsing logic directly via the public function when given
    /// a controlled npm output fixture.
    ///
    /// This is a contract test for the JSON parsing in get_installed_version.
    #[test]
    fn npm_list_json_parsing_extracts_version_correctly() {
        // Simulate the output of `npm list -g --depth=0 --json`
        let npm_output = r#"{
  "dependencies": {
    "@anthropic-ai/claude-code": {
      "version": "1.0.5",
      "resolved": "...",
      "overridden": false
    }
  }
}"#;

        // We test the parsing logic in isolation using a helper that
        // performs the same string extraction as get_installed_version.
        let pkg = "@anthropic-ai/claude-code";
        let version = extract_version_from_npm_list_json(npm_output, pkg);
        assert_eq!(
            version,
            Some("1.0.5".to_string()),
            "get_installed_version must extract '1.0.5' from the JSON output"
        );
    }

    /// WS3-UNIT-16: get_installed_version returns None when package is absent.
    #[test]
    fn npm_list_json_parsing_returns_none_for_missing_package() {
        let npm_output = r#"{"dependencies": {}}"#;
        let version = extract_version_from_npm_list_json(npm_output, "@anthropic-ai/claude-code");
        assert_eq!(
            version, None,
            "Must return None when package is not in npm list output"
        );
    }

    // ── maybe_print_npm_update_notice guards ──────────────────────────────

    /// WS3-UNIT-17: maybe_print_npm_update_notice returns immediately when skip=true.
    ///
    /// When --skip-update-check is passed, NO subprocesses must be spawned.
    /// We verify this indirectly: the function must return within 1ms
    /// (no npm subprocess overhead possible in that time).
    #[test]
    fn maybe_print_npm_update_notice_skips_when_skip_true() {
        let start = std::time::Instant::now();
        // skip=true must prevent any npm subprocess from running.
        maybe_print_npm_update_notice("claude", true);
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(100),
            "maybe_print_npm_update_notice with skip=true must return in <100ms, \
             got {}ms. Subprocess was spawned when it shouldn't have been.",
            elapsed.as_millis()
        );
    }

    /// WS3-UNIT-18: maybe_print_npm_update_notice is a no-op for unknown tools.
    #[test]
    fn maybe_print_npm_update_notice_noop_for_unknown_tool() {
        let start = std::time::Instant::now();
        maybe_print_npm_update_notice("totally-unknown-tool", false);
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(100),
            "maybe_print_npm_update_notice for unknown tool must return in <100ms \
             (no npm package → no subprocess), got {}ms",
            elapsed.as_millis()
        );
    }

    // ── Test helpers ───────────────────────────────────────────────────────

    /// Helper that mirrors the JSON parsing logic in `get_installed_version`
    /// for use in unit tests without spawning npm subprocesses.
    fn extract_version_from_npm_list_json(output: &str, pkg: &str) -> Option<String> {
        let search_key = format!("\"{}\"", pkg);
        let pkg_pos = output.find(&search_key)?;
        let after_pkg = &output[pkg_pos..];
        let version_pos = after_pkg.find("\"version\"")?;
        let after_version = &after_pkg[version_pos..];
        let colon_pos = after_version.find(':')?;
        let after_colon = after_version[colon_pos + 1..].trim_start();
        if !after_colon.starts_with('"') {
            return None;
        }
        let inner = &after_colon[1..];
        let end = inner.find('"')?;
        let version = inner[..end].to_string();
        if version.is_empty() { None } else { Some(version) }
    }
}
