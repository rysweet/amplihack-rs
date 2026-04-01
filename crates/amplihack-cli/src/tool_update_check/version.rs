//! Version querying, sanitization, and npm subprocess execution.

use crate::util::strip_ansi;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Subprocess timeout for each npm command.
const NPM_TIMEOUT: Duration = Duration::from_secs(3);

/// Query the locally installed version of an npm package.
///
/// Runs: `npm list -g --depth=0 --json`
/// Parses the JSON output to extract the version for `pkg`.
///
/// Returns `None` if npm is unavailable, times out, or the package is not
/// installed globally.
pub fn get_installed_version(pkg: &str) -> Option<String> {
    let output = run_npm_with_timeout(&["list", "-g", "--depth=0", "--json"], NPM_TIMEOUT)?;
    parse_version_from_npm_list_json(&output, pkg)
}

/// Extract the version string for `pkg` from `npm list -g --depth=0 --json` output.
///
/// JSON structure: `{"dependencies": {"@pkg/name": {"version": "1.2.3"}}}`
/// Uses simple string search to avoid a JSON parsing dependency.
pub(super) fn parse_version_from_npm_list_json(output: &str, pkg: &str) -> Option<String> {
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
    if version.is_empty() {
        None
    } else {
        Some(version)
    }
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

/// Strip all characters from `s` that are not safe for semver display.
///
/// Strips ANSI escape sequences, then applies an allowlist of `[a-zA-Z0-9.\-+]`.
/// Prevents terminal injection from a malicious npm registry response.
///
/// ```rust
/// # use amplihack_cli::tool_update_check::sanitize_version;
/// assert_eq!(sanitize_version("1.2.3"), "1.2.3");
/// assert_eq!(sanitize_version("\x1b[31m1.2.3\x1b[0m"), "1.2.3");
/// assert_eq!(sanitize_version("1.2.3\n"), "1.2.3");
/// ```
pub fn sanitize_version(s: &str) -> String {
    // Strip ANSI escape sequences first (ANSI codes contain alphanumeric chars
    // that would otherwise survive the allowlist filter and corrupt output).
    let stripped = strip_ansi(s);

    // Allowlist filter — keep only semver-safe characters: [a-zA-Z0-9.\-+]
    // Pre-allocate to the stripped length; result is always ≤ input length.
    let mut result = String::with_capacity(stripped.len());
    result.extend(
        stripped
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-' || *c == '+'),
    );
    result
}

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
