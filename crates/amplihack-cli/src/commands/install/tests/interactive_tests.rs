//! TDD tests for the interactive install wizard (issue #433).
//!
//! These tests define the contract for `interactive.rs` — the pure logic layer
//! of the interactive install wizard. They cover:
//!
//! - Enum variant exhaustiveness and display
//! - Default config construction
//! - TTY validation and fallback
//! - Repo-local scope resolution (git repo detection)
//! - Flag interaction (`--interactive` + `--local`)
//! - Manifest field serialization round-trips
//! - Config application to InstallManifest
//!
//! Following TDD methodology: these tests are written FIRST and will fail until
//! the `interactive` module is implemented.

use super::*;
use std::fs;
use std::path::Path;

// ---------------------------------------------------------------------------
// 1. DefaultTool enum — display names and exhaustiveness
// ---------------------------------------------------------------------------

#[test]
fn default_tool_display_names_are_human_readable() {
    // The wizard presents tool choices to users; display names must be clear.
    assert_eq!(
        interactive::DefaultTool::Claude.display_name(),
        "Claude Code"
    );
    assert_eq!(
        interactive::DefaultTool::Copilot.display_name(),
        "GitHub Copilot"
    );
    assert_eq!(
        interactive::DefaultTool::Codex.display_name(),
        "OpenAI Codex CLI"
    );
}

#[test]
fn default_tool_all_variants_returns_three_options() {
    let variants = interactive::DefaultTool::all_variants();
    assert_eq!(variants.len(), 3);
    // Order matters — Claude is first (most common default).
    assert_eq!(variants[0], interactive::DefaultTool::Claude);
    assert_eq!(variants[1], interactive::DefaultTool::Copilot);
    assert_eq!(variants[2], interactive::DefaultTool::Codex);
}

#[test]
fn default_tool_serializes_to_lowercase_string() {
    // Manifest stores the tool as a simple lowercase string.
    assert_eq!(interactive::DefaultTool::Claude.as_str(), "claude");
    assert_eq!(interactive::DefaultTool::Copilot.as_str(), "copilot");
    assert_eq!(interactive::DefaultTool::Codex.as_str(), "codex");
}

// ---------------------------------------------------------------------------
// 2. HookScope enum — scope resolution and path building
// ---------------------------------------------------------------------------

#[test]
fn hook_scope_global_returns_global_settings_path() {
    // Global scope always resolves to ~/.claude/settings.json regardless of CWD.
    let scope = interactive::HookScope::Global;
    let path = scope.settings_path_for(Path::new("/nonexistent"));
    // Global ignores the repo_root argument.
    assert!(
        path.to_string_lossy().contains(".claude/settings.json")
            || path.to_string_lossy().ends_with(".claude/settings.json"),
        "global scope must resolve to ~/.claude/settings.json, got: {}",
        path.display()
    );
}

#[test]
fn hook_scope_repo_local_builds_path_under_repo_root() {
    let repo_root = Path::new("/tmp/my-project");
    let scope = interactive::HookScope::RepoLocal;
    let path = scope.settings_path_for(repo_root);
    assert_eq!(
        path,
        repo_root.join(".claude/settings.json"),
        "repo-local scope must resolve to <repo>/.claude/settings.json"
    );
}

#[test]
fn hook_scope_all_variants_returns_two_options() {
    let variants = interactive::HookScope::all_variants();
    assert_eq!(variants.len(), 2);
    assert_eq!(variants[0], interactive::HookScope::Global);
    assert_eq!(variants[1], interactive::HookScope::RepoLocal);
}

// ---------------------------------------------------------------------------
// 3. UpdateCheckPreference enum — serialization and variants
// ---------------------------------------------------------------------------

#[test]
fn update_check_preference_all_variants_returns_four_options() {
    let variants = interactive::UpdateCheckPreference::all_variants();
    assert_eq!(variants.len(), 4);
}

#[test]
fn update_check_preference_serializes_to_stable_strings() {
    // These strings are persisted in the manifest and must remain stable.
    assert_eq!(
        interactive::UpdateCheckPreference::AutoWeekly.as_str(),
        "auto-weekly"
    );
    assert_eq!(
        interactive::UpdateCheckPreference::AutoDaily.as_str(),
        "auto-daily"
    );
    assert_eq!(
        interactive::UpdateCheckPreference::Manual.as_str(),
        "manual"
    );
    assert_eq!(
        interactive::UpdateCheckPreference::Disabled.as_str(),
        "disabled"
    );
}

#[test]
fn update_check_preference_display_names_are_descriptive() {
    // Users see these in the dialoguer prompt.
    let weekly = interactive::UpdateCheckPreference::AutoWeekly;
    assert!(
        weekly.display_name().contains("weekly") || weekly.display_name().contains("Weekly"),
        "display name should mention 'weekly'"
    );
}

// ---------------------------------------------------------------------------
// 4. InteractiveConfig — default construction and builder
// ---------------------------------------------------------------------------

#[test]
fn interactive_config_default_uses_claude_global_auto_weekly() {
    let config = interactive::InteractiveConfig::default();
    assert_eq!(config.default_tool, interactive::DefaultTool::Claude);
    assert_eq!(config.hook_scope, interactive::HookScope::Global);
    assert_eq!(
        config.update_check,
        interactive::UpdateCheckPreference::AutoWeekly
    );
}

#[test]
fn interactive_config_builder_overrides_defaults() {
    let config = interactive::InteractiveConfig {
        default_tool: interactive::DefaultTool::Copilot,
        hook_scope: interactive::HookScope::RepoLocal,
        update_check: interactive::UpdateCheckPreference::Disabled,
    };
    assert_eq!(config.default_tool, interactive::DefaultTool::Copilot);
    assert_eq!(config.hook_scope, interactive::HookScope::RepoLocal);
    assert_eq!(
        config.update_check,
        interactive::UpdateCheckPreference::Disabled
    );
}

// ---------------------------------------------------------------------------
// 5. TTY validation — non-interactive fallback
// ---------------------------------------------------------------------------

#[test]
fn validate_tty_returns_false_in_non_tty_environment() {
    // In test environments stdin is never a real TTY, so this should return false.
    let is_tty = interactive::validate_tty();
    assert!(
        !is_tty,
        "validate_tty() must return false when stdin is not a terminal"
    );
}

#[test]
fn maybe_run_wizard_returns_default_config_when_non_interactive() {
    // When interactive=false, the wizard is skipped entirely and defaults are used.
    let result = interactive::maybe_run_wizard(false);
    assert!(result.is_ok(), "non-interactive mode must not error");
    assert!(
        result.unwrap().is_none(),
        "non-interactive mode must return None (no config override)"
    );
}

#[test]
fn maybe_run_wizard_falls_back_to_defaults_in_non_tty() {
    // Even when interactive=true, if stdin is not a TTY we fall back gracefully.
    // In the test runner, stdin is NOT a TTY, so this tests the fallback path.
    let result = interactive::maybe_run_wizard(true);
    assert!(result.is_ok(), "non-TTY fallback must not error");
    // Returns Some(default config) with a warning to stderr — not None.
    // The caller still gets a config to apply (the defaults), distinguishing
    // "user asked for interactive but couldn't get TTY" from "user never asked".
    let config = result.unwrap();
    assert!(
        config.is_some(),
        "interactive=true in non-TTY should return Some(default config), not None"
    );
    let config = config.unwrap();
    assert_eq!(
        config.default_tool,
        interactive::DefaultTool::Claude,
        "non-TTY fallback must use Claude as default tool"
    );
}

// ---------------------------------------------------------------------------
// 6. Repo-local scope resolution — git repo detection
// ---------------------------------------------------------------------------

#[test]
fn resolve_hook_scope_falls_back_to_global_when_no_git_repo() {
    // If user picks RepoLocal but CWD has no .git, we fall back to Global.
    let temp = tempfile::tempdir().unwrap();
    // temp dir has no .git — not a repo
    let resolved = interactive::resolve_hook_scope(interactive::HookScope::RepoLocal, temp.path());
    assert_eq!(
        resolved,
        interactive::HookScope::Global,
        "repo-local scope must fall back to global when no git repo found"
    );
}

#[test]
fn resolve_hook_scope_keeps_repo_local_when_git_repo_exists() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join(".git")).unwrap();
    let resolved = interactive::resolve_hook_scope(interactive::HookScope::RepoLocal, temp.path());
    assert_eq!(
        resolved,
        interactive::HookScope::RepoLocal,
        "repo-local scope must be preserved when .git directory exists"
    );
}

#[test]
fn resolve_hook_scope_preserves_global_regardless_of_git() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join(".git")).unwrap();
    let resolved = interactive::resolve_hook_scope(interactive::HookScope::Global, temp.path());
    assert_eq!(
        resolved,
        interactive::HookScope::Global,
        "global scope must not be overridden even when git repo exists"
    );
}

// ---------------------------------------------------------------------------
// 7. Manifest integration — new optional fields
// ---------------------------------------------------------------------------

#[test]
fn install_manifest_deserializes_without_new_fields() {
    // Backward compatibility: existing manifests without default_tool /
    // update_check_preference must deserialize without error.
    let json = r#"{"files":["a.txt"],"dirs":["d"],"binaries":[],"hook_registrations":[]}"#;
    let manifest: InstallManifest = serde_json::from_str(json).unwrap();
    assert_eq!(manifest.files, vec!["a.txt"]);
    assert!(
        manifest.default_tool.is_none(),
        "missing default_tool field must deserialize as None"
    );
    assert!(
        manifest.update_check_preference.is_none(),
        "missing update_check_preference field must deserialize as None"
    );
}

#[test]
fn install_manifest_round_trips_with_new_fields() {
    // New fields serialize and deserialize correctly.
    let mut manifest = InstallManifest::default();
    manifest.default_tool = Some("copilot".to_string());
    manifest.update_check_preference = Some("auto-daily".to_string());

    let json = serde_json::to_string(&manifest).unwrap();
    let deserialized: InstallManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.default_tool.as_deref(), Some("copilot"));
    assert_eq!(
        deserialized.update_check_preference.as_deref(),
        Some("auto-daily")
    );
}

#[test]
fn install_manifest_new_fields_are_optional_in_json() {
    // When both new fields are None, they should either be absent from JSON
    // or present as null — either way, deserialization must succeed.
    let manifest = InstallManifest::default();
    let json = serde_json::to_string(&manifest).unwrap();
    let deserialized: InstallManifest = serde_json::from_str(&json).unwrap();
    assert!(deserialized.default_tool.is_none());
    assert!(deserialized.update_check_preference.is_none());
}

// ---------------------------------------------------------------------------
// 8. apply_config — writes wizard results to manifest
// ---------------------------------------------------------------------------

#[test]
fn apply_config_sets_manifest_fields() {
    let config = interactive::InteractiveConfig {
        default_tool: interactive::DefaultTool::Codex,
        hook_scope: interactive::HookScope::Global,
        update_check: interactive::UpdateCheckPreference::Manual,
    };
    let mut manifest = InstallManifest::default();
    interactive::apply_config(&config, &mut manifest);
    assert_eq!(manifest.default_tool.as_deref(), Some("codex"));
    assert_eq!(manifest.update_check_preference.as_deref(), Some("manual"));
}

#[test]
fn apply_config_overwrites_existing_manifest_fields() {
    let config = interactive::InteractiveConfig {
        default_tool: interactive::DefaultTool::Claude,
        hook_scope: interactive::HookScope::Global,
        update_check: interactive::UpdateCheckPreference::AutoDaily,
    };
    let mut manifest = InstallManifest::default();
    manifest.default_tool = Some("copilot".to_string());
    manifest.update_check_preference = Some("disabled".to_string());

    interactive::apply_config(&config, &mut manifest);
    assert_eq!(
        manifest.default_tool.as_deref(),
        Some("claude"),
        "apply_config must overwrite existing default_tool"
    );
    assert_eq!(
        manifest.update_check_preference.as_deref(),
        Some("auto-daily"),
        "apply_config must overwrite existing update_check_preference"
    );
}

// ---------------------------------------------------------------------------
// 9. Flag interaction: --interactive + --local compose orthogonally
// ---------------------------------------------------------------------------

#[test]
fn interactive_flag_does_not_affect_local_path_resolution() {
    // The interactive flag only controls the wizard; --local controls the
    // install source. They are orthogonal.
    // This test verifies the API contract: maybe_run_wizard() does not
    // take or return a path — it only produces an InteractiveConfig.
    let result = interactive::maybe_run_wizard(false);
    assert!(result.is_ok());
    assert!(result.unwrap().is_none());
    // The local path is handled entirely by run_install(local, interactive)
    // — the wizard never touches it.
}

// ---------------------------------------------------------------------------
// 10. Edge case: empty prompt lists would panic dialoguer
// ---------------------------------------------------------------------------

#[test]
fn all_enum_variant_lists_are_non_empty() {
    // dialoguer::Select panics on empty item lists.
    // Verify all variant lists used by the wizard are non-empty.
    assert!(
        !interactive::DefaultTool::all_variants().is_empty(),
        "DefaultTool::all_variants() must not be empty"
    );
    assert!(
        !interactive::HookScope::all_variants().is_empty(),
        "HookScope::all_variants() must not be empty"
    );
    assert!(
        !interactive::UpdateCheckPreference::all_variants().is_empty(),
        "UpdateCheckPreference::all_variants() must not be empty"
    );
}
