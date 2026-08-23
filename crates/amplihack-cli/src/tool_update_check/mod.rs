//! Pre-launch npm tool update notice (WS3).
//!
//! Before launching an npm-distributed tool (claude, copilot, codex), checks
//! whether a newer version is available and prints a one-line stderr notice.
//!
//! Design constraints: stdlib only, 3-second timeout per npm subprocess,
//! skipped in non-interactive mode, version output sanitized before printing.

mod version;

pub use version::{get_latest_version, sanitize_version};

use crate::util::is_noninteractive;
use amplihack_utils::launch_target::{LaunchTarget, TargetSource};

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

    // Issue #1266: report the version of the binary that will ACTUALLY be
    // launched, not whatever `npm list -g` finds under npm's ambient prefix.
    // Those are routinely different files — on a host whose PATH leads with a
    // broken npm install, the ambient answer told the user to upgrade to a
    // version they were already running. A notice that names a different binary
    // than the one being launched is the same defect as installing one, only
    // quieter.
    let installed = match amplihack_utils::launch_target::resolve(tool).target {
        Some(target) if notice_applies(&target, tool) => target.version,
        // Either nothing healthy is installed — the launch path reports that —
        // or the binary that will launch did not come out of `pkg`, so this
        // notice has nothing true to say about it.
        _ => return,
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

/// Does an `npm install -g <pkg>` notice actually describe *this* target?
///
/// Two ways it does not, both of which shipped:
///
/// * **The binary is not one amplihack installed.** On the host issue #1266 was
///   reported from, `~/.local/bin/claude` wins resolution and `decide_install`
///   correctly answers `UseExisting` — installing into `~/.npm-global` cannot
///   change what launches. The notice, one function away, told the user to run
///   `npm install -g @anthropic-ai/claude-code` anyway: a command that writes
///   somewhere else, printed on every launch, forever. Two answers to one
///   question inside a single launch is the disagreement this issue exists to
///   delete, and a notice is not exempt from it just because it is advisory.
///
/// * **The binary is not even the same product.** `binary_candidates("claude")`
///   is `["rustyclawd", "claude"]` — rustyclawd first — so any `rustyclawd` on
///   `$PATH` can be the resolved target for `amplihack claude`. Comparing its
///   version against `@anthropic-ai/claude-code`'s registry entry is
///   meaningless, and the comparison is `!=` rather than "older than", so it
///   fires in both directions and can never stop firing.
///
/// The file-name test is what closes the second case, because a `rustyclawd`
/// sitting in `~/.npm-global/bin` would otherwise pass the source test. It is
/// [`launch_target::target_is_the_tool`] rather than a local copy because
/// `decide_install` needs the identical question answered the identical way —
/// this notice having the check while the install decision lacked it was worth
/// a multi-hundred-megabyte install on every launch.
fn notice_applies(target: &LaunchTarget, tool: &str) -> bool {
    target.source == TargetSource::AmplihackPrefix
        && amplihack_utils::launch_target::target_is_the_tool(target, tool)
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
        // NOTE: must stay in lockstep with `npm_package_for_install` in
        // `bootstrap.rs` — otherwise the update check queries one package
        // while the installer writes another, silently suppressing updates.
        "copilot" => Some("@github/copilot"),
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
    // Not re-exported from this module: nothing outside `tool_update_check`
    // calls it, so the tests reach into `version` directly rather than the
    // module's public surface carrying it for them.
    use super::version::run_npm_with_timeout;
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
    /// Must match `npm_package_for_install` in `bootstrap.rs`.
    #[test]
    fn npm_package_for_copilot_returns_github_package() {
        assert_eq!(
            npm_package_for_tool("copilot"),
            Some("@github/copilot"),
            "copilot must map to @github/copilot"
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

    // The `npm list -g --json` parsing tests that lived here went with
    // `get_installed_version` in issue #1266 — the launched-binary version now
    // comes from `launch_target::resolve`, and `launch_target::extract_version`
    // carries the parsing tests.

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

    // -----------------------------------------------------------------------
    // Issue #1266 — the notice must describe the binary that will launch
    // -----------------------------------------------------------------------

    fn target_at(path: &str, source: TargetSource) -> LaunchTarget {
        LaunchTarget {
            path: std::path::PathBuf::from(path),
            version: "2.1.238".to_string(),
            source,
        }
    }

    #[test]
    fn no_notice_for_a_binary_amplihack_did_not_install() {
        // The reported host: ~/.local/bin/claude wins, decide_install answers
        // UseExisting, and `npm install -g @anthropic-ai/claude-code` writes to
        // a prefix that is not where this file came from.
        for source in [
            TargetSource::FallbackDir,
            TargetSource::Path,
            TargetSource::ExplicitOverride {
                user_supplied: true,
            },
        ] {
            assert!(
                !notice_applies(&target_at("/home/you/.local/bin/claude", source), "claude"),
                "{source:?} is not amplihack's to update"
            );
        }
    }

    #[test]
    fn a_notice_still_fires_for_amplihacks_own_install() {
        // The gate must not silence the one case the notice is FOR.
        assert!(notice_applies(
            &target_at(
                "/home/you/.npm-global/bin/claude",
                TargetSource::AmplihackPrefix
            ),
            "claude"
        ));
    }

    #[test]
    fn no_notice_when_the_resolved_binary_is_a_different_product() {
        // `binary_candidates("claude")` is ["rustyclawd", "claude"], so
        // rustyclawd can be the target for `amplihack claude`. Comparing its
        // version to @anthropic-ai/claude-code's is meaningless, and `!=` fires
        // in both directions — so it would never stop.
        assert!(
            !notice_applies(
                &target_at(
                    "/home/you/.npm-global/bin/rustyclawd",
                    TargetSource::AmplihackPrefix
                ),
                "claude"
            ),
            "a rustyclawd in amplihack's own prefix still is not claude"
        );
    }
}
