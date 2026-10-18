//! Startup repair must follow parsed client intent and preserve strict failures.
#![cfg(unix)]
#[path = "issue_1538_startup_provider/fixture.rs"]
mod fixture;
use fixture::Fixture;
use std::fs;

fn legacy_launch(provider: &str, inherited: &str) {
    let f = Fixture::new(inherited, false);
    let before = f.protected_state();
    let out = f.run(&[provider, "--no-reflection", "--subprocess-safe"]);
    assert!(out.status.success(), "{}", Fixture::diagnostic(&out));
    assert!(f.events().contains(&format!("{provider}:launch")));
    assert_eq!(f.protected_state(), before);
    assert!(!f.events().contains(&"codex:plugin".to_string()));
    assert!(f.home.join(".amplihack/.installed-version").is_file());
    assert!(
        f.home
            .join(".amplihack/amplifier-bundle/recipes/workflow-tdd.yaml")
            .is_file()
    );
}

#[test]
fn explicit_claude_ignores_available_legacy_codex() {
    legacy_launch("claude", "claude");
}

#[test]
fn explicit_copilot_ignores_available_legacy_codex() {
    legacy_launch("copilot", "copilot");
}

#[test]
fn explicit_claude_overrides_inherited_codex() {
    legacy_launch("claude", "codex");
}

#[test]
fn explicit_copilot_overrides_inherited_codex() {
    legacy_launch("copilot", "codex");
}

#[test]
fn selected_codex_overrides_inherited_claude_and_preserves_foreign_state() {
    let f = Fixture::new("claude", false);
    let before = f.protected_state();
    let out = f.run(&["codex", "--no-reflection", "--subprocess-safe"]);
    assert!(!out.status.success(), "{}", Fixture::diagnostic(&out));
    assert!(Fixture::diagnostic(&out).contains("legacy or changed Codex ownership proof"));
    assert!(f.events().contains(&"codex:plugin".to_string()));
    assert!(!f.events().contains(&"codex:launch".to_string()));
    assert_eq!(f.protected_state(), before);
}

#[test]
fn selected_codex_without_native_support_still_fails() {
    let f = Fixture::new("claude", true);
    let before = f.protected_state();
    let out = f.run(&["codex", "--no-reflection", "--subprocess-safe"]);
    assert!(!out.status.success(), "{}", Fixture::diagnostic(&out));
    assert!(Fixture::diagnostic(&out).contains("selected Codex requires native plugin support"));
    assert!(!f.events().contains(&"codex:launch".to_string()));
    assert_eq!(f.protected_state(), before);
}

#[test]
fn generic_asset_repair_failure_is_not_silenced() {
    let f = Fixture::new("claude", false);
    let obstacle = f.home.join(".amplihack/.claude");
    fs::write(&obstacle, b"foreign staging obstacle\n").unwrap();
    let before = f.protected_state();
    let out = f.run(&["claude", "--no-reflection", "--subprocess-safe"]);
    assert!(!out.status.success(), "{}", Fixture::diagnostic(&out));
    assert!(Fixture::diagnostic(&out).contains("self-heal failed"));
    assert!(!f.events().contains(&"claude:launch".to_string()));
    assert_eq!(fs::read(obstacle).unwrap(), b"foreign staging obstacle\n");
    assert_eq!(f.protected_state(), before);
    assert!(!f.home.join(".amplihack/.installed-version").exists());
}

#[test]
fn direct_install_keeps_strict_legacy_codex_guard() {
    let f = Fixture::new("claude", false);
    let before = f.protected_state();
    let out = f.run(&["install", "--local", f.source.to_str().unwrap()]);
    assert!(!out.status.success(), "{}", Fixture::diagnostic(&out));
    assert!(Fixture::diagnostic(&out).contains("legacy or changed Codex ownership proof"));
    assert!(f.events().contains(&"codex:plugin".to_string()));
    assert_eq!(f.protected_state(), before);
}
