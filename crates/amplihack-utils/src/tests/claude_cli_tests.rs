use super::*;

// ---------------------------------------------------------------------------
// parse_semver
// ---------------------------------------------------------------------------

#[test]
fn parse_semver_simple() {
    assert_eq!(parse_semver("1.2.3"), Some("1.2.3".into()));
}

#[test]
fn parse_semver_with_prefix() {
    assert_eq!(parse_semver("claude v1.0.23"), Some("1.0.23".into()));
}

#[test]
fn parse_semver_multiline() {
    let text = "Claude Code CLI\nversion 2.5.10\nmore stuff";
    assert_eq!(parse_semver(text), Some("2.5.10".into()));
}

#[test]
fn parse_semver_no_match() {
    assert_eq!(parse_semver("no version here"), None);
}

#[test]
fn parse_semver_partial() {
    assert_eq!(parse_semver("1.2"), None);
}

// ---------------------------------------------------------------------------
// is_newer
// ---------------------------------------------------------------------------

#[test]
fn is_newer_major() {
    assert!(is_newer("1.0.0", "2.0.0"));
}

#[test]
fn is_newer_minor() {
    assert!(is_newer("1.2.0", "1.3.0"));
}

#[test]
fn is_newer_patch() {
    assert!(is_newer("1.2.3", "1.2.4"));
}

#[test]
fn is_newer_same() {
    assert!(!is_newer("1.2.3", "1.2.3"));
}

#[test]
fn is_newer_older() {
    assert!(!is_newer("2.0.0", "1.9.9"));
}

#[test]
fn is_newer_with_v_prefix() {
    assert!(is_newer("v1.0.0", "v2.0.0"));
}

#[test]
fn is_newer_invalid_returns_false() {
    assert!(!is_newer("abc", "1.0.0"));
    assert!(!is_newer("1.0.0", "xyz"));
}

// ---------------------------------------------------------------------------
// VersionStatus serde round-trip
// ---------------------------------------------------------------------------

#[test]
fn version_status_current_serde() {
    let v = VersionStatus::Current("1.2.3".into());
    let json = serde_json::to_string(&v).expect("serialize");
    let deser: VersionStatus = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(deser, v);
}

#[test]
fn version_status_update_available_serde() {
    let v = VersionStatus::UpdateAvailable {
        current: "1.0.0".into(),
        latest: "2.0.0".into(),
    };
    let json = serde_json::to_string(&v).expect("serialize");
    let deser: VersionStatus = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(deser, v);
}

#[test]
fn version_status_unknown_serde() {
    let v = VersionStatus::Unknown;
    let json = serde_json::to_string(&v).expect("serialize");
    let deser: VersionStatus = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(deser, VersionStatus::Unknown);
}

// ---------------------------------------------------------------------------
// get_claude_cli_path
// ---------------------------------------------------------------------------

#[test]
fn get_claude_cli_path_does_not_panic() {
    // The result depends on whether claude is installed; just exercise the
    // function and ensure it doesn't blow up.
    let _ = get_claude_cli_path();
}

// ---------------------------------------------------------------------------
// npm_global_dir / npm_global_bin
// ---------------------------------------------------------------------------

#[test]
fn npm_global_dir_based_on_home() {
    if let Some(dir) = npm_global_dir() {
        assert!(
            dir.to_str().unwrap_or("").contains(".npm-global"),
            "expected .npm-global in path: {}",
            dir.display()
        );
    }
    // If HOME is not set, None is acceptable.
}

#[test]
fn npm_global_bin_is_subdir() {
    if let Some(bin) = npm_global_bin() {
        assert!(
            bin.ends_with("bin"),
            "expected bin suffix: {}",
            bin.display()
        );
    }
}

// ---------------------------------------------------------------------------
// validate_binary
// ---------------------------------------------------------------------------

#[test]
fn validate_binary_echo() {
    // `echo` accepts --version on some systems; even if it doesn't, the
    // function should not panic.
    let echo = PathBuf::from("/usr/bin/echo");
    if echo.exists() {
        // echo --version may or may not succeed, that's fine.
        let _ = validate_binary(&echo);
    }
}

#[test]
fn validate_binary_nonexistent() {
    assert!(!validate_binary(Path::new("/nonexistent/binary")));
}

// ---------------------------------------------------------------------------
// ClaudeCliError display
// ---------------------------------------------------------------------------

#[test]
fn error_display_npm_not_found() {
    let e = ClaudeCliError::NpmNotFound;
    let msg = e.to_string();
    assert!(msg.contains("npm"), "error should mention npm: {msg}");
}

#[test]
fn error_display_install_failed() {
    let e = ClaudeCliError::InstallFailed {
        code: Some(1),
        stderr: "permission denied".into(),
    };
    let msg = e.to_string();
    assert!(msg.contains("permission denied"), "{msg}");
}

#[test]
fn error_display_validation_failed() {
    let e = ClaudeCliError::ValidationFailed {
        path: "/usr/bin/claude".into(),
        reason: "segfault".into(),
    };
    let msg = e.to_string();
    assert!(msg.contains("segfault"), "{msg}");
}
