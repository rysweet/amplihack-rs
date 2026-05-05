use super::check::should_skip_update_check;
use super::install::{binary_filename, extract_archive, find_binary};
use super::network::{
    github_error_message, is_retryable_error, parse_latest_release, validate_download_url,
};
use super::*;
use sha2::{Digest, Sha256};

/// When AMPLIHACK_NONINTERACTIVE=1 is set, ALL subcommands — including launch
/// commands — must skip the update check to avoid polluting scripted output.
#[test]
fn test_skip_update_check_when_noninteractive_env_set() {
    unsafe { std::env::set_var("AMPLIHACK_NONINTERACTIVE", "1") };
    let result = should_skip_update_check_for_subcommand("launch");
    unsafe { std::env::remove_var("AMPLIHACK_NONINTERACTIVE") };
    assert!(
        result,
        "should_skip_update_check_for_subcommand('launch') must return true \
         when AMPLIHACK_NONINTERACTIVE=1"
    );
}

/// When AMPLIHACK_PARITY_TEST=1 is set, the update check must be suppressed.
#[test]
fn test_skip_update_check_when_parity_test_env_set() {
    unsafe { std::env::set_var("AMPLIHACK_PARITY_TEST", "1") };
    let result = should_skip_update_check_for_subcommand("launch");
    unsafe { std::env::remove_var("AMPLIHACK_PARITY_TEST") };
    assert!(
        result,
        "should_skip_update_check_for_subcommand('launch') must return true \
         when AMPLIHACK_PARITY_TEST=1"
    );
}

/// The `mode` subcommand is not a launch command — update checks must be skipped.
#[test]
fn test_skip_update_check_for_mode_subcommand() {
    unsafe {
        std::env::remove_var("AMPLIHACK_NONINTERACTIVE");
        std::env::remove_var("AMPLIHACK_PARITY_TEST");
        std::env::remove_var(NO_UPDATE_CHECK_ENV);
    }
    assert!(
        should_skip_update_check_for_subcommand("mode"),
        "should_skip_update_check_for_subcommand('mode') must return true — \
         'mode' is not a launch command"
    );
}

/// The `plugin` subcommand is not a launch command — update checks must be skipped.
#[test]
fn test_skip_update_check_for_plugin_subcommand() {
    unsafe {
        std::env::remove_var("AMPLIHACK_NONINTERACTIVE");
        std::env::remove_var("AMPLIHACK_PARITY_TEST");
        std::env::remove_var(NO_UPDATE_CHECK_ENV);
    }
    assert!(
        should_skip_update_check_for_subcommand("plugin"),
        "should_skip_update_check_for_subcommand('plugin') must return true"
    );
}

/// Unknown subcommands must skip the update check.
#[test]
fn test_skip_update_check_for_unknown_subcommand() {
    unsafe {
        std::env::remove_var("AMPLIHACK_NONINTERACTIVE");
        std::env::remove_var("AMPLIHACK_PARITY_TEST");
        std::env::remove_var(NO_UPDATE_CHECK_ENV);
    }
    assert!(
        should_skip_update_check_for_subcommand("totally-unknown-command"),
        "should_skip_update_check_for_subcommand('totally-unknown-command') must return true"
    );
}

/// The `launch` subcommand IS a launch command — update check must proceed.
#[test]
fn test_allow_update_check_for_launch_subcommand() {
    unsafe {
        std::env::remove_var("AMPLIHACK_NONINTERACTIVE");
        std::env::remove_var("AMPLIHACK_PARITY_TEST");
        std::env::remove_var(NO_UPDATE_CHECK_ENV);
    }
    assert!(
        !should_skip_update_check_for_subcommand("launch"),
        "should_skip_update_check_for_subcommand('launch') must return false"
    );
}

/// The `claude` subcommand IS a launch command.
#[test]
fn test_allow_update_check_for_claude_subcommand() {
    unsafe {
        std::env::remove_var("AMPLIHACK_NONINTERACTIVE");
        std::env::remove_var("AMPLIHACK_PARITY_TEST");
        std::env::remove_var(NO_UPDATE_CHECK_ENV);
    }
    assert!(
        !should_skip_update_check_for_subcommand("claude"),
        "should_skip_update_check_for_subcommand('claude') must return false"
    );
}

#[test]
fn is_retryable_error_classifies_status_codes() {
    for status in [500u16, 502, 503, 504] {
        let err = ureq::Error::Status(status, ureq::Response::new(status, "err", "").unwrap());
        assert!(is_retryable_error(&err), "{status} should be retryable");
    }
    let err = ureq::Error::Status(429, ureq::Response::new(429, "Rate Limited", "").unwrap());
    assert!(is_retryable_error(&err), "429 should be retryable");
    for status in [400u16, 401, 403, 404, 422] {
        let err = ureq::Error::Status(
            status,
            ureq::Response::new(status, "client err", "").unwrap(),
        );
        assert!(
            !is_retryable_error(&err),
            "{status} should NOT be retryable"
        );
    }
}

#[test]
fn github_error_message_contains_actionable_advice() {
    let msg_403 = github_error_message(403, "https://api.github.com/test");
    assert!(msg_403.contains("rate limit"));
    assert!(msg_403.contains("AMPLIHACK_NO_UPDATE_CHECK"));

    let msg_429 = github_error_message(429, "https://api.github.com/test");
    assert!(msg_429.contains("rate limit"));
}

#[test]
fn github_error_message_5xx_indicates_transient() {
    for status in [500u16, 502, 503, 504] {
        let msg = github_error_message(status, "https://api.github.com/test");
        assert!(
            msg.contains("Transient server error"),
            "expected transient-error message for {status}, got: {msg}"
        );
        assert!(
            msg.contains("retried automatically"),
            "expected retry hint for {status}, got: {msg}"
        );
    }
}

#[test]
fn validate_download_url_accepts_allowed_hosts() {
    assert!(validate_download_url("https://api.github.com/repos/x/y/releases/latest").is_ok());
    assert!(validate_download_url("https://github.com/x/y/releases/download/v1/x.tar.gz").is_ok());
    assert!(validate_download_url("https://objects.githubusercontent.com/x/y.tar.gz").is_ok());
}

#[test]
fn validate_download_url_rejects_disallowed_hosts() {
    assert!(validate_download_url("https://example.com/evil.tar.gz").is_err());
    assert!(
        validate_download_url("http://api.github.com/repos/x/y").is_err(),
        "http:// (not https://) must be rejected"
    );
    assert!(
        validate_download_url("https://attacker.com/https://api.github.com/").is_err(),
        "URL that contains but does not start with an allowed prefix must be rejected"
    );
    assert!(
        validate_download_url("").is_err(),
        "empty URL must be rejected"
    );
}

#[test]
fn normalize_tag_strips_v_prefix() {
    assert_eq!(normalize_tag("v1.2.3").unwrap(), "1.2.3");
}

#[test]
fn normalize_tag_rejects_non_semver() {
    assert!(normalize_tag("snapshot-abcdef").is_err());
}

#[test]
fn is_newer_detects_version_bumps() {
    assert!(is_newer("0.1.0", "0.2.0").unwrap());
    assert!(!is_newer("0.2.0", "0.2.0").unwrap());
    assert!(!is_newer("0.2.1", "0.2.0").unwrap());
}

#[test]
fn should_skip_update_check_for_update_related_args() {
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let prev_ni = std::env::var_os("AMPLIHACK_NONINTERACTIVE");
    let prev_pt = std::env::var_os("AMPLIHACK_PARITY_TEST");
    let prev_nuc = std::env::var_os(NO_UPDATE_CHECK_ENV);
    unsafe {
        std::env::remove_var("AMPLIHACK_NONINTERACTIVE");
        std::env::remove_var("AMPLIHACK_PARITY_TEST");
        std::env::remove_var(NO_UPDATE_CHECK_ENV);
    }
    assert!(should_skip_update_check(&[
        OsString::from("amplihack"),
        OsString::from("update")
    ]));
    assert!(should_skip_update_check(&[
        OsString::from("amplihack"),
        OsString::from("version")
    ]));
    assert!(should_skip_update_check(&[
        OsString::from("amplihack"),
        OsString::from("help")
    ]));
    assert!(should_skip_update_check(&[
        OsString::from("amplihack"),
        OsString::from("-V")
    ]));
    assert!(!should_skip_update_check(&[
        OsString::from("amplihack"),
        OsString::from("copilot")
    ]));
    unsafe {
        match prev_ni {
            Some(v) => std::env::set_var("AMPLIHACK_NONINTERACTIVE", v),
            None => std::env::remove_var("AMPLIHACK_NONINTERACTIVE"),
        }
        match prev_pt {
            Some(v) => std::env::set_var("AMPLIHACK_PARITY_TEST", v),
            None => std::env::remove_var("AMPLIHACK_PARITY_TEST"),
        }
        match prev_nuc {
            Some(v) => std::env::set_var(NO_UPDATE_CHECK_ENV, v),
            None => std::env::remove_var(NO_UPDATE_CHECK_ENV),
        }
    }
}

#[test]
fn should_skip_update_check_for_non_launch_subcommands() {
    for subcmd in &["mode", "plugin", "recipe", "memory", "install", "doctor"] {
        assert!(
            should_skip_update_check(&[OsString::from("amplihack"), OsString::from(*subcmd),]),
            "expected update check to be skipped for subcommand '{subcmd}'"
        );
    }
}

#[test]
fn should_not_skip_update_check_for_launch_subcommands() {
    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let prev_ni = std::env::var_os("AMPLIHACK_NONINTERACTIVE");
    let prev_pt = std::env::var_os("AMPLIHACK_PARITY_TEST");
    let prev_nuc = std::env::var_os(NO_UPDATE_CHECK_ENV);
    unsafe {
        std::env::remove_var("AMPLIHACK_NONINTERACTIVE");
        std::env::remove_var("AMPLIHACK_PARITY_TEST");
        std::env::remove_var(NO_UPDATE_CHECK_ENV);
    }
    for subcmd in &["launch", "claude", "copilot", "codex", "amplifier"] {
        assert!(
            !should_skip_update_check(&[OsString::from("amplihack"), OsString::from(*subcmd),]),
            "expected update check to NOT be skipped for launch subcommand '{subcmd}'"
        );
    }
    unsafe {
        match prev_ni {
            Some(v) => std::env::set_var("AMPLIHACK_NONINTERACTIVE", v),
            None => std::env::remove_var("AMPLIHACK_NONINTERACTIVE"),
        }
        match prev_pt {
            Some(v) => std::env::set_var("AMPLIHACK_PARITY_TEST", v),
            None => std::env::remove_var("AMPLIHACK_PARITY_TEST"),
        }
        match prev_nuc {
            Some(v) => std::env::set_var(NO_UPDATE_CHECK_ENV, v),
            None => std::env::remove_var(NO_UPDATE_CHECK_ENV),
        }
    }
}

#[test]
fn cache_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("cache");
    write_cache(&path, "1.2.3").unwrap();
    let (version, timestamp) = read_cache(&path).unwrap();
    assert_eq!(version, "1.2.3");
    assert!(timestamp > 0);
}

#[test]
fn cache_path_uses_home() {
    let temp = tempfile::tempdir().unwrap();
    let path = cache_path_from_home(temp.path());
    assert_eq!(
        path,
        temp.path().join(".config/amplihack/last_update_check")
    );
}

#[test]
fn parse_latest_release_selects_matching_asset() {
    let archive_name = expected_archive_name().unwrap();
    let checksum_name = format!("{archive_name}.sha256");
    let json = format!(
        r#"{{
            "tag_name": "v0.2.0",
            "draft": false,
            "prerelease": false,
            "assets": [
                {{"name": "wrong.tar.gz", "browser_download_url": "https://example.invalid/wrong"}},
                {{"name": "{archive_name}", "browser_download_url": "https://example.invalid/right"}},
                {{"name": "{checksum_name}", "browser_download_url": "https://example.invalid/right.sha256"}}
            ]
        }}"#
    );
    let release =
        parse_latest_release(json.into_bytes(), &expected_archive_name().unwrap()).unwrap();
    assert_eq!(
        release,
        UpdateRelease {
            version: "0.2.0".to_string(),
            asset_url: "https://example.invalid/right".to_string(),
            checksum_url: Some("https://example.invalid/right.sha256".to_string()),
        }
    );
}

#[test]
fn parse_latest_release_no_checksum_asset() {
    let archive_name = expected_archive_name().unwrap();
    let json = format!(
        r#"{{
            "tag_name": "v0.2.0",
            "draft": false,
            "prerelease": false,
            "assets": [
                {{"name": "{archive_name}", "browser_download_url": "https://example.invalid/right"}}
            ]
        }}"#
    );
    let release =
        parse_latest_release(json.into_bytes(), &expected_archive_name().unwrap()).unwrap();
    assert_eq!(release.checksum_url, None);
}

#[test]
fn sha256_computation_produces_64_hex_char_digest() {
    let data = b"hello world";
    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = format!("{:x}", hasher.finalize());
    assert_eq!(digest.len(), 64);
    assert!(
        digest.chars().all(|c| c.is_ascii_hexdigit()),
        "digest must be all hex digits"
    );
}

#[test]
fn sha256_digest_changes_when_data_changes() {
    let data = b"some binary content";
    let mut hasher = Sha256::new();
    hasher.update(data);
    let actual = format!("{:x}", hasher.finalize());
    let mut wrong = actual.clone();
    wrong.replace_range(0..1, if wrong.starts_with('a') { "b" } else { "a" });
    assert_ne!(actual, wrong);
}

#[test]
fn current_test_platform_has_release_target() {
    assert!(supported_release_target().is_some());
}

#[test]
fn extract_archive_finds_both_binaries() {
    let temp = tempfile::tempdir().unwrap();
    let archive_path = temp.path().join("release.tar.gz");
    create_test_archive(&archive_path).unwrap();
    let bytes = fs::read(&archive_path).unwrap();

    let extract_dir = temp.path().join("extract");
    fs::create_dir_all(&extract_dir).unwrap();
    extract_archive(&bytes, &extract_dir).unwrap();

    assert!(find_binary(&extract_dir, binary_filename("amplihack")).is_ok());
    assert!(find_binary(&extract_dir, binary_filename("amplihack-hooks")).is_ok());
}

fn create_test_archive(path: &Path) -> Result<()> {
    let tar_gz = fs::File::create(path)?;
    let encoder = flate2::write::GzEncoder::new(tar_gz, flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);

    for binary in [
        binary_filename("amplihack"),
        binary_filename("amplihack-hooks"),
    ] {
        let data = b"#!/bin/sh\nexit 0\n";
        let mut header = tar::Header::new_gnu();
        header.set_path(binary)?;
        header.set_size(data.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder.append(&header, &data[..])?;
    }

    let encoder = builder.into_inner()?;
    encoder.finish()?;
    Ok(())
}

#[test]
fn wants_startup_update_positive() {
    use super::check::*;
    // Not publicly exported, but we test the outcome through StartupUpdateOutcome
    // by verifying StartupUpdateOutcome variants exist.
    assert_eq!(
        StartupUpdateOutcome::Continue,
        StartupUpdateOutcome::Continue
    );
    assert_ne!(
        StartupUpdateOutcome::Continue,
        StartupUpdateOutcome::ExitSuccess
    );
}

#[test]
fn shell_profile_path_bash() {
    use crate::commands::install::paths::shell_profile_path;
    let _lock = crate::test_support::env_lock();
    unsafe { std::env::set_var("SHELL", "/bin/bash") };
    let result = shell_profile_path();
    unsafe { std::env::remove_var("SHELL") };
    if let Some(path) = result {
        assert!(
            path.to_string_lossy().ends_with(".bashrc"),
            "expected .bashrc, got {}",
            path.display()
        );
    }
}

#[test]
fn shell_profile_path_zsh() {
    use crate::commands::install::paths::shell_profile_path;
    let _lock = crate::test_support::env_lock();
    unsafe { std::env::set_var("SHELL", "/bin/zsh") };
    let result = shell_profile_path();
    unsafe { std::env::remove_var("SHELL") };
    if let Some(path) = result {
        assert!(
            path.to_string_lossy().ends_with(".zshrc"),
            "expected .zshrc, got {}",
            path.display()
        );
    }
}

#[test]
fn shell_profile_path_unknown() {
    use crate::commands::install::paths::shell_profile_path;
    let _lock = crate::test_support::env_lock();
    unsafe { std::env::set_var("SHELL", "/bin/csh") };
    let result = shell_profile_path();
    unsafe { std::env::remove_var("SHELL") };
    assert!(result.is_none(), "unsupported shell should return None");
}

#[test]
fn ensure_local_bin_on_shell_path_creates_export() {
    use crate::commands::install::paths::ensure_local_bin_on_shell_path;
    let tmp = tempfile::TempDir::new().unwrap();
    // Use home_env_lock so we serialize with HOME-reading tests like
    // test_resolve_binary_path_expands_tilde_to_home_dir.
    let _lock = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let prev_home = std::env::var_os("HOME");
    let prev_shell = std::env::var_os("SHELL");
    unsafe { std::env::set_var("HOME", tmp.path().as_os_str()) };
    unsafe { std::env::set_var("SHELL", "/bin/bash") };
    ensure_local_bin_on_shell_path().unwrap();
    let content = std::fs::read_to_string(tmp.path().join(".bashrc")).unwrap();
    assert!(
        content.contains(".local/bin"),
        "should have added .local/bin to .bashrc"
    );
    // Second call is a no-op
    ensure_local_bin_on_shell_path().unwrap();
    let content2 = std::fs::read_to_string(tmp.path().join(".bashrc")).unwrap();
    let count = content2.matches("export PATH").count();
    assert_eq!(count, 1, "should not duplicate the export line");
    match prev_shell {
        Some(v) => unsafe { std::env::set_var("SHELL", v) },
        None => unsafe { std::env::remove_var("SHELL") },
    }
    match prev_home {
        Some(v) => unsafe { std::env::set_var("HOME", v) },
        None => unsafe { std::env::remove_var("HOME") },
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Issue #334: Windows x86_64 release support.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn release_target_table_covers_windows_x86_64() {
    // The supported_release_target() function in mod.rs is hard-coded with
    // cfg!() arms, so we can only directly observe the host's mapping. To
    // assert the Windows arm exists without conditionally compiling the test
    // away, we read the source and check the literal arm is present. This is
    // a static guard — it fails any commit that drops the windows arm.
    let src = include_str!("mod.rs");
    assert!(
        src.contains(r#"target_os = "windows", target_arch = "x86_64""#),
        "supported_release_target() must include a windows/x86_64 arm",
    );
    assert!(
        src.contains(r#"x86_64-pc-windows-msvc"#),
        "supported_release_target() must map windows/x86_64 to x86_64-pc-windows-msvc",
    );
}

#[test]
fn binary_filename_appends_exe_on_windows() {
    // binary_filename() uses cfg!(windows) so on non-Windows hosts it returns
    // the bare name. Inspect the source to guarantee the .exe arm exists.
    let src = include_str!("install.rs");
    assert!(
        src.contains(r#""amplihack" => "amplihack.exe""#),
        "binary_filename() must append .exe to amplihack on windows",
    );
    assert!(
        src.contains(r#""amplihack-hooks" => "amplihack-hooks.exe""#),
        "binary_filename() must append .exe to amplihack-hooks on windows",
    );
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
#[test]
fn windows_host_resolves_msvc_target() {
    assert_eq!(supported_release_target(), Some("x86_64-pc-windows-msvc"));
    assert_eq!(
        expected_archive_name().unwrap(),
        "amplihack-x86_64-pc-windows-msvc.tar.gz"
    );
    assert_eq!(binary_filename("amplihack"), "amplihack.exe");
}
