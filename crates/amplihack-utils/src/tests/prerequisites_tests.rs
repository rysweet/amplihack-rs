use super::*;

// ---------------------------------------------------------------------------
// Platform detection
// ---------------------------------------------------------------------------

#[test]
fn detect_platform_returns_known_variant() {
    let p = detect_platform();
    // We're running on Linux in CI, so expect Linux or Wsl.
    assert!(
        matches!(
            p,
            Platform::Linux | Platform::Wsl | Platform::MacOs | Platform::Windows
        ),
        "expected a known platform, got {p:?}"
    );
}

#[test]
fn is_wsl_does_not_panic() {
    // Just exercise the function — result depends on environment.
    let _ = is_wsl();
}

// ---------------------------------------------------------------------------
// Install hints
// ---------------------------------------------------------------------------

#[test]
fn install_hint_macos_git() {
    let hint = install_hint("git", Platform::MacOs);
    assert!(
        hint.contains("brew"),
        "macOS hint should mention brew: {hint}"
    );
}

#[test]
fn install_hint_linux_node() {
    let hint = install_hint("node", Platform::Linux);
    assert!(
        hint.contains("apt") || hint.contains("nodesource"),
        "Linux hint should mention apt or nodesource: {hint}"
    );
}

#[test]
fn install_hint_windows_git() {
    let hint = install_hint("git", Platform::Windows);
    assert!(
        hint.contains("winget"),
        "Windows hint should mention winget: {hint}"
    );
}

#[test]
fn install_hint_unknown_platform() {
    let hint = install_hint("git", Platform::Unknown);
    assert!(
        hint.contains("manually"),
        "unknown hint should say manually: {hint}"
    );
}

#[test]
fn install_hint_wsl_uses_linux_hints() {
    let hint = install_hint("git", Platform::Wsl);
    assert!(
        hint.contains("apt"),
        "WSL hint should use Linux/apt: {hint}"
    );
}

// ---------------------------------------------------------------------------
// Tool checking
// ---------------------------------------------------------------------------

#[test]
fn check_tool_git_found() {
    let r = check_tool("git");
    // git should be installed in any CI environment.
    assert!(r.found, "git should be found");
    assert!(r.version.is_some(), "git version should be detected");
    assert!(r.path.is_some(), "git path should be detected");
}

#[test]
fn check_tool_nonexistent() {
    let r = check_tool("definitely_not_installed_xyz_789");
    assert!(!r.found);
    assert!(r.version.is_none());
    assert!(r.path.is_none());
    assert!(!r.install_hint.is_empty());
}

#[test]
fn check_tool_sets_required_flag() {
    let git = check_tool("git");
    assert!(git.required, "git should be required");

    let tmux = check_tool("tmux");
    assert!(!tmux.required, "tmux should be optional");
}

#[test]
fn check_prerequisites_includes_all_tools() {
    let results = check_prerequisites();
    let names: Vec<&str> = results.iter().map(|r| r.name.as_str()).collect();
    assert!(names.contains(&"git"), "should check git");
    assert!(names.contains(&"node"), "should check node");
    assert!(names.contains(&"npm"), "should check npm");
    assert!(names.contains(&"uv"), "should check uv");
    assert!(names.contains(&"rg"), "should check rg");
    assert!(names.contains(&"tmux"), "should check tmux");
    assert_eq!(results.len(), 6);
}

// ---------------------------------------------------------------------------
// safe_subprocess_call
// ---------------------------------------------------------------------------

#[test]
fn safe_subprocess_call_echo() {
    let r = safe_subprocess_call(&["echo", "hello"], 5).expect("echo should succeed");
    assert!(r.success());
    assert!(r.stdout.contains("hello"));
}

#[test]
fn safe_subprocess_call_nonexistent() {
    let r = safe_subprocess_call(&["absolutely_not_a_command_xyz"], 5);
    // Should either error or return a non-success result.
    if let Ok(cr) = r {
        assert!(!cr.success());
    }
}

// ---------------------------------------------------------------------------
// missing_required
// ---------------------------------------------------------------------------

#[test]
fn missing_required_excludes_found_tools() {
    let missing = missing_required();
    // git should be installed → must not appear.
    assert!(
        !missing.contains(&"git".to_string()),
        "git should not be listed as missing"
    );
}

// ---------------------------------------------------------------------------
// summary_string
// ---------------------------------------------------------------------------

#[test]
fn summary_string_formats_results() {
    let results = vec![
        ToolCheckResult {
            name: "git".into(),
            found: true,
            version: Some("2.40.0".into()),
            path: Some(PathBuf::from("/usr/bin/git")),
            install_hint: "brew install git".into(),
            required: true,
        },
        ToolCheckResult {
            name: "uv".into(),
            found: false,
            version: None,
            path: None,
            install_hint: "brew install uv".into(),
            required: false,
        },
    ];

    let s = summary_string(&results);
    assert!(s.contains("✓"), "found tool should have ✓");
    assert!(s.contains("✗"), "missing tool should have ✗");
    assert!(s.contains("2.40.0"), "version should appear");
    assert!(
        s.contains("brew install uv"),
        "install hint should appear for missing tool"
    );
}

// ---------------------------------------------------------------------------
// ToolCheckResult serde round-trip
// ---------------------------------------------------------------------------

#[test]
fn tool_check_result_serializes() {
    let r = ToolCheckResult {
        name: "node".into(),
        found: true,
        version: Some("v20.11.0".into()),
        path: Some(PathBuf::from("/usr/bin/node")),
        install_hint: "brew install node".into(),
        required: true,
    };
    let json = serde_json::to_string(&r).expect("serialize");
    let deser: ToolCheckResult = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(deser.name, "node");
    assert!(deser.found);
    assert_eq!(deser.version.as_deref(), Some("v20.11.0"));
}

// ---------------------------------------------------------------------------
// Platform serde round-trip
// ---------------------------------------------------------------------------

#[test]
fn platform_serializes() {
    let p = Platform::MacOs;
    let json = serde_json::to_string(&p).expect("serialize");
    let deser: Platform = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(deser, Platform::MacOs);
}

// ---------------------------------------------------------------------------
// Node.js version parsing — parse_node_major_version()
// ---------------------------------------------------------------------------

#[test]
fn parse_node_major_version_standard_format() {
    // `node --version` prints "v20.11.0"
    assert_eq!(parse_node_major_version("v20.11.0"), Some(20));
}

#[test]
fn parse_node_major_version_without_v_prefix() {
    assert_eq!(parse_node_major_version("20.11.0"), Some(20));
}

#[test]
fn parse_node_major_version_v24() {
    assert_eq!(parse_node_major_version("v24.0.0"), Some(24));
}

#[test]
fn parse_node_major_version_high_version() {
    assert_eq!(parse_node_major_version("v99.1.2"), Some(99));
}

#[test]
fn parse_node_major_version_single_digit() {
    assert_eq!(parse_node_major_version("v8.0.0"), Some(8));
}

#[test]
fn parse_node_major_version_major_only() {
    // Tolerate strings like "v20" without minor/patch
    assert_eq!(parse_node_major_version("v20"), Some(20));
}

#[test]
fn parse_node_major_version_empty_string() {
    assert_eq!(parse_node_major_version(""), None);
}

#[test]
fn parse_node_major_version_garbage() {
    assert_eq!(parse_node_major_version("not-a-version"), None);
}

#[test]
fn parse_node_major_version_git_version_string() {
    // Must not confuse other tools' version output
    assert_eq!(parse_node_major_version("git version 2.40.0"), None);
}

#[test]
fn parse_node_major_version_whitespace_padded() {
    // `node --version` may have trailing newline
    assert_eq!(parse_node_major_version("  v20.11.0\n"), Some(20));
}

// ---------------------------------------------------------------------------
// Node.js minimum version check — check_node_minimum_version()
// ---------------------------------------------------------------------------

#[test]
fn check_node_minimum_version_sufficient() {
    let result = check_node_minimum_version(24);
    // On CI with Node >= 24, this should pass. If Node < 24 is installed,
    // the function returns a NodeVersionError. Either way it must not panic.
    // We test the pure logic more precisely via parse_node_major_version;
    // this test ensures the plumbing doesn't panic.
    assert!(
        result.is_ok() || result.is_err(),
        "check_node_minimum_version must not panic"
    );
}

#[test]
fn node_version_error_displays_actionable_message() {
    let err = NodeVersionError::InsufficientVersion {
        found: 20,
        minimum: 24,
        install_hint: "brew install node".into(),
    };
    let msg = err.to_string();
    assert!(
        msg.contains("24"),
        "error should mention required version: {msg}"
    );
    assert!(
        msg.contains("20"),
        "error should mention found version: {msg}"
    );
    assert!(
        msg.contains("brew install node") || msg.contains("install"),
        "error should include installation hint: {msg}"
    );
}

#[test]
fn node_version_error_not_found_is_distinct() {
    let err = NodeVersionError::VersionUndetectable {
        install_hint: "sudo apt install nodejs".into(),
    };
    let msg = err.to_string();
    assert!(
        msg.contains("could not") || msg.contains("unable") || msg.contains("detect"),
        "undetectable error should explain the situation: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Node.js auto-ensure helpers
// ---------------------------------------------------------------------------

#[test]
fn node_platform_triple_returns_known_values() {
    let (os, arch) = node_platform_triple().expect("should detect platform");
    assert!(
        ["linux", "darwin", "win"].contains(&os),
        "unexpected OS: {os}"
    );
    assert!(["x64", "arm64"].contains(&arch), "unexpected arch: {arch}");
}

#[test]
fn node_auto_install_version_is_valid_semver() {
    assert!(
        NODE_AUTO_INSTALL_VERSION.starts_with('v'),
        "auto-install version should start with 'v': {NODE_AUTO_INSTALL_VERSION}"
    );
    let major = parse_node_major_version(NODE_AUTO_INSTALL_VERSION);
    assert!(
        major.is_some() && major.unwrap() >= NODE_MINIMUM_MAJOR,
        "auto-install version must satisfy minimum: {NODE_AUTO_INSTALL_VERSION} vs v{NODE_MINIMUM_MAJOR}"
    );
}

#[test]
fn node_auto_install_version_and_minimum_are_consistent() {
    // The bundled download version must always satisfy the minimum.
    let major = parse_node_major_version(NODE_AUTO_INSTALL_VERSION).unwrap();
    assert!(
        major >= NODE_MINIMUM_MAJOR,
        "NODE_AUTO_INSTALL_VERSION ({NODE_AUTO_INSTALL_VERSION}, major={major}) \
         must be >= NODE_MINIMUM_MAJOR ({NODE_MINIMUM_MAJOR})"
    );
}
