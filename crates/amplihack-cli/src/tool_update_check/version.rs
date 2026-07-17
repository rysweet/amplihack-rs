//! Version querying, sanitization, and npm subprocess execution.

use crate::util::{run_output_with_timeout, strip_ansi};
use std::process::Command;
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
    let search_key = format!("\"{pkg}\"");
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
/// Uses the shared child-killing timeout helper so a hung or malicious npm
/// binary on PATH cannot keep running after this function returns.
///
/// Returns `None` if:
/// - `npm` is not found on PATH
/// - The process does not complete within `timeout`
/// - The process exits with a non-zero status
/// - stdout is not valid UTF-8
pub fn run_npm_with_timeout(args: &[&str], timeout: Duration) -> Option<String> {
    let mut cmd = Command::new("npm");
    cmd.args(args);
    let output = run_output_with_timeout(cmd, timeout).ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn run_npm_with_timeout_terminates_child_after_timeout() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let sentinel = temp.path().join("npm-still-ran");
        let fake_npm = temp.path().join("npm");
        std::fs::write(
            &fake_npm,
            "#!/bin/sh\n\
             /bin/sleep 0.2\n\
             printf 'late' > \"$AMPLIHACK_NPM_SENTINEL\"\n\
             printf '1.2.3\\n'\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&fake_npm).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&fake_npm, perms).unwrap();

        let previous_path = std::env::var_os("PATH");
        let previous_sentinel = std::env::var_os("AMPLIHACK_NPM_SENTINEL");
        unsafe {
            std::env::set_var("PATH", temp.path());
            std::env::set_var("AMPLIHACK_NPM_SENTINEL", &sentinel);
        }

        let result = run_npm_with_timeout(&["--version"], Duration::from_millis(10));
        std::thread::sleep(Duration::from_millis(350));

        match previous_path {
            Some(value) => unsafe { std::env::set_var("PATH", value) },
            None => unsafe { std::env::remove_var("PATH") },
        }
        match previous_sentinel {
            Some(value) => unsafe { std::env::set_var("AMPLIHACK_NPM_SENTINEL", value) },
            None => unsafe { std::env::remove_var("AMPLIHACK_NPM_SENTINEL") },
        }

        assert!(result.is_none(), "timeout should return no npm output");
        assert!(
            !sentinel.exists(),
            "timed-out npm subprocess must be terminated, not left running in a background thread"
        );
    }
}
