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
    match r {
        Ok(cr) => assert!(!cr.success()),
        Err(_) => {} // Also acceptable.
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
