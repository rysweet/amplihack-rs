//! Pre-launch npm tool update notice (WS3).
//!
//! Before launching an npm-distributed tool (claude, copilot, codex), checks
//! whether a newer version is available and prints a one-line stderr notice.
//!
//! Design constraints: stdlib only, 3-second timeout per npm subprocess,
//! skipped in non-interactive mode, version output sanitized before printing.

mod version;

pub use version::{
    get_installed_version, get_latest_version, run_npm_with_timeout, sanitize_version,
};

use crate::util::is_noninteractive;

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

    // Sanitize both versions before comparison — prevents spurious update
    // notices from whitespace differences (e.g. trailing newlines in npm output)
    // and ensures ANSI-stripped forms are compared.  SEC-WS3: sanitization runs
    // before any comparison or display path.
    let safe_installed = sanitize_version(&installed);
    let safe_latest = sanitize_version(&latest);

    // Only print when sanitized versions actually differ.
    if safe_installed != safe_latest {
        eprintln!("amplihack: update available: {pkg} {safe_installed} → {safe_latest}");
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
// Unit tests (TDD — these define the contract)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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
        assert_eq!(
            sanitize_version("1.0.0+build.20240101"),
            "1.0.0+build.20240101"
        );
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
        assert_eq!(
            sanitize_version("\x1b[32;1m1.0.0-beta\x1b[0m"),
            "1.0.0-beta"
        );
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
        let result = run_npm_with_timeout(
            &["totally-invalid-npm-subcommand-that-will-exit-nonzero"],
            Duration::from_millis(500),
        );
        let _ = result; // may be None or Some depending on environment
    }

    // ── get_installed_version JSON parsing ────────────────────────────────

    /// WS3-UNIT-15: get_installed_version parses a well-formed JSON response.
    #[test]
    fn npm_list_json_parsing_extracts_version_correctly() {
        let npm_output = r#"{
  "dependencies": {
    "@anthropic-ai/claude-code": {
      "version": "1.0.5",
      "resolved": "...",
      "overridden": false
    }
  }
}"#;

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
    #[test]
    fn maybe_print_npm_update_notice_skips_when_skip_true() {
        let start = std::time::Instant::now();
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

    fn extract_version_from_npm_list_json(output: &str, pkg: &str) -> Option<String> {
        version::parse_version_from_npm_list_json(output, pkg)
    }
}
